//! # tracker — компонент сопровождения целей (M2 плана миграции шины)
//!
//! Подписка на [`event_bus`] `at/detections` → `MultiTargetTracker`
//! (Kalman + Hungarian, крейт `target-tracker`) → публикация `at/tracks`
//! (`TrackMsg` на каждый активный трек на каждый кадр) + статус
//! `at/status/tracker`.
//!
//! Первый компонент-**потребитель** шины: демонстрирует полный цикл
//! детектор → трекер на новой архитектуре. Кадры не нужны — трекинг по
//! bbox-событиям (пиксельный контекст при необходимости добавляется чтением
//! кольца по `frame_seq`).

use std::time::{Duration, Instant};

use common::Detection;
use event_bus::{topics, DetectionsFrame, EventBus, TrackMsg};
use target_tracker::MultiTargetTracker;

/// Ошибки компонента.
#[derive(thiserror::Error, Debug)]
pub enum TrackerError {
    #[error("bus: {0}")]
    Bus(#[from] event_bus::BusError),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}

/// Конфигурация.
#[derive(Debug, Clone)]
pub struct TrackerConfig {
    /// Endpoint шины.
    pub bus: String,
    /// Максимальный возраст трека, мс (порог LOST).
    pub max_target_age_ms: u64,
    /// Максимальное число пропущенных кадров.
    pub max_missed_frames: u32,
    /// IoU-порог сопоставления детекция↔трек.
    pub match_iou_threshold: f32,
    /// Публиковать только подтверждённые (locked) треки.
    pub locked_only: bool,
    /// Период статуса.
    pub status_interval: Duration,
    /// Прекратить после N секунд (None — пока жива шина/детекции).
    pub max_duration: Option<Duration>,
    /// Тишина детекций до завершения.
    pub quiet_timeout: Option<Duration>,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            bus: "tcp/127.0.0.1:7447".into(),
            max_target_age_ms: 2000,
            max_missed_frames: 60,
            match_iou_threshold: 0.3,
            locked_only: false,
            status_interval: Duration::from_secs(5),
            max_duration: None,
            quiet_timeout: None,
        }
    }
}

/// Статус компонента (at/status/tracker).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackerStatus {
    pub v: u8,
    pub frames_in: u64,
    pub tracks_published: u64,
    pub active_tracks: usize,
    pub fps: f32,
}

/// Метрики окна.
struct Metrics {
    window_start: Instant,
    window_frames: u64,
}

impl Metrics {
    fn new() -> Self {
        Self { window_start: Instant::now(), window_frames: 0 }
    }
    fn bump(&mut self) {
        self.window_frames += 1;
    }
    fn fps(&mut self) -> f32 {
        let secs = self.window_start.elapsed().as_secs_f32();
        let fps = if secs > 0.0 { self.window_frames as f32 / secs } else { 0.0 };
        self.window_start = Instant::now();
        self.window_frames = 0;
        fps
    }
}

/// Компонент-трекер.
pub struct Tracker {
    cfg: TrackerConfig,
}

impl Tracker {
    pub fn new(cfg: TrackerConfig) -> Result<Self, TrackerError> {
        if !(0.0..=1.0).contains(&cfg.match_iou_threshold) {
            return Err(TrackerError::InvalidConfig("iou threshold".into()));
        }
        Ok(Self { cfg })
    }

    /// Основной цикл: детекции → треки → шина.
    pub async fn run(&self, bus: &EventBus) -> Result<(), TrackerError> {
        let det_sub = bus.subscribe_detections().await?;
        let track_pub = bus.publish_tracks().await?;
        let status_pub = bus
            .publisher::<TrackerStatus>(&topics::status("tracker"))
            .await?;

        let mut mt = MultiTargetTracker::new(
            self.cfg.max_target_age_ms,
            self.cfg.max_missed_frames,
            self.cfg.match_iou_threshold,
        )
        .with_auto_create(true);

        let mut frames_in: u64 = 0;
        let mut tracks_published: u64 = 0;
        let mut metrics = Metrics::new();
        let started = Instant::now();
        let mut last_det = Instant::now();
        let mut last_status = Instant::now();
        tracing::info!("tracker ready: consuming at/detections");

        loop {
            if let Some(max) = self.cfg.max_duration {
                if started.elapsed() >= max {
                    break;
                }
            }
            if let Some(q) = self.cfg.quiet_timeout {
                if frames_in > 0 && last_det.elapsed() >= q {
                    tracing::info!("quiet timeout: no detections, tracker exiting");
                    break;
                }
            }

            // Ждём следующий пакет детекций (таймаут — для статуса/лимитов).
            let event = match det_sub.recv_timeout(Duration::from_millis(500)).await {
                Ok(e) => e,
                Err(event_bus::BusError::Timeout) => {
                    if last_status.elapsed() >= self.cfg.status_interval {
                        let snap = TrackerStatus {
                            v: event_bus::CONTRACT_VERSION,
                            frames_in,
                            tracks_published,
                            active_tracks: mt.track_count(),
                            fps: metrics.fps(),
                        };
                        let _ = status_pub.publish(&snap).await;
                        last_status = Instant::now();
                    }
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            last_det = Instant::now();
            frames_in += 1;
            metrics.bump();

            let DetectionsFrame { detections, frame_seq, .. } = event;

            // Штатный трекер принимает &[Detection].
            mt.update(&detections);

            // Публикация активных треков этого кадра.
            for t in mt.active_tracks() {
                if self.cfg.locked_only && !mt.is_locked(t.id) {
                    continue;
                }
                let msg = TrackMsg {
                    v: event_bus::CONTRACT_VERSION,
                    track_id: t.id,
                    frame_seq,
                    bbox: t.bbox,
                    vx: t.velocity.0,
                    vy: t.velocity.1,
                    class: detections
                        .iter()
                        .max_by(|a, b| {
                            a.confidence
                                .partial_cmp(&b.confidence)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|d| d.class.clone())
                        .unwrap_or_default(),
                    class_id: 0,
                    confidence: t.confidence,
                    age: t.missed_frames,
                    misses: t.missed_frames,
                };
                if track_pub.publish(&msg).await.is_ok() {
                    tracks_published += 1;
                }
            }

            if last_status.elapsed() >= self.cfg.status_interval {
                let snap = TrackerStatus {
                    v: event_bus::CONTRACT_VERSION,
                    frames_in,
                    tracks_published,
                    active_tracks: mt.track_count(),
                    fps: metrics.fps(),
                };
                let _ = status_pub.publish(&snap).await;
                tracing::info!(
                    frames_in,
                    tracks_published,
                    active = mt.track_count(),
                    "tracker status"
                );
                last_status = Instant::now();
            }
        }

        let snap = TrackerStatus {
            v: event_bus::CONTRACT_VERSION,
            frames_in,
            tracks_published,
            active_tracks: mt.track_count(),
            fps: 0.0,
        };
        let _ = status_pub.publish(&snap).await;
        tracing::info!(frames_in, tracks_published, "tracker finished");
        Ok(())
    }
}

/// Преобразование Detection → событие шины (для тестов/утилит).
#[must_use]
pub fn detections_to_frame(frame_seq: u64, detections: Vec<Detection>) -> DetectionsFrame {
    DetectionsFrame {
        frame_seq,
        captured_at: chrono::Utc::now(),
        detections,
        frame_w: 0,
        frame_h: 0,
    }
}
