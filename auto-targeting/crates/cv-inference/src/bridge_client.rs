//! Реальный клиент к C++ rknn-bridge микросервису.
//!
//! Заменяет stub-реализацию в `backend.rs`. Реализует IPC протокол
//! из ADR-0001: Unix domain socket + length-prefixed JSON.
//!
//! ## Протокол
//!
//! 1. Rust отправляет `init` JSON → bridge отвечает `init_ack`
//! 2. Для каждого кадра: Rust отправляет `infer` JSON → bridge отвечает `infer_ack`
//! 3. Periodically: `health` → `health_ack`
//! 4. При завершении: `shutdown` → `shutdown_ack`
//!
//! ## Frame data
//!
//! Frame передаётся inline в JSON как base64 (упрощение для Phase 2).
//! В production (Phase 6) — shared memory через memfd + SCM_RIGHTS.

use crate::backend::{InferenceBackend, InferenceError, InferenceResult};
use async_trait::async_trait;
use common::{BoundingBox, Detection, Frame};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Конфигурация для RknnBridgeClient.
#[derive(Debug, Clone)]
pub struct RknnBridgeConfig {
    /// Путь к Unix socket (например, "/tmp/rknn-bridge.sock").
    pub socket_path: PathBuf,
    /// Путь к .rknn модели (передаётся в init).
    pub model_path: String,
    /// Ширина входа модели (пиксели).
    pub input_width: u32,
    /// Высота входа модели (пиксели).
    pub input_height: u32,
    /// Формат входа модели ("nv12", "rgb24").
    pub input_format: String,
    /// Порог confidence.
    pub confidence_threshold: f32,
    /// Порог NMS IoU.
    pub nms_threshold: f32,
    /// Таймаут подключения (мс).
    pub connect_timeout_ms: u64,
    /// Таймаут ожидания ответа (мс).
    pub response_timeout_ms: u64,
    /// M6 (D-016): именованный SHM-сегмент для кадров (None → base64).
    /// Сегмент создаётся клиентом при init; кадры передаются без base64.
    pub frame_shm: Option<String>,
    /// Число буферов в сегменте (double buffering).
    pub frame_shm_buffers: u32,
}

impl Default for RknnBridgeConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/tmp/rknn-bridge.sock"),
            model_path: "/opt/auto-targeting/models/yolov8n_int8.rknn".to_string(),
            input_width: 1280,
            input_height: 720,
            input_format: "nv12".to_string(),
            confidence_threshold: 0.45,
            nms_threshold: 0.45,
            connect_timeout_ms: 5000,
            response_timeout_ms: 1000,
            frame_shm: None,
            frame_shm_buffers: 2,
        }
    }
}

impl RknnBridgeConfig {
    pub fn from_common(cfg: &common::InferenceConfig) -> Self {
        Self {
            socket_path: PathBuf::from(&cfg.bridge_socket),
            model_path: cfg.model_path.clone(),
            input_width: 1280, // из VideoConfig в реальном использовании
            input_height: 720,
            input_format: "nv12".to_string(),
            confidence_threshold: cfg.confidence_threshold,
            nms_threshold: cfg.nms_threshold,
            ..Default::default()
        }
    }
}

// === JSON message types ===

#[derive(Debug, Serialize)]
struct InitRequest<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    model_path: &'a str,
    input_width: u32,
    input_height: u32,
    input_format: &'a str,
    confidence_threshold: f32,
    nms_threshold: f32,
    /// M6: путь именованного SHM-сегмента (нет → base64 на bridge).
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_shm: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_shm_buffers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_shm_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct InitResponse {
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    output_classes: u32,
    #[serde(default)]
    #[allow(dead_code)]
    backend: String,
}

#[derive(Debug, Serialize)]
struct InferRequest<'a> {
    #[serde(rename = "type")]
    msg_type: &'a str,
    frame_seq: u64,
    /// None в shm-режиме (кадр лежит в сегменте).
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_data_b64: Option<String>,
    /// M6: индекс буфера в SHM-сегменте.
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_shm_buf: Option<u32>,
    frame_width: u32,
    frame_height: u32,
    frame_format: &'a str,
    captured_at_ms: u64,
}

#[derive(Debug, Deserialize)]
struct InferResponse {
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    #[allow(dead_code)]
    frame_seq: u64,
    #[serde(default)]
    latency_ms: u32,
    #[serde(default)]
    detections: Vec<DetectionJson>,
}

#[derive(Debug, Deserialize)]
struct DetectionJson {
    bbox: BBoxJson,
    class: String,
    class_id: u32,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct BBoxJson {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl From<DetectionJson> for Detection {
    fn from(d: DetectionJson) -> Self {
        Detection {
            bbox: BoundingBox {
                x: d.bbox.x,
                y: d.bbox.y,
                width: d.bbox.width,
                height: d.bbox.height,
            },
            class: d.class,
            class_id: d.class_id,
            confidence: d.confidence,
            frame_seq: 0, // устанавливается вызывающим
            detected_at: chrono::Utc::now(),
        }
    }
}

#[derive(Debug, Serialize)]
struct HealthRequest {
    #[serde(rename = "type")]
    msg_type: &'static str,
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    ok: bool,
    #[serde(default)]
    model_loaded: bool,
    #[serde(default)]
    npu_utilization: f32,
    #[serde(default)]
    #[allow(dead_code)]
    backend: String,
}

#[derive(Debug, Serialize)]
struct ShutdownRequest {
    #[serde(rename = "type")]
    msg_type: &'static str,
}

/// Реальный клиент к C++ rknn-bridge.
pub struct RknnBridgeClient {
    config: RknnBridgeConfig,
    stream: Option<UnixStream>,
    initialized: bool,
    #[allow(dead_code)]
    backend_name: String,
    #[allow(dead_code)]
    output_classes: u32,
    #[allow(dead_code)]
    /// Время последнего health check.
    last_health_check: Option<Instant>,
    /// M6: mmap SHM-сегмента кадров (см. FrameShm).
    frame_shm: Option<FrameShm>,
    /// Счётчик буферов (round-robin по frame_shm_buffers).
    shm_buf_cursor: u32,
}

/// Владение именованным SHM-сегментом кадров (M6, D-016):
/// создаётся при init, удаляется в Drop.
struct FrameShm {
    path: String,
    ptr: *mut u8,
    len: usize,
    frame_size: usize,
    buffers: u32,
}

// FrameShm: сырой mmap-указатель; доступ строго из владеющего клиента
// (infer — &mut self; указатель не расшаривается между потоками).
unsafe impl Send for FrameShm {}
unsafe impl Sync for FrameShm {}

impl FrameShm {
    fn create(path: &str, frame_size: usize, buffers: u32) -> Result<Self, String> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        let len = frame_size * buffers as usize;
        // O_CREAT|O_EXCL как в shmem-buffer: свежий сегмент.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("create {path}: {e}"))?;
        file.set_len(len as u64)
            .map_err(|e| format!("ftruncate {path}: {e}"))?;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                std::os::fd::AsRawFd::as_raw_fd(&file),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(format!("mmap {path}"));
        }
        Ok(Self {
            path: path.to_string(),
            ptr: ptr as *mut u8,
            len,
            frame_size,
            buffers,
        })
    }

    /// Записать кадр в буфер с индексом (memcpy ~1.2 МБ).
    fn write_frame(&mut self, index: u32, data: &[u8]) -> Result<(), String> {
        if index >= self.buffers {
            return Err(format!("buf index {index} >= {}", self.buffers));
        }
        if data.len() > self.frame_size {
            return Err(format!(
                "frame {} > buffer {}",
                data.len(),
                self.frame_size
            ));
        }
        let dst = unsafe { self.ptr.add(index as usize * self.frame_size) };
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len()) };
        Ok(())
    }
}

impl Drop for FrameShm {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut _, self.len);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl RknnBridgeClient {
    pub fn new(config: RknnBridgeConfig) -> Self {
        Self {
            config,
            stream: None,
            initialized: false,
            backend_name: String::new(),
            output_classes: 0,
            last_health_check: None,
            frame_shm: None,
            shm_buf_cursor: 0,
        }
    }

    pub fn from_common(cfg: &common::InferenceConfig) -> Self {
        Self::new(RknnBridgeConfig::from_common(cfg))
    }

    /// Подключиться к Unix socket.
    fn connect_socket(&self) -> InferenceResult<UnixStream> {
        let deadline = Instant::now() + Duration::from_millis(self.config.connect_timeout_ms);
        let mut last_err = None;

        while Instant::now() < deadline {
            match UnixStream::connect(&self.config.socket_path) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(
                            self.config.response_timeout_ms,
                        )))
                        .map_err(|e| {
                            InferenceError::BridgeConnection(format!("set_read_timeout: {e}"))
                        })?;
                    stream
                        .set_write_timeout(Some(Duration::from_millis(
                            self.config.response_timeout_ms,
                        )))
                        .map_err(|e| {
                            InferenceError::BridgeConnection(format!("set_write_timeout: {e}"))
                        })?;
                    debug!(path = ?self.config.socket_path, "connected to rknn-bridge");
                    return Ok(stream);
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }

        Err(InferenceError::BridgeConnection(format!(
            "failed to connect to {} within {}ms: {}",
            self.config.socket_path.display(),
            self.config.connect_timeout_ms,
            last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        )))
    }

    /// Отправить JSON message с length-prefix и получить ответ.
    fn send_recv(&mut self, json: &str) -> InferenceResult<String> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| InferenceError::BridgeConnection("not connected".to_string()))?;

        // 4-byte big-endian length prefix
        let len = json.len() as u32;
        stream
            .write_all(&len.to_be_bytes())
            .map_err(|e| InferenceError::BridgeProtocol(format!("write length: {e}")))?;
        stream
            .write_all(json.as_bytes())
            .map_err(|e| InferenceError::BridgeProtocol(format!("write data: {e}")))?;

        // Read response
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .map_err(|e| InferenceError::BridgeProtocol(format!("read length: {e}")))?;
        let resp_len = u32::from_be_bytes(len_buf) as usize;

        if resp_len > 10_000_000 {
            return Err(InferenceError::BridgeProtocol(format!(
                "response too large: {resp_len} bytes"
            )));
        }

        let mut resp_buf = vec![0u8; resp_len];
        stream
            .read_exact(&mut resp_buf)
            .map_err(|e| InferenceError::BridgeProtocol(format!("read data: {e}")))?;

        String::from_utf8(resp_buf)
            .map_err(|e| InferenceError::BridgeProtocol(format!("utf8 decode: {e}")))
    }

    /// Base64 encode (перф-аудит 2026-08: быстрый путь без ветвлений на
    /// полный 3-байтовый чанк; байты пишутся в Vec<u8> — String::push(char)
    /// делал UTF-8-энкод на каждый из 1.8M символов кадра; хвост ≤2 байт
    /// обрабатывается отдельно).
    fn base64_encode(data: &[u8]) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);

        let mut quad = [0u8; 4];
        for chunk in data.chunks_exact(3) {
            let triple =
                (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
            quad[0] = CHARS[((triple >> 18) & 0x3F) as usize];
            quad[1] = CHARS[((triple >> 12) & 0x3F) as usize];
            quad[2] = CHARS[((triple >> 6) & 0x3F) as usize];
            quad[3] = CHARS[(triple & 0x3F) as usize];
            out.extend_from_slice(&quad);
        }

        match data.chunks_exact(3).remainder() {
            [b0] => {
                let triple = u32::from(*b0) << 16;
                out.extend_from_slice(&[
                    CHARS[((triple >> 18) & 0x3F) as usize],
                    CHARS[((triple >> 12) & 0x3F) as usize],
                    b'=',
                    b'=',
                ]);
            }
            [b0, b1] => {
                let triple = (u32::from(*b0) << 16) | (u32::from(*b1) << 8);
                out.extend_from_slice(&[
                    CHARS[((triple >> 18) & 0x3F) as usize],
                    CHARS[((triple >> 12) & 0x3F) as usize],
                    CHARS[((triple >> 6) & 0x3F) as usize],
                    b'=',
                ]);
            }
            _ => {}
        }

        // Вывод base64 — чистый ASCII; from_utf8 — одна валидирующая проверка.
        String::from_utf8(out).expect("base64 is ASCII")
    }
}

#[async_trait]
impl InferenceBackend for RknnBridgeClient {
    async fn init(&mut self) -> InferenceResult<()> {
        info!(
            socket = ?self.config.socket_path,
            model = %self.config.model_path,
            "initializing rknn-bridge client"
        );

        // Подключиться к socket
        let stream = self.connect_socket()?;
        self.stream = Some(stream);

        // M6: при первом init создаём SHM-сегмент кадров (если запрошен).
        // Формат bridged-кадра сегодня — letterboxed RGB24 input_size²
        // (детектор подаёт его так), поэтому размер буфера фиксирован.
        let mut frame_shm_path: Option<String> = None;
        let mut frame_shm_buffers: Option<u32> = None;
        let mut frame_shm_size: Option<u64> = None;
        if self.config.frame_shm.is_some() && self.frame_shm.is_none() {
            let path = self.config.frame_shm.clone().expect("checked");
            // Сегмент мог остаться от крэша — пересоздаём.
            let _ = std::fs::remove_file(&path);
            // Размер: формат входа на сегодня rgb24 (letterbox в детекторе).
            // Для nv12-инференса размер бы считался иначе; контракт D-016 —
            // буфер = input_width*input_height*3.
            let frame_size =
                self.config.input_width as usize * self.config.input_height as usize * 3;
            match FrameShm::create(&path, frame_size, self.config.frame_shm_buffers) {
                Ok(shm) => {
                    frame_shm_buffers = Some(shm.buffers);
                    frame_shm_size = Some(shm.frame_size as u64);
                    frame_shm_path = Some(shm.path.clone());
                    self.frame_shm = Some(shm);
                    info!(path = %path, "frame shm segment created");
                }
                Err(e) => {
                    // Деградация: base64-путь остаётся рабочим.
                    warn!(error = %e, "frame shm create failed — base64 fallback");
                    self.config.frame_shm = None;
                }
            }
        }

        // Отправить init
        let req = InitRequest {
            msg_type: "init",
            model_path: &self.config.model_path,
            input_width: self.config.input_width,
            input_height: self.config.input_height,
            input_format: &self.config.input_format,
            confidence_threshold: self.config.confidence_threshold,
            nms_threshold: self.config.nms_threshold,
            frame_shm: frame_shm_path.as_deref(),
            frame_shm_buffers,
            frame_shm_size,
        };
        let json = serde_json::to_string(&req)
            .map_err(|e| InferenceError::BridgeProtocol(format!("serialize init: {e}")))?;

        let resp_json = self.send_recv(&json)?;
        let resp: InitResponse = serde_json::from_str(&resp_json)
            .map_err(|e| InferenceError::BridgeProtocol(format!("deserialize init_ack: {e}")))?;

        if !resp.ok {
            return Err(InferenceError::BridgeConnection(format!(
                "init failed: {}",
                resp.error
            )));
        }

        self.initialized = true;
        self.backend_name = resp.backend.clone();
        self.output_classes = resp.output_classes;

        info!(
            backend = %resp.backend,
            classes = resp.output_classes,
            "rknn-bridge initialized"
        );
        Ok(())
    }

    async fn infer(&mut self, frame: &Frame) -> InferenceResult<Vec<Detection>> {
        if !self.initialized {
            return Err(InferenceError::ModelNotLoaded);
        }

        let format_str = match frame.metadata.format {
            common::PixelFormat::Nv12 => "nv12",
            common::PixelFormat::Rgb24 => "rgb24",
            common::PixelFormat::Yuyv => "yuyv",
            common::PixelFormat::Mjpeg => "mjpeg",
        };

        // M6: кадр через SHM-сегмент (round-robin по буферам), иначе base64.
        let (frame_data_b64, frame_shm_buf) = if let Some(shm) = self.frame_shm.as_mut() {
            let index = self.shm_buf_cursor % shm.buffers;
            self.shm_buf_cursor = self.shm_buf_cursor.wrapping_add(1);
            if let Err(e) = shm.write_frame(index, &frame.data) {
                return Err(InferenceError::BridgeProtocol(format!(
                    "shm write: {e}"
                )));
            }
            (None, Some(index))
        } else {
            (Some(Self::base64_encode(&frame.data)), None)
        };

        let req = InferRequest {
            msg_type: "infer",
            frame_seq: frame.metadata.seq,
            frame_data_b64,
            frame_shm_buf,
            frame_width: frame.metadata.width,
            frame_height: frame.metadata.height,
            frame_format: format_str,
            captured_at_ms: frame.metadata.captured_at.timestamp_millis() as u64,
        };

        let json = serde_json::to_string(&req)
            .map_err(|e| InferenceError::BridgeProtocol(format!("serialize infer: {e}")))?;

        let resp_json = self.send_recv(&json)?;
        let resp: InferResponse = serde_json::from_str(&resp_json)
            .map_err(|e| InferenceError::BridgeProtocol(format!("deserialize infer_ack: {e}")))?;

        if !resp.ok {
            return Err(InferenceError::Inference(format!(
                "infer failed: {}",
                resp.error
            )));
        }

        // Convert JSON detections to common::Detection
        let mut detections: Vec<Detection> =
            resp.detections.into_iter().map(Detection::from).collect();

        // Set frame_seq on all detections
        for d in &mut detections {
            d.frame_seq = frame.metadata.seq;
        }

        debug!(
            frame_seq = frame.metadata.seq,
            latency_ms = resp.latency_ms,
            detection_count = detections.len(),
            "inference complete"
        );

        Ok(detections)
    }

    async fn health_check(&self) -> InferenceResult<()> {
        if !self.initialized {
            return Err(InferenceError::ModelNotLoaded);
        }

        // Используем self.send_recv через mutable ref — но health_check это &self.
        // Создаём временный clone connection для health check.
        // В реальном использовании health_check вызывается редко, так что это OK.
        let mut stream = UnixStream::connect(&self.config.socket_path)
            .map_err(|e| InferenceError::BridgeConnection(format!("health connect: {e}")))?;

        let req = HealthRequest { msg_type: "health" };
        let json = serde_json::to_string(&req)
            .map_err(|e| InferenceError::BridgeProtocol(format!("serialize health: {e}")))?;

        // Send
        let len = json.len() as u32;
        stream
            .write_all(&len.to_be_bytes())
            .map_err(|e| InferenceError::BridgeProtocol(format!("write: {e}")))?;
        stream
            .write_all(json.as_bytes())
            .map_err(|e| InferenceError::BridgeProtocol(format!("write: {e}")))?;

        // Read response
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .map_err(|e| InferenceError::BridgeProtocol(format!("read: {e}")))?;
        let resp_len = u32::from_be_bytes(len_buf) as usize;
        let mut resp_buf = vec![0u8; resp_len];
        stream
            .read_exact(&mut resp_buf)
            .map_err(|e| InferenceError::BridgeProtocol(format!("read: {e}")))?;

        let resp: HealthResponse = serde_json::from_slice(&resp_buf)
            .map_err(|e| InferenceError::BridgeProtocol(format!("deserialize health_ack: {e}")))?;

        if !resp.ok {
            return Err(InferenceError::BridgeConnection(
                "health check failed".to_string(),
            ));
        }

        debug!(
            model_loaded = resp.model_loaded,
            npu_utilization = resp.npu_utilization,
            "health check passed"
        );
        Ok(())
    }

    fn name(&self) -> &str {
        "RknnBridgeClient"
    }
}

impl Drop for RknnBridgeClient {
    fn drop(&mut self) {
        if self.initialized {
            // Try to send shutdown (best-effort)
            let req = ShutdownRequest {
                msg_type: "shutdown",
            };
            if let Ok(json) = serde_json::to_string(&req) {
                let _ = self.send_recv(&json);
                debug!("sent shutdown to rknn-bridge");
            } else {
                warn!("failed to serialize shutdown request");
            }
        }
        // stream will be closed on drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default() {
        let cfg = RknnBridgeConfig::default();
        assert_eq!(cfg.socket_path, PathBuf::from("/tmp/rknn-bridge.sock"));
        assert_eq!(cfg.input_width, 1280);
        assert_eq!(cfg.confidence_threshold, 0.45);
    }

    #[test]
    fn config_from_common() {
        let common_cfg = common::InferenceConfig {
            model_path: "/tmp/model.rknn".to_string(),
            bridge_socket: "/tmp/test.sock".to_string(),
            confidence_threshold: 0.5,
            nms_threshold: 0.4,
            ..Default::default()
        };
        let cfg = RknnBridgeConfig::from_common(&common_cfg);
        assert_eq!(cfg.model_path, "/tmp/model.rknn");
        assert_eq!(cfg.socket_path, PathBuf::from("/tmp/test.sock"));
        assert_eq!(cfg.confidence_threshold, 0.5);
    }

    #[test]
    fn base64_encode_empty() {
        assert_eq!(RknnBridgeClient::base64_encode(&[]), "");
    }

    #[test]
    fn base64_encode_one_byte() {
        // 'f' (0x66) → "Zg=="
        assert_eq!(RknnBridgeClient::base64_encode(&[0x66]), "Zg==");
    }

    #[test]
    fn base64_encode_two_bytes() {
        // "fo" (0x66, 0x6F) → "Zm8="
        assert_eq!(RknnBridgeClient::base64_encode(&[0x66, 0x6F]), "Zm8=");
    }

    #[test]
    fn base64_encode_three_bytes() {
        // "foo" (0x66, 0x6F, 0x6F) → "Zm9v"
        assert_eq!(RknnBridgeClient::base64_encode(&[0x66, 0x6F, 0x6F]), "Zm9v");
    }

    #[test]
    fn base64_encode_known_string() {
        // "Hello, World!" → "SGVsbG8sIFdvcmxkIQ=="
        let data = b"Hello, World!";
        let encoded = RknnBridgeClient::base64_encode(data);
        assert_eq!(encoded, "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn client_construction_does_not_connect() {
        let cfg = RknnBridgeConfig::default();
        let client = RknnBridgeClient::new(cfg);
        assert!(!client.initialized);
        assert!(client.stream.is_none());
    }

    #[tokio::test]
    async fn init_fails_when_socket_not_listening() {
        let cfg = RknnBridgeConfig {
            socket_path: PathBuf::from("/tmp/nonexistent-rknn-bridge.sock"),
            connect_timeout_ms: 500, // short timeout for test
            ..Default::default()
        };
        let mut client = RknnBridgeClient::new(cfg);
        let result = client.init().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, InferenceError::BridgeConnection(_)));
    }

    #[test]
    fn detection_json_conversion() {
        let dj = DetectionJson {
            bbox: BBoxJson {
                x: 100,
                y: 200,
                width: 50,
                height: 80,
            },
            class: "person".to_string(),
            class_id: 0,
            confidence: 0.92,
        };
        let det: Detection = dj.into();
        assert_eq!(det.bbox.x, 100);
        assert_eq!(det.bbox.y, 200);
        assert_eq!(det.bbox.width, 50);
        assert_eq!(det.bbox.height, 80);
        assert_eq!(det.class, "person");
        assert_eq!(det.class_id, 0);
        assert!((det.confidence - 0.92).abs() < 1e-6);
    }

    #[test]
    fn init_request_serialization() {
        let req = InitRequest {
            msg_type: "init",
            model_path: "/tmp/model.rknn",
            input_width: 1280,
            input_height: 720,
            input_format: "nv12",
            confidence_threshold: 0.45,
            nms_threshold: 0.45,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"type\":\"init\""));
        assert!(json.contains("\"model_path\":\"/tmp/model.rknn\""));
        assert!(json.contains("\"input_width\":1280"));
        assert!(json.contains("\"input_height\":720"));
        assert!(json.contains("\"input_format\":\"nv12\""));
        assert!(json.contains("\"confidence_threshold\":0.45"));
    }

    #[test]
    fn init_response_deserialization() {
        let json = r#"{"ok":true,"output_classes":80,"backend":"stub"}"#;
        let resp: InitResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.output_classes, 80);
        assert_eq!(resp.backend, "stub");
    }

    #[test]
    fn init_response_deserialization_with_error() {
        let json = r#"{"ok":false,"error":"model not found"}"#;
        let resp: InitResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error, "model not found");
    }

    #[test]
    fn infer_response_deserialization() {
        let json = r#"{
            "ok": true,
            "frame_seq": 123,
            "latency_ms": 45,
            "detections": [
                {
                    "bbox": {"x": 100, "y": 200, "width": 50, "height": 80},
                    "class": "person",
                    "class_id": 0,
                    "confidence": 0.92
                }
            ]
        }"#;
        let resp: InferResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.frame_seq, 123);
        assert_eq!(resp.latency_ms, 45);
        assert_eq!(resp.detections.len(), 1);
        assert_eq!(resp.detections[0].class, "person");
        assert!((resp.detections[0].confidence - 0.92).abs() < 1e-6);
    }

    /// Integration test: запустить C++ rknn-bridge, подключиться, сделать infer.
    /// Требует собранного бинарника rknn-bridge.
    #[tokio::test]
    #[ignore = "requires rknn-bridge binary running"]
    async fn integration_with_real_bridge() {
        let cfg = RknnBridgeConfig::default();
        let mut client = RknnBridgeClient::new(cfg);

        client.init().await.expect("init should succeed");
        assert!(client.initialized);
        assert_eq!(client.backend_name, "stub");

        // Health check
        client
            .health_check()
            .await
            .expect("health check should pass");

        // Infer with a dummy frame
        let frame = common::Frame {
            data: vec![0u8; 1280 * 720 * 3 / 2], // NV12
            metadata: common::FrameMetadata {
                width: 1280,
                height: 720,
                format: common::PixelFormat::Nv12,
                captured_at: chrono::Utc::now(),
                seq: 1,
            },
        };

        let detections = client.infer(&frame).await.expect("infer should succeed");
        // Stub backend returns 1-2 detections
        assert!(!detections.is_empty());
    }
}
