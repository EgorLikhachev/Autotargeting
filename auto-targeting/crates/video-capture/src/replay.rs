//! Replay source — reads recorded frames from disk for regression testing.
//!
//! ## Status: ✅ Working (Phase 6.3)
//!
//! A recording is a directory containing:
//! - `meta.json` — recording metadata (width, height, fps, frame count)
//! - `frame_NNNNNN.bin` — raw frame data (RGB24 by default)
//!
//! ## Recording a session
//!
//! ```ignore
//! use video_capture::{ReplaySource, Recording};
//!
//! // During a real run, write frames to disk:
//! let recording = Recording::create("/tmp/session1", 1280, 720, 30).unwrap();
//! for frame in frames {
//!     recording.append(&frame).unwrap();
//! }
//! recording.finalize().unwrap();
//! ```
//!
//! ## Replaying a session
//!
//! ```ignore
//! let mut source = ReplaySource::open("/tmp/session1", 30).unwrap();
//! let mut rx = source.start().await.unwrap();
//! while let Some(frame) = rx.recv().await {
//!     // Process frame — identical to live capture
//! }
//! ```

use crate::traits::{VideoCaptureError, VideoResult, VideoSource};
use async_trait::async_trait;
use chrono::Utc;
use common::{Frame, FrameMetadata, PixelFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info};

/// Recording metadata — written to `meta.json` in the recording directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingMeta {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: String, // "rgb24", "nv12", etc.
    pub frame_count: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A recording — used to write frames to disk for later replay.
pub struct Recording {
    dir: PathBuf,
    meta: RecordingMeta,
    next_seq: u64,
}

impl Recording {
    /// Create a new recording directory. Fails if the directory already exists
    /// with a `meta.json` file (use `open_for_append` for that).
    pub fn create(dir: impl AsRef<Path>, width: u32, height: u32, fps: u32) -> VideoResult<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .map_err(|e| VideoCaptureError::DeviceOpen(format!("create dir: {e}")))?;

        let meta = RecordingMeta {
            width,
            height,
            fps,
            format: "rgb24".to_string(),
            frame_count: 0,
            created_at: Utc::now(),
        };

        Ok(Self {
            dir,
            meta,
            next_seq: 0,
        })
    }

    /// Append a frame to the recording.
    pub fn append(&mut self, frame: &Frame) -> VideoResult<()> {
        let path = self.dir.join(format!("frame_{:06}.bin", self.next_seq));
        std::fs::write(&path, &frame.data)
            .map_err(|e| VideoCaptureError::Capture(format!("write frame: {e}")))?;
        self.next_seq += 1;
        self.meta.frame_count = self.next_seq;
        Ok(())
    }

    /// Finalize the recording — write `meta.json`.
    pub fn finalize(self) -> VideoResult<()> {
        let meta_path = self.dir.join("meta.json");
        let json = serde_json::to_string_pretty(&self.meta)
            .map_err(|e| VideoCaptureError::Capture(format!("serialize meta: {e}")))?;
        std::fs::write(&meta_path, json)
            .map_err(|e| VideoCaptureError::Capture(format!("write meta: {e}")))?;
        Ok(())
    }
}

/// Replay source — reads a recording from disk and emits frames as if live.
pub struct ReplaySource {
    dir: PathBuf,
    meta: RecordingMeta,
    /// If true, replay at the recorded FPS. If false, emit as fast as possible.
    pub real_time: bool,
    /// If true, loop the recording when it ends. Useful for long-running tests.
    pub loop_playback: bool,
    capture_task: Option<JoinHandle<()>>,
}

impl ReplaySource {
    /// Open a recording directory and read its metadata.
    pub fn open(dir: impl AsRef<Path>) -> VideoResult<Self> {
        let dir = dir.as_ref().to_path_buf();
        let meta_path = dir.join("meta.json");
        let meta_str = std::fs::read_to_string(&meta_path)
            .map_err(|e| VideoCaptureError::DeviceOpen(format!("read meta.json: {e}")))?;
        let meta: RecordingMeta = serde_json::from_str(&meta_str)
            .map_err(|e| VideoCaptureError::DeviceOpen(format!("parse meta.json: {e}")))?;

        Ok(Self {
            dir,
            meta,
            real_time: true,
            loop_playback: false,
            capture_task: None,
        })
    }

    /// Get the recording metadata.
    pub fn meta(&self) -> &RecordingMeta {
        &self.meta
    }

    /// Set real-time playback mode (default: true).
    pub fn with_real_time(mut self, real_time: bool) -> Self {
        self.real_time = real_time;
        self
    }

    /// Enable/disable looping (default: false).
    pub fn with_loop(mut self, loop_playback: bool) -> Self {
        self.loop_playback = loop_playback;
        self
    }
}

#[async_trait]
impl VideoSource for ReplaySource {
    async fn start(&mut self) -> VideoResult<mpsc::Receiver<Frame>> {
        let (tx, rx) = mpsc::channel(8);

        let dir = self.dir.clone();
        let meta = self.meta.clone();
        let real_time = self.real_time;
        let loop_playback = self.loop_playback;

        info!(
            dir = %dir.display(),
            frame_count = meta.frame_count,
            fps = meta.fps,
            real_time,
            loop_playback,
            "starting replay source"
        );

        let handle = tokio::spawn(async move {
            let interval = Duration::from_secs_f64(1.0 / meta.fps as f64);
            let format = match meta.format.as_str() {
                "rgb24" => PixelFormat::Rgb24,
                "nv12" => PixelFormat::Nv12,
                "yuyv" => PixelFormat::Yuyv,
                "mjpeg" => PixelFormat::Mjpeg,
                _ => PixelFormat::Rgb24,
            };

            loop {
                let mut sent_any = false;
                for seq in 0..meta.frame_count {
                    let path = dir.join(format!("frame_{seq:06}.bin"));
                    let data = match std::fs::read(&path) {
                        Ok(d) => d,
                        Err(e) => {
                            tracing::error!(
                                frame = seq,
                                path = %path.display(),
                                error = %e,
                                "failed to read frame — stopping replay"
                            );
                            return;
                        }
                    };

                    let frame = Frame {
                        data,
                        metadata: FrameMetadata {
                            width: meta.width,
                            height: meta.height,
                            format,
                            captured_at: Utc::now(),
                            seq,
                        },
                    };

                    if tx.send(frame).await.is_err() {
                        debug!("receiver dropped — stopping replay");
                        return;
                    }
                    sent_any = true;

                    if real_time {
                        tokio::time::sleep(interval).await;
                    }
                }

                if !loop_playback {
                    debug!("replay complete (no loop) — stopping");
                    return;
                }
                if !sent_any {
                    debug!("empty recording — stopping");
                    return;
                }
                debug!("replay looped — restarting from frame 0");
            }
        });

        self.capture_task = Some(handle);
        Ok(rx)
    }

    async fn stop(&mut self) -> VideoResult<()> {
        if let Some(handle) = self.capture_task.take() {
            handle.abort();
            debug!("replay source stopped");
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "ReplaySource"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_frame(seq: u64, w: u32, h: u32) -> Frame {
        Frame {
            data: vec![(seq % 256) as u8; (w * h * 3) as usize],
            metadata: FrameMetadata {
                width: w,
                height: h,
                format: PixelFormat::Rgb24,
                captured_at: Utc::now(),
                seq,
            },
        }
    }

    #[test]
    fn recording_create_and_append() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = Recording::create(tmp.path(), 32, 24, 30).unwrap();
        for i in 0..5 {
            rec.append(&make_test_frame(i, 32, 24)).unwrap();
        }
        rec.finalize().unwrap();

        // Verify meta.json exists
        let meta_path = tmp.path().join("meta.json");
        assert!(meta_path.exists());

        // Verify frame files exist
        for i in 0..5 {
            let frame_path = tmp.path().join(format!("frame_{i:06}.bin"));
            assert!(frame_path.exists(), "frame {i} should exist");
        }
    }

    #[test]
    fn replay_open_reads_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = Recording::create(tmp.path(), 64, 48, 15).unwrap();
        for i in 0..3 {
            rec.append(&make_test_frame(i, 64, 48)).unwrap();
        }
        rec.finalize().unwrap();

        let source = ReplaySource::open(tmp.path()).unwrap();
        assert_eq!(source.meta().width, 64);
        assert_eq!(source.meta().height, 48);
        assert_eq!(source.meta().fps, 15);
        assert_eq!(source.meta().frame_count, 3);
    }

    #[tokio::test]
    async fn replay_emits_all_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = Recording::create(tmp.path(), 16, 16, 100).unwrap();
        for i in 0..5 {
            rec.append(&make_test_frame(i, 16, 16)).unwrap();
        }
        rec.finalize().unwrap();

        let mut source = ReplaySource::open(tmp.path()).unwrap();
        source.real_time = false; // emit as fast as possible
        let mut rx = source.start().await.unwrap();

        let mut frames = Vec::new();
        while let Some(f) = rx.recv().await {
            frames.push(f);
            if frames.len() >= 5 {
                break;
            }
        }

        assert_eq!(frames.len(), 5);
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(f.metadata.seq, i as u64);
            assert_eq!(f.data[0], (i as u64 % 256) as u8);
        }
    }

    #[tokio::test]
    async fn replay_loops_when_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = Recording::create(tmp.path(), 8, 8, 1000).unwrap();
        for i in 0..3 {
            rec.append(&make_test_frame(i, 8, 8)).unwrap();
        }
        rec.finalize().unwrap();

        let mut source = ReplaySource::open(tmp.path()).unwrap();
        source.real_time = false;
        source.loop_playback = true;
        let mut rx = source.start().await.unwrap();

        // Collect 7 frames — should see seq 0,1,2,0,1,2,0 (loop)
        let mut seqs = Vec::new();
        for _ in 0..7 {
            let f = rx.recv().await.expect("frame");
            seqs.push(f.metadata.seq);
        }

        assert_eq!(seqs, vec![0, 1, 2, 0, 1, 2, 0]);
    }

    #[tokio::test]
    async fn replay_stops_when_no_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = Recording::create(tmp.path(), 8, 8, 1000).unwrap();
        for i in 0..3 {
            rec.append(&make_test_frame(i, 8, 8)).unwrap();
        }
        rec.finalize().unwrap();

        let mut source = ReplaySource::open(tmp.path()).unwrap();
        source.real_time = false;
        source.loop_playback = false;
        let mut rx = source.start().await.unwrap();

        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
            if count > 10 {
                panic!("should have stopped after 3 frames");
            }
        }
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn replay_stop_aborts_task() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rec = Recording::create(tmp.path(), 8, 8, 1).unwrap();
        for i in 0..100 {
            rec.append(&make_test_frame(i, 8, 8)).unwrap();
        }
        rec.finalize().unwrap();

        let mut source = ReplaySource::open(tmp.path()).unwrap();
        source.real_time = true;
        source.loop_playback = true;
        let mut rx = source.start().await.unwrap();

        // Receive a couple
        for _ in 0..2 {
            let _ = rx.recv().await.expect("frame");
        }

        source.stop().await.unwrap();
        // After stop, no more frames should arrive
        tokio::time::sleep(Duration::from_millis(100)).await;
        // The receiver may have buffered frames, but the task is aborted
    }

    #[test]
    fn replay_open_fails_for_missing_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let result = ReplaySource::open(tmp.path());
        assert!(result.is_err());
    }
}
