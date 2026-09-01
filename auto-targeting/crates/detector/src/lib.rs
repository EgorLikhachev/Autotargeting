//! # detector — независимый компонент детекции (TG26-35)
//!
//! Контур: **SHM-кольцо** (кадры, TG26-160) → существующий инференс-бэкенд
//! (NPU через rknn-bridge / ONNX x86 / mock) → **события на шине** (zenoh,
//! D-014) `at/detections` + статус `at/status/detector`.
//!
//! ## Guard-дисциплина
//!
//! Кадр копируется из слота и `FrameGuard` отпускается ДО инференса
//! (base64-round-trip до ~90 мс) — слот кольца не заморожен на время
//! тяжёлой работы, остальные потребители (рекордер и т.д.) не страдают.
//!
//! ## Контракт события
//!
//! [`event_bus::DetectionsFrame`]: `frame_seq` = id кадра кольца,
//! `captured_at` = метка захвата кадра, bbox в пикселях исходного кадра
//! (`frame_w` × `frame_h`).
//!
//! ## Координатный контракт бэкендов
//!
//! - **bridge (NPU)**: C++ принимает RGB24 при `input_width/height` из
//!   init и сам делает letterbox-unprojection в эти размеры → детектор
//!   обязан подать letterboxed 640×640 RGB24 и задать в init dims кольца.
//! - **cpu-onnx**: конвертирует/letterbox-ит/unproject-ит сам — подаём
//!   исходный кадр кольца.

use std::time::{Duration, Instant};

use common::{Frame, FrameMetadata, PixelFormat};
use cv_inference::InferenceBackend;
use event_bus::{topics, DetectionsFrame, EventBus};
use shmem_buffer::{FrameConsumer, FrameGuard, NextStep, StorageFormat};

/// Ошибки компонента.
#[derive(thiserror::Error, Debug)]
pub enum DetectorError {
    #[error("bus: {0}")]
    Bus(#[from] event_bus::BusError),
    #[error("ring: {0}")]
    Ring(#[from] shmem_buffer::RingError),
    #[error("inference: {0}")]
    Infer(#[from] cv_inference::backend::InferenceError),
    #[error("preprocess: {0}")]
    Preprocess(#[from] video_capture::ConversionError),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("no frames within quiet timeout")]
    NoFrames,
}

/// Выбор инференс-бэкенда.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendKind {
    /// NPU через rknn-bridge (unix).
    #[default]
    Bridge,
    /// ONNX Runtime (x86-dev; feature cpu-onnx).
    CpuOnnx,
    /// Мок (тесты/демо без модели).
    Mock,
}

/// Конфигурация компонента.
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// Имя сегмента SHM-кольца.
    pub segment: String,
    /// Путь к модели (bridge: .rknn; cpu-onnx: .onnx).
    pub model_path: String,
    pub backend: BackendKind,
    pub confidence_threshold: f32,
    pub nms_threshold: f32,
    /// Сокет rknn-bridge (bridge-бэкенд).
    pub bridge_socket: String,
    /// M6: путь SHM-сегмента кадров (пусто → base64-путь).
    pub frame_shm_path: String,
    /// Завершение при тишине кольца.
    pub quiet_timeout: Duration,
    /// Период публикации статуса.
    pub status_interval: Duration,
    /// Прекратить после N секунд (0 — пока жив стрим).
    pub max_duration: Option<Duration>,
    /// C5 (ADR D-017): NPU-троттлинг. Выше throttle_temp_c — работаем
    /// 1 кадр каждые throttle_every_n (пропуская остальные), ниже — штатно.
    /// 0.0 = выключено.
    pub throttle_temp_c: f32,
    /// Период троттлинга (каждый N-й кадр).
    pub throttle_every_n: u32,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            segment: "autotarget.frames".into(),
            model_path: "/opt/auto-targeting/models/yolov8n_int8.rknn".into(),
            backend: BackendKind::default(),
            confidence_threshold: 0.45,
            nms_threshold: 0.45,
            bridge_socket: "/tmp/rknn-bridge.sock".into(),
            frame_shm_path: "/dev/shm/at-infer".into(),
            quiet_timeout: Duration::from_secs(5),
            status_interval: Duration::from_secs(5),
            max_duration: None,
            throttle_temp_c: 85.0,
            throttle_every_n: 3,
        }
    }
}

/// Метрики контура (для статуса и приёмки).
#[derive(Debug, Clone, Copy, Default)]
pub struct DetectorStats {
    pub frames_processed: u64,
    pub frames_published: u64,
    pub jumps: u64,
    pub infer_errors: u64,
    pub detections_total: u64,
}

/// Снимок статуса на шину.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
pub struct DetectorStatus {
    pub fps: f32,
    pub infer_p50_us: u64,
    pub infer_p95_us: u64,
    pub e2e_p50_us: u64,
    pub frames_processed: u64,
    pub frames_published: u64,
    pub jumps: u64,
    pub infer_errors: u64,
    pub detections_total: u64,
    pub frame_w: u32,
    pub frame_h: u32,
}

/// Окно измерений (без аллокаций на кадр сверх вектора).
struct Metrics {
    infer_us: Vec<u64>,
    e2e_us: Vec<u64>,
    window_start: Instant,
    window_frames: u64,
}

impl Metrics {
    fn new() -> Self {
        Self {
            infer_us: Vec::with_capacity(512),
            e2e_us: Vec::with_capacity(512),
            window_start: Instant::now(),
            window_frames: 0,
        }
    }
    fn push(&mut self, infer_us: u64, e2e_us: u64) {
        if self.infer_us.len() >= 512 {
            self.rotate();
        }
        self.infer_us.push(infer_us);
        self.e2e_us.push(e2e_us);
        self.window_frames += 1;
    }
    fn rotate(&mut self) {
        let fps = self.fps();
        self.window_start = Instant::now();
        self.window_frames = 0;
        let _ = fps;
    }
    fn fps(&self) -> f32 {
        let secs = self.window_start.elapsed().as_secs_f32();
        if secs <= 0.0 {
            return 0.0;
        }
        self.window_frames as f32 / secs
    }
    fn p(stats: &[u64], q: f64) -> u64 {
        if stats.is_empty() {
            return 0;
        }
        let mut v = stats.to_vec();
        v.sort_unstable();
        let idx = ((q / 100.0) * (v.len() as f64 - 1.0)).round() as usize;
        v[idx.min(v.len() - 1)]
    }
    fn snapshot(&mut self, s: &DetectorStats, w: u32, h: u32) -> DetectorStatus {
        let fps = self.fps();
        self.window_start = Instant::now();
        self.window_frames = 0;
        DetectorStatus {
            fps,
            infer_p50_us: Self::p(&self.infer_us, 50.0),
            infer_p95_us: Self::p(&self.infer_us, 95.0),
            e2e_p50_us: Self::p(&self.e2e_us, 50.0),
            frames_processed: s.frames_processed,
            frames_published: s.frames_published,
            jumps: s.jumps,
            infer_errors: s.infer_errors,
            detections_total: s.detections_total,
            frame_w: w,
            frame_h: h,
        }
    }
}

/// Собрать бэкенд по конфигурации и геометрии кольца.
#[cfg_attr(not(unix), allow(unused_variables))]
fn build_backend(
    cfg: &DetectorConfig,
    w: u32,
    h: u32,
    frame_shm_path: &str,
) -> Result<Box<dyn InferenceBackend>, DetectorError> {
    match cfg.backend {
        BackendKind::Bridge => {
            #[cfg(unix)]
            {
                let bc = cv_inference::RknnBridgeConfig {
                    socket_path: cfg.bridge_socket.clone().into(),
                    model_path: cfg.model_path.clone(),
                    // Контракт C++: инит-размеры = исходный кадр (unprojection),
                    // формат — rgb24 (детектор подаёт letterboxed RGB).
                    input_width: w,
                    input_height: h,
                    input_format: "rgb24".into(),
                    confidence_threshold: cfg.confidence_threshold,
                    nms_threshold: cfg.nms_threshold,
                    // M6: кадры через именованный SHM-сегмент (без base64).
                    frame_shm: Some(frame_shm_path.to_string()),
                    frame_shm_buffers: 2,
                    ..Default::default()
                };
                Ok(Box::new(cv_inference::RknnBridgeClient::new(bc)))
            }
            #[cfg(not(unix))]
            Err(DetectorError::InvalidConfig(
                "bridge backend is Unix-only (rknn-bridge unix socket)".into(),
            ))
        }
        BackendKind::CpuOnnx => {
            #[cfg(feature = "cpu-onnx")]
            {
                Ok(Box::new(
                    cv_inference::CpuInferenceBackend::new(&cfg.model_path)
                        .with_confidence_threshold(cfg.confidence_threshold)
                        .with_iou_threshold(cfg.nms_threshold),
                ))
            }
            #[cfg(not(feature = "cpu-onnx"))]
            Err(DetectorError::InvalidConfig(
                "cpu-onnx backend requires feature cpu-onnx".into(),
            ))
        }
        BackendKind::Mock => Ok(Box::new(cv_inference::MockInferenceBackend::empty())),
    }
}

/// Конверсия КОПИИ кадра под бэкенд (вызывается после release guard —
/// B1: слот не должен быть занят на время конверсии ~мс).
fn preprocess_frame(
    raw: &Frame,
    format: StorageFormat,
    backend: BackendKind,
) -> Result<Frame, DetectorError> {
    match (backend, format) {
        (BackendKind::Bridge, StorageFormat::Nv12) => {
            let rgb = video_capture::nv12_to_rgb24(raw)?;
            let (lb, _) = yolov8::letterbox(&rgb.data, rgb.metadata.width, rgb.metadata.height);
            Ok(Frame {
                data: lb,
                metadata: FrameMetadata {
                    width: yolov8::INPUT_SIZE,
                    height: yolov8::INPUT_SIZE,
                    format: PixelFormat::Rgb24,
                    captured_at: rgb.metadata.captured_at,
                    seq: rgb.metadata.seq,
                },
            })
        }
        (BackendKind::Bridge, StorageFormat::Rgb24) => {
            let (lb, _) = yolov8::letterbox(&raw.data, raw.metadata.width, raw.metadata.height);
            Ok(Frame {
                data: lb,
                metadata: FrameMetadata {
                    width: yolov8::INPUT_SIZE,
                    height: yolov8::INPUT_SIZE,
                    format: PixelFormat::Rgb24,
                    captured_at: raw.metadata.captured_at,
                    seq: raw.metadata.seq,
                },
            })
        }
        (_, StorageFormat::Nv12) | (_, StorageFormat::Rgb24) => Ok(raw.clone()),
    }
}

/// Детектор-компонент.
pub struct Detector {
    cfg: DetectorConfig,
}

impl Detector {
    pub fn new(cfg: DetectorConfig) -> Result<Self, DetectorError> {
        if cfg.confidence_threshold < 0.0 || cfg.confidence_threshold > 1.0 {
            return Err(DetectorError::InvalidConfig("conf threshold".into()));
        }
        Ok(Self { cfg })
    }

    /// Основной контур. `bus` уже подключен; потребитель кольца передан.
    pub async fn run(
        &self,
        consumer: &FrameConsumer,
        bus: &EventBus,
    ) -> Result<DetectorStats, DetectorError> {
        // Геометрия и формат — из первого кадра (ring хранит их в каждом слоте).
        // B1 аудита: guard НЕ держится через build_backend+init (модель/коннект
        // могут занять секунды) — снимаем геометрию и отпускаем слот.
        let first = wait_first(consumer, self.cfg.quiet_timeout).await?;
        let (w, h, format, first_id) = {
            let v = first.view();
            (v.width, v.height, v.storage_format(), v.frame_id)
        };
        drop(first);
        let format =
            format.ok_or_else(|| DetectorError::InvalidConfig("unsupported ring format".into()))?;
        tracing::info!(w, h, ?format, backend = ?self.cfg.backend, "detector attached");

        let mut backend = build_backend(&self.cfg, w, h, &self.cfg.frame_shm_path)?;
        backend.init().await?;
        tracing::info!(backend = backend.name(), "inference backend ready");

        let det_pub = bus.publish_detections().await?;
        let status_pub = bus
            .publisher::<DetectorStatus>(&topics::status("detector"))
            .await?;

        let mut stats = DetectorStats::default();
        let mut metrics = Metrics::new();
        let mut last: u64 = first_id; // первый кадр уже учтён (слот отпущен)
        let mut pending: Option<FrameGuard> = None;
        let started = Instant::now();
        let mut last_frame_at = Instant::now();
        let mut last_status = Instant::now();
        // C5: кэш термалы (обновление ~5с) и счётчик троттлинга.
        let mut npu_temp_cache: Option<f32> = None;
        let mut thermal_last_poll = Instant::now() - Duration::from_secs(10);
        let mut throttle_skip: u32 = 0;

        loop {
            if let Some(max) = self.cfg.max_duration {
                if started.elapsed() >= max {
                    break;
                }
            }
            if last_frame_at.elapsed() >= self.cfg.quiet_timeout && stats.frames_processed > 0 {
                tracing::info!("quiet timeout: ring stalled, detector exiting");
                break;
            }

            let guard = match pending.take() {
                Some(g) => g,
                None => match acquire(consumer, &mut last, &mut stats) {
                    Some(g) => g,
                    None => {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                        continue;
                    }
                },
            };
            // C5: NPU-троттлинг — thermal-зонд раз в ~5с (дешёво, sysfs).
            if self.cfg.throttle_temp_c > 0.0 {
                thermal_poll(&mut npu_temp_cache, &mut thermal_last_poll);
                if let Some(t) = npu_temp_cache {
                    if t >= self.cfg.throttle_temp_c {
                        throttle_skip = throttle_skip.saturating_add(1);
                        if throttle_skip % self.cfg.throttle_every_n.max(1) != 0 {
                            continue; // кадр пропущен — NPU остывает
                        }
                    }
                }
            }
            last_frame_at = Instant::now();
            stats.frames_processed += 1;

            // === Guard-дисциплина (B1): под guard — ТОЛЬКО копия данных и
            // метаданных; конверсия/letterbox/инференс — после release.
            let (frame_id, ts_ns, raw_copy, raw_meta) = {
                let v = guard.view();
                (v.frame_id, v.ts_ns, guard.to_vec(), v.to_metadata())
            };
            drop(guard);
            let raw = common::Frame {
                data: raw_copy,
                metadata: raw_meta,
            };
            let prepared = preprocess_frame(&raw, format, self.cfg.backend)?;

            let t_infer = Instant::now();
            // Инференс (bridge-путь блокирующий внутри async — как в
            // live-demo; multi-thread реактор шины это покрывает).
            let detections = match backend.infer(&prepared).await {
                Ok(d) => d,
                Err(e) => {
                    stats.infer_errors += 1;
                    tracing::warn!(error = %e, "infer failed, skipping frame");
                    continue;
                }
            };
            let infer_us = t_infer.elapsed().as_micros() as u64;

            // Публикация события на шину.
            let event = DetectionsFrame {
                frame_seq: frame_id,
                captured_at: shmem_buffer::ts_ns_to_datetime(ts_ns),
                detections,
                frame_w: w,
                frame_h: h,
            };
            let e2e_us = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64)
                .saturating_sub(ts_ns)
                / 1000;
            stats.detections_total += event.detections.len() as u64;
            if det_pub.publish(&event).await.is_ok() {
                stats.frames_published += 1;
            }
            metrics.push(infer_us, e2e_us);

            // Статус-топик.
            if last_status.elapsed() >= self.cfg.status_interval {
                let snap = metrics.snapshot(&stats, w, h);
                let _ = status_pub.publish(&snap).await;
                tracing::info!(
                    fps = snap.fps,
                    infer_p50_us = snap.infer_p50_us,
                    e2e_p50_us = snap.e2e_p50_us,
                    "detector status"
                );
                last_status = Instant::now();
            }
        }

        let snap = metrics.snapshot(&stats, w, h);
        let _ = status_pub.publish(&snap).await;
        tracing::info!(
            published = stats.frames_published,
            processed = stats.frames_processed,
            "detector finished"
        );
        Ok(stats)
    }
}

/// Обновить кэш NPU-температуры не чаще раза в 5с (sysfs; C5).
fn thermal_poll(cache: &mut Option<f32>, last: &mut Instant) {
    if last.elapsed() >= Duration::from_secs(5) {
        *cache = system_telemetry::npu_temp_c().ok().flatten();
        *last = Instant::now();
    }
}

async fn wait_first(
    consumer: &FrameConsumer,
    timeout: Duration,
) -> Result<FrameGuard, DetectorError> {
    let deadline = Instant::now() + timeout;
    let mut last = 0u64;
    let mut stats = DetectorStats::default();
    while Instant::now() < deadline {
        if let Some(g) = acquire(consumer, &mut last, &mut stats) {
            return Ok(g);
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    Err(DetectorError::NoFrames)
}

fn acquire(
    consumer: &FrameConsumer,
    last: &mut u64,
    stats: &mut DetectorStats,
) -> Option<FrameGuard> {
    match consumer.next_after(*last) {
        NextStep::Frame(g) => {
            *last = g.frame_id();
            Some(g)
        }
        NextStep::UpToDate => None,
        NextStep::TooFarBehind { latest, .. } => {
            stats.jumps += 1;
            *last = latest;
            if let NextStep::Frame(g) = consumer.next_after(*last) {
                *last = g.frame_id();
                return Some(g);
            }
            None
        }
    }
}
