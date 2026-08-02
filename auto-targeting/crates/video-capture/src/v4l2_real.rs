//! V4L2 video source — REAL implementation using the `v4l` crate.
//!
//! This module is compiled when the `v4l2` feature is enabled.
//! It provides the V4l2Source struct, device discovery, and capture loop.

#![cfg(feature = "v4l2")]

use crate::traits::{VideoCaptureError, VideoResult, VideoSource};
use async_trait::async_trait;
use chrono::Utc;
use common::{Frame, FrameMetadata, PixelFormat};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use v4l::buffer::Type;
use v4l::io::mmap::Stream as MMapStream;
use v4l::io::Stream as StreamTrait;
use v4l::video::Capture;

/// V4L2 video source — opens a real or vivid device.
pub struct V4l2Source {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: PixelFormat,
    pub queue_depth: usize,
    stop_flag: Option<Arc<AtomicBool>>,
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
            stop_flag: None,
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
            other => {
                warn!(format = other, "unknown pixel format, defaulting to YUYV");
                PixelFormat::Yuyv
            }
        };
        Self {
            device: cfg.device.clone(),
            width: cfg.width,
            height: cfg.height,
            fps: cfg.fps,
            format,
            queue_depth: cfg.queue_depth,
            stop_flag: None,
        }
    }

    pub fn probe(&self) -> VideoResult<DeviceProbe> {
        info!(
            device = %self.device,
            width = self.width,
            height = self.height,
            fps = self.fps,
            format = ?self.format,
            "probing V4L2 device"
        );

        let device = v4l::Device::with_path(&self.device)
            .map_err(|e| VideoCaptureError::DeviceOpen(format!("open {}: {e}", self.device)))?;

        let formats = device
            .enum_formats()
            .map_err(|e| VideoCaptureError::DeviceConfig(format!("enum_formats: {e}")))?;

        let v4l_fourcc = pixel_format_to_fourcc(self.format);
        let format_supported = formats.iter().any(|f| f.fourcc == v4l_fourcc);

        let supported_formats: Vec<String> = formats
            .iter()
            .map(|f| fourcc_to_string(f.fourcc))
            .collect();

        if !format_supported {
            return Err(VideoCaptureError::DeviceConfig(format!(
                "device {} does not support {:?} format. Supported: {:?}",
                self.device, self.format, supported_formats
            )));
        }

        Ok(DeviceProbe {
            device_path: self.device.clone(),
            supported_formats,
            requested_format: self.format,
            requested_width: self.width,
            requested_height: self.height,
            requested_fps: self.fps,
        })
    }
}

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
        writeln!(f, "Device: {}", self.device_path)?;
        writeln!(
            f,
            "Requested: {}x{} @ {} fps {:?}",
            self.requested_width, self.requested_height, self.requested_fps, self.requested_format
        )?;
        writeln!(f, "Supported formats: {:?}", self.supported_formats)?;
        Ok(())
    }
}

#[async_trait]
impl VideoSource for V4l2Source {
    async fn start(&mut self) -> VideoResult<mpsc::Receiver<Frame>> {
        let _probe = self.probe()?;

        let (tx, rx) = mpsc::channel(self.queue_depth.max(1));

        let device_path = self.device.clone();
        let width = self.width;
        let height = self.height;
        let fps = self.fps;
        let format = self.format;
        let fourcc = pixel_format_to_fourcc(format);

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = Arc::clone(&stop_flag);

        info!(device = %device_path, "starting V4L2 capture thread");

        std::thread::spawn(move || {
            let result = run_capture_loop(
                &device_path,
                width,
                height,
                fps,
                fourcc,
                format,
                tx,
                stop_flag_clone,
            );
            if let Err(e) = result {
                error!(error = %e, "V4L2 capture thread exited with error");
            } else {
                info!("V4L2 capture thread exited cleanly");
            }
        });

        self.stop_flag = Some(stop_flag);
        Ok(rx)
    }

    async fn stop(&mut self) -> VideoResult<()> {
        if let Some(flag) = self.stop_flag.take() {
            flag.store(true, Ordering::SeqCst);
            debug!("V4L2 stop flag set");
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "V4l2Source"
    }
}

fn run_capture_loop(
    device_path: &str,
    width: u32,
    height: u32,
    _fps: u32,
    fourcc: v4l::format::FourCC,
    format: PixelFormat,
    tx: mpsc::Sender<Frame>,
    stop_flag: Arc<AtomicBool>,
) -> VideoResult<()> {
    let mut device = v4l::Device::with_path(device_path)
        .map_err(|e| VideoCaptureError::DeviceOpen(format!("open {device_path}: {e}")))?;

    let fmt = v4l::Format::new(width, height, fourcc);
    device
        .set_format(&fmt)
        .map_err(|e| VideoCaptureError::DeviceConfig(format!("set_format: {e}")))?;

    let negotiated = device
        .format()
        .map_err(|e| VideoCaptureError::DeviceConfig(format!("get_format: {e}")))?;
    info!(
        width = negotiated.width,
        height = negotiated.height,
        fourcc = ?negotiated.fourcc,
        "V4L2 format negotiated"
    );

    let buffer_type = Type::VideoCapture;
    let mut buffers = MMapStream::new(&mut device, buffer_type)
        .map_err(|e| VideoCaptureError::DeviceConfig(format!("MMapStream::new: {e}")))?;

    buffers
        .start()
        .map_err(|e| VideoCaptureError::DeviceConfig(format!("stream start: {e}")))?;

    info!("V4L2 streaming started");

    let mut seq: u64 = 0;
    let capture_start = Instant::now();

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            info!("V4L2 capture: stop flag detected, exiting loop");
            break;
        }

        let (data, _meta) = match buffers.next() {
            Ok((data, meta)) => (data, meta),
            Err(e) => {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                warn!(error = %e, "V4L2 dequeue error — retrying");
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
        };

        let frame_data = data.to_vec();

        let frame = Frame {
            data: frame_data,
            metadata: FrameMetadata {
                width: negotiated.width,
                height: negotiated.height,
                format,
                captured_at: Utc::now(),
                seq,
            },
        };

        if tx.blocking_send(frame).is_err() {
            debug!("V4L2: receiver dropped, stopping capture");
            break;
        }

        seq += 1;
    }

    let _ = buffers.stop();
    info!(
        frames_captured = seq,
        uptime_secs = capture_start.elapsed().as_secs_f64(),
        "V4L2 capture thread shutting down"
    );

    Ok(())
}

fn pixel_format_to_fourcc(fmt: PixelFormat) -> v4l::format::FourCC {
    match fmt {
        PixelFormat::Yuyv => v4l::format::FourCC::new(b"YUYV"),
        PixelFormat::Mjpeg => v4l::format::FourCC::new(b"MJPG"),
        PixelFormat::Rgb24 => v4l::format::FourCC::new(b"RGB3"),
        PixelFormat::Nv12 => v4l::format::FourCC::new(b"NV12"),
    }
}

fn fourcc_to_string(fourcc: v4l::format::FourCC) -> String {
    String::from_utf8_lossy(&fourcc.repr)
        .trim_end_matches('\0')
        .to_string()
}

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
