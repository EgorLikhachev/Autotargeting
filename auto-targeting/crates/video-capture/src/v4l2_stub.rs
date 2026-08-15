//! V4L2 video source — STUB (no `v4l2` feature).
//!
//! This is the fallback when the `v4l2` feature is not enabled.
//! It provides the same API surface (V4l2Source struct, device discovery
//! utilities) but `start()` always returns an error.
//!
//! Enable V4L2 support with:
//!   cargo build -p video-capture --features v4l2
//!
//! Requires `libclang-dev` at build time.

#![cfg(not(feature = "v4l2"))]

use crate::traits::{VideoCaptureError, VideoResult, VideoSource};
use async_trait::async_trait;
use common::{Frame, PixelFormat};
use tokio::sync::mpsc;
use tracing::warn;

/// Stub V4L2 source — returns an error when `start()` is called.
///
/// Enable the `v4l2` feature to get a real implementation:
///   `cargo build --features video-capture/v4l2`
pub struct V4l2Source {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: PixelFormat,
    pub queue_depth: usize,
}

impl V4l2Source {
    pub fn new(device: impl Into<String>, width: u32, height: u32, fps: u32) -> Self {
        Self {
            device: device.into(),
            width,
            height,
            fps,
            format: PixelFormat::Yuyv,
            queue_depth: 3,
        }
    }

    pub fn with_format(mut self, format: PixelFormat) -> Self {
        self.format = format;
        self
    }

    pub fn from_common(cfg: &common::VideoConfig) -> Self {
        let format = match cfg.format.as_str() {
            "nv12" => PixelFormat::Nv12,
            "yuyv" => PixelFormat::Yuyv,
            "rgb24" => PixelFormat::Rgb24,
            "mjpeg" => PixelFormat::Mjpeg,
            _ => PixelFormat::Yuyv,
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

    /// Probe is not available without the `v4l2` feature.
    pub fn probe(&self) -> VideoResult<DeviceProbe> {
        Err(VideoCaptureError::DeviceOpen(
            "V4L2 support not compiled in — enable the `v4l2` feature".to_string(),
        ))
    }
}

/// Device probe result — only available with `v4l2` feature.
#[derive(Debug, Clone)]
pub struct DeviceProbe {
    pub device_path: String,
    pub supported_formats: Vec<String>,
    pub requested_format: PixelFormat,
    pub requested_width: u32,
    pub requested_height: u32,
    pub requested_fps: u32,
}

impl std::fmt::Display for DeviceProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeviceProbe (stub)")
    }
}

#[async_trait]
impl VideoSource for V4l2Source {
    async fn start(&mut self) -> VideoResult<mpsc::Receiver<Frame>> {
        warn!(
            device = %self.device,
            "V4l2Source::start called but the `v4l2` feature is not enabled — returning error"
        );
        Err(VideoCaptureError::DeviceOpen(format!(
            "V4L2 support not compiled in (enable `v4l2` feature). Device: {}",
            self.device
        )))
    }

    async fn stop(&mut self) -> VideoResult<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "V4l2Source (stub — enable `v4l2` feature)"
    }
}

// === Device discovery utilities (always available) ===

pub fn device_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

pub fn list_v4l2_devices() -> Vec<String> {
    let mut devices = Vec::new();
    for i in 0..64 {
        let path = format!("/dev/video{i}");
        if std::path::Path::new(&path).exists() {
            devices.push(path);
        }
    }
    devices
}

pub fn query_formats(device: &str) -> VideoResult<String> {
    let output = std::process::Command::new("v4l2-ctl")
        .args(["--device", device, "--list-formats-ext"])
        .output()
        .map_err(|e| VideoCaptureError::DeviceOpen(format!("v4l2-ctl: {e}")))?;

    if !output.status.success() {
        return Err(VideoCaptureError::DeviceConfig(format!(
            "v4l2-ctl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_exists_returns_false_for_nonexistent() {
        assert!(!device_exists("/dev/video999"));
    }

    #[test]
    #[cfg(unix)] // /dev/null не существует на Windows — тест только для Unix-хостов
    fn device_exists_returns_true_for_dev_null() {
        assert!(device_exists("/dev/null"));
    }

    #[test]
    fn list_v4l2_devices_does_not_panic() {
        let _devices = list_v4l2_devices();
    }

    #[test]
    fn v4l2_source_construction() {
        let src = V4l2Source::new("/dev/video0", 1280, 720, 30);
        assert_eq!(src.device, "/dev/video0");
        assert_eq!(src.width, 1280);
        assert_eq!(src.fps, 30);
    }

    #[test]
    fn probe_returns_error_without_feature() {
        let src = V4l2Source::new("/dev/video0", 640, 480, 30);
        let result = src.probe();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("v4l2"));
    }

    #[tokio::test]
    async fn start_returns_error_without_feature() {
        let mut src = V4l2Source::new("/dev/video0", 640, 480, 30);
        let result = src.start().await;
        assert!(result.is_err());
    }

    /// Vivid-gated test — only meaningful with v4l2 feature, but kept here
    /// for discovery logic.
    #[test]
    #[ignore = "requires vivid kernel module: sudo modprobe vivid"]
    fn vivid_devices_detected() {
        let devices = list_v4l2_devices();
        if devices.is_empty() {
            eprintln!("SKIP: no V4L2 devices (run `sudo modprobe vivid`)");
            return;
        }
        println!("Found V4L2 devices: {devices:?}");
        assert!(!devices.is_empty());
    }
}
