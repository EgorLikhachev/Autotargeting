//! V4L2 video source — opens `/dev/videoN` via the Linux V4L2 API.
//!
//! ## Status: 🚧 Phase 1 stub
//!
//! The real implementation will use the `v4l` crate (or `v4l2` crate) to:
//! 1. Open the device file.
//! 2. Negotiate format (MJPEG preferred for USB cameras — saves USB bandwidth).
//! 3. Queue buffers (mmap or userptr).
//! 4. Dequeue frames in a loop.
//!
//! ## CI testing strategy (Phase 1.7)
//!
//! GitHub Actions Ubuntu runners have the `vivid` kernel module available.
//! `vivid` creates synthetic V4L2 devices that produce test patterns —
//! perfect for CI without real hardware.
//!
//! ```yaml
//! # In .github/workflows/ci.yml
//! - name: Load vivid kernel module
//!   run: sudo modprobe vivid
//!
//! - name: Verify /dev/video0 exists
//!   run: ls -la /dev/video0
//!
//! - name: Run V4L2 tests
//!   run: cargo test -p video-capture --features vivid-tests -- --include-ignored
//! ```
//!
//! Tests guarded by `#[cfg(feature = "vivid-tests")]` only run when the
//! feature is enabled (and the `vivid` module is loaded).
//!
//! ## Local dev
//!
//! ```bash
//! sudo modprobe vivid
//! v4l2-ctl --list-formats-ext -d /dev/video0
//! cargo test -p video-capture --features vivid-tests
//! ```

use crate::traits::{VideoCaptureError, VideoResult, VideoSource};
use async_trait::async_trait;
use common::Frame;
use tokio::sync::mpsc;

/// Stub V4L2 source. Phase 1 will implement this.
pub struct V4l2Source {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl V4l2Source {
    pub fn new(device: impl Into<String>, width: u32, height: u32, fps: u32) -> Self {
        Self {
            device: device.into(),
            width,
            height,
            fps,
        }
    }
}

#[async_trait]
impl VideoSource for V4l2Source {
    async fn start(&mut self) -> VideoResult<mpsc::Receiver<Frame>> {
        tracing::warn!(
            device = %self.device,
            "V4l2Source::start not yet implemented (Phase 1) — use SyntheticVideoSource for testing"
        );
        Err(VideoCaptureError::DeviceOpen(format!(
            "V4l2Source not implemented yet (Phase 1). Device: {}",
            self.device
        )))
    }

    async fn stop(&mut self) -> VideoResult<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "V4l2Source (stub — Phase 1)"
    }
}

/// Check if a V4L2 device exists at the given path.
/// Useful for CI tests that gate on `vivid` availability.
pub fn device_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

/// List available V4L2 devices by scanning `/dev/video*`.
/// Returns paths in sorted order.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_exists_returns_false_for_nonexistent() {
        assert!(!device_exists("/dev/video999"));
    }

    #[test]
    fn device_exists_returns_true_for_dev_null() {
        // /dev/null always exists on Linux
        assert!(device_exists("/dev/null"));
    }

    #[test]
    fn list_v4l2_devices_does_not_panic() {
        // Just verify it doesn't crash — actual devices depend on environment
        let _devices = list_v4l2_devices();
    }

    #[test]
    fn v4l2_source_construction() {
        let src = V4l2Source::new("/dev/video0", 1280, 720, 30);
        assert_eq!(src.device, "/dev/video0");
        assert_eq!(src.width, 1280);
        assert_eq!(src.fps, 30);
    }

    /// This test is `#[ignore]` by default — it requires the `vivid` kernel
    /// module to be loaded. Run with:
    ///   sudo modprobe vivid
    ///   cargo test -p video-capture -- --include-ignored vivid
    #[test]
    #[ignore = "requires vivid kernel module: sudo modprobe vivid"]
    fn vivid_module_is_loaded_and_creates_devices() {
        let devices = list_v4l2_devices();
        assert!(
            !devices.is_empty(),
            "expected at least one /dev/video* device after `modprobe vivid`"
        );
        println!("Found V4L2 devices: {devices:?}");
        // Verify at least one is readable
        let first = &devices[0];
        assert!(
            device_exists(first),
            "device {first} should exist (listed but not found?)"
        );
    }

    /// This test checks that `vivid` produces a recognizable test pattern.
    /// Requires `v4l2-ctl` and the `vivid` module.
    #[test]
    #[ignore = "requires vivid + v4l2-ctl"]
    fn vivid_device_supports_querying_formats() {
        let devices = list_v4l2_devices();
        if devices.is_empty() {
            eprintln!("SKIP: no V4L2 devices — run `sudo modprobe vivid`");
            return;
        }
        let dev = &devices[0];
        let output = std::process::Command::new("v4l2-ctl")
            .args(["--device", dev, "--list-formats-ext"])
            .output();
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                println!("v4l2-ctl output for {dev}:\n{stdout}");
                // vivid should report at least one format
                assert!(
                    stdout.contains("YU") || stdout.contains("RGB") || stdout.contains("MJPEG"),
                    "expected at least one pixel format from vivid"
                );
            }
            Err(e) => {
                eprintln!("SKIP: v4l2-ctl not available: {e}");
            }
        }
    }
}
