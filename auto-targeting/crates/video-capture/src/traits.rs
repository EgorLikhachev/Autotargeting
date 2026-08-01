//! Trait definitions for video sources and sinks.
//!
//! The capture pipeline is split into:
//! - `VideoSource` — produces frames (V4L2 device, file replay, synthetic generator).
//! - `FrameSender` / `FrameReceiver` — channel-like abstraction for frame handoff.
//!
//! This decoupling lets us:
//! - Test the rest of the system with `SyntheticVideoSource` (no camera).
//! - Replay recorded sessions in regression tests.
//! - Switch V4L2 for a future MIPI-CSI source without touching downstream.

use async_trait::async_trait;
use common::Frame;
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum VideoCaptureError {
    #[error("device open error: {0}")]
    DeviceOpen(String),

    #[error("device configuration error: {0}")]
    DeviceConfig(String),

    #[error("capture error: {0}")]
    Capture(String),

    #[error("decode error: {0}")]
    Decode(String),

    #[error("device disconnected")]
    Disconnected,
}

pub type VideoResult<T> = std::result::Result<T, VideoCaptureError>;

/// A source of video frames. Implemented by:
/// - `V4l2Device` (Phase 1)
/// - `ReplaySource` (Phase 1 — for tests)
/// - `SyntheticSource` (Phase 0 — pattern generator for dev)
#[async_trait]
pub trait VideoSource: Send {
    /// Start capturing. Returns a receiver that yields frames.
    /// The source owns the capture task; dropping the receiver stops capture.
    async fn start(&mut self) -> VideoResult<mpsc::Receiver<Frame>>;

    /// Stop capturing. Idempotent.
    async fn stop(&mut self) -> VideoResult<()>;

    /// Human-readable name (e.g. "V4l2Device(/dev/video0)").
    fn name(&self) -> &str;
}

/// Convenience: create a channel for frame handoff.
pub fn frame_channel(buffer: usize) -> (mpsc::Sender<Frame>, mpsc::Receiver<Frame>) {
    mpsc::channel(buffer)
}
