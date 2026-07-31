//! Inference backend abstraction.
//!
//! Trait that abstracts over different inference engines:
//! - `RknnBridgeClient` — production, talks to C++ RKNN microservice (Phase 2).
//! - `CpuInferenceBackend` — ONNX Runtime fallback (Phase 2).
//! - `MockInferenceBackend` — for tests, returns canned detections.

use async_trait::async_trait;
use common::{Detection, Frame};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("bridge connection error: {0}")]
    BridgeConnection(String),

    #[error("bridge protocol error: {0}")]
    BridgeProtocol(String),

    #[error("inference failed: {0}")]
    Inference(String),

    #[error("model not loaded")]
    ModelNotLoaded,

    #[error("backend unavailable: {0}")]
    Unavailable(String),
}

pub type InferenceResult<T> = std::result::Result<T, InferenceError>;

/// Inference backend — produces detections from frames.
#[async_trait]
pub trait InferenceBackend: Send {
    /// Initialize the backend (load model, connect to bridge, etc.).
    async fn init(&mut self) -> InferenceResult<()>;

    /// Run inference on a single frame. Returns detections in image coordinates.
    async fn infer(&mut self, frame: &Frame) -> InferenceResult<Vec<Detection>>;

    /// Health check — is the backend ready to accept inference requests?
    async fn health_check(&self) -> InferenceResult<()>;

    /// Human-readable backend name.
    fn name(&self) -> &str;
}

/// Stub for the RKNN bridge client (Phase 2).
pub struct RknnBridgeClient {
    pub socket_path: String,
    pub model_path: String,
}

impl RknnBridgeClient {
    pub fn new(socket_path: impl Into<String>, model_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            model_path: model_path.into(),
        }
    }
}

#[async_trait]
impl InferenceBackend for RknnBridgeClient {
    async fn init(&mut self) -> InferenceResult<()> {
        tracing::warn!(
            socket = %self.socket_path,
            "RknnBridgeClient::init not yet implemented (Phase 2)"
        );
        Err(InferenceError::Unavailable(
            "RknnBridgeClient not implemented yet (Phase 2)".to_string(),
        ))
    }

    async fn infer(&mut self, _frame: &Frame) -> InferenceResult<Vec<Detection>> {
        Err(InferenceError::Unavailable(
            "RknnBridgeClient not implemented yet (Phase 2)".to_string(),
        ))
    }

    async fn health_check(&self) -> InferenceResult<()> {
        Err(InferenceError::Unavailable(
            "RknnBridgeClient not implemented yet (Phase 2)".to_string(),
        ))
    }

    fn name(&self) -> &str {
        "RknnBridgeClient"
    }
}

/// Stub for CPU ONNX Runtime fallback (Phase 2).
pub struct CpuInferenceBackend {
    pub model_path: String,
}

impl CpuInferenceBackend {
    pub fn new(model_path: impl Into<String>) -> Self {
        Self {
            model_path: model_path.into(),
        }
    }
}

#[async_trait]
impl InferenceBackend for CpuInferenceBackend {
    async fn init(&mut self) -> InferenceResult<()> {
        Err(InferenceError::Unavailable(
            "CpuInferenceBackend not implemented yet (Phase 2)".to_string(),
        ))
    }

    async fn infer(&mut self, _frame: &Frame) -> InferenceResult<Vec<Detection>> {
        Err(InferenceError::Unavailable(
            "CpuInferenceBackend not implemented yet (Phase 2)".to_string(),
        ))
    }

    async fn health_check(&self) -> InferenceResult<()> {
        Err(InferenceError::Unavailable(
            "CpuInferenceBackend not implemented yet (Phase 2)".to_string(),
        ))
    }

    fn name(&self) -> &str {
        "CpuInferenceBackend"
    }
}

/// Mock backend — returns canned detections. For tests and dev.
pub struct MockInferenceBackend {
    detections: Vec<Detection>,
}

impl MockInferenceBackend {
    pub fn new(detections: Vec<Detection>) -> Self {
        Self { detections }
    }

    pub fn empty() -> Self {
        Self {
            detections: Vec::new(),
        }
    }
}

#[async_trait]
impl InferenceBackend for MockInferenceBackend {
    async fn init(&mut self) -> InferenceResult<()> {
        Ok(())
    }

    async fn infer(&mut self, _frame: &Frame) -> InferenceResult<Vec<Detection>> {
        Ok(self.detections.clone())
    }

    async fn health_check(&self) -> InferenceResult<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "MockInferenceBackend"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common::{BoundingBox, PixelFormat};

    fn make_frame() -> Frame {
        Frame {
            data: vec![0u8; 1280 * 720 * 3 / 2],
            metadata: common::FrameMetadata {
                width: 1280,
                height: 720,
                format: PixelFormat::Nv12,
                captured_at: Utc::now(),
                seq: 1,
            },
        }
    }

    #[tokio::test]
    async fn mock_backend_returns_canned_detections() {
        let dets = vec![Detection {
            bbox: BoundingBox {
                x: 100,
                y: 100,
                width: 50,
                height: 80,
            },
            class: "person".to_string(),
            class_id: 0,
            confidence: 0.9,
            frame_seq: 1,
            detected_at: Utc::now(),
        }];
        let mut be = MockInferenceBackend::new(dets.clone());
        be.init().await.unwrap();
        let result = be.infer(&make_frame()).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].class, "person");
    }

    #[tokio::test]
    async fn rknn_bridge_returns_unavailable() {
        let mut be = RknnBridgeClient::new("/tmp/test.sock", "/tmp/model.rknn");
        let res = be.init().await;
        assert!(res.is_err());
    }
}
