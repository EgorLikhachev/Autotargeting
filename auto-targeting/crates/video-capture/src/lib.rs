//! Video capture module — V4L2 capture + MJPEG decode.
//!
//! Status: 🚧 Phase 0 scaffolding only.
//! Phase 1 will implement:
//! - `V4l2Device` — opens /dev/videoN, configures format/framerate, queues buffers.
//! - `MjpegDecoder` — decodes MJPEG frames to NV12 (preferred NPU input).
//! - `FrameQueue` — backpressure-aware queue (drop-old strategy).
//!
//! See `docs/HYPOTHESES.md` H-003 for camera compatibility assumptions.

pub mod traits;

pub use traits::{VideoCaptureError, VideoSource};

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
