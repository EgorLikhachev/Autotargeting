//! Video capture module — V4L2 capture + synthetic sources for testing.
//!
//! ## Status
//!
//! - `SyntheticVideoSource` — ✅ Working. Generates test patterns, no kernel
//!   modules needed. Used in CI and dev.
//! - `ReplaySource` — ✅ Working. Reads recorded frames from disk (Phase 6.3).
//! - `V4l2Source` — 🚧 Phase 1. Stub only; will use `v4l2` crate to open real
//!   devices or `vivid` kernel module for CI.
//!
//! ## CI strategy
//!
//! CI runners (GitHub Actions Ubuntu) have the `vivid` kernel module available.
//! To enable it: `sudo modprobe vivid`. Then `/dev/video0` exists and produces
//! test patterns. Tests guarded by `#[cfg(feature = "vivid-ci")]` or by a
//! runtime check for `/dev/video*` existence.

pub mod replay;
pub mod synthetic;
pub mod traits;
pub mod v4l2_stub;

pub use replay::ReplaySource;
pub use synthetic::{SyntheticConfig, SyntheticPattern, SyntheticVideoSource};
pub use traits::{VideoCaptureError, VideoSource};
pub use v4l2_stub::V4l2Source;

/// Re-export common frame types for convenience.
pub use common::{Frame, FrameMetadata, PixelFormat};

/// Configuration for a video source.
#[derive(Debug, Clone)]
pub struct VideoSourceConfig {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: PixelFormat,
    pub queue_depth: usize,
}

impl VideoSourceConfig {
    pub fn from_common(cfg: &common::VideoConfig) -> Self {
        let format = match cfg.format.as_str() {
            "nv12" => PixelFormat::Nv12,
            "yuyv" => PixelFormat::Yuyv,
            "rgb24" => PixelFormat::Rgb24,
            "mjpeg" => PixelFormat::Mjpeg,
            other => {
                tracing::warn!(
                    format = other,
                    "unknown pixel format string, defaulting to MJPEG"
                );
                PixelFormat::Mjpeg
            }
        };
        Self {
            device: cfg.device.clone(),
            width: cfg.width,
            height: cfg.height,
            fps: cfg.fps,
            format,
            queue_depth: cfg.queue_depth,
        }
    }
}
