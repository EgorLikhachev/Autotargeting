//! Core domain types for the Auto-Targeting System.
//!
//! These types are intentionally framework-agnostic — no MAVLink, no V4L2
//! types here. They cross module boundaries through trait boundaries.

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Monotonic timestamp — uses process-monotonic `Instant` for measuring
/// latencies, wraps an `Instant` so we can't accidentally compare across
/// processes.
#[derive(Debug, Clone, Copy)]
pub struct Timestamp {
    inner: Instant,
}

impl Timestamp {
    pub fn now() -> Self {
        Self {
            inner: Instant::now(),
        }
    }

    /// Elapsed since this timestamp, in microseconds. Useful for latency logs.
    pub fn elapsed_us(&self) -> u64 {
        self.inner.elapsed().as_micros() as u64
    }

    /// Elapsed since this timestamp, in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.inner.elapsed().as_millis() as u64
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::now()
    }
}

/// A captured video frame.
///
/// Holds the pixel data and metadata needed for downstream consumers
/// (inference, tracking). The data is owned to keep lifetimes simple;
/// in Phase 1's optimization we will switch to shared memory (dmabuf).
#[derive(Debug, Clone)]
pub struct Frame {
    /// Pixel data, format defined by `metadata.format`.
    pub data: Vec<u8>,
    pub metadata: FrameMetadata,
}

/// Metadata for a frame — measured at capture time.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FrameMetadata {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Capture timestamp (when V4L2 handed us the buffer).
    pub captured_at: chrono::DateTime<chrono::Utc>,
    /// Sequence number — increments per capture, used to detect drops.
    pub seq: u64,
}

/// Supported pixel formats. Kept minimal — expand as Phase 1 lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    /// YUV 4:2:0 semi-planar (NV12) — preferred for NPU input.
    Nv12,
    /// YUV 4:2:2 packed — common UVC output.
    Yuyv,
    /// 8-bit RGB.
    Rgb24,
    /// JPEG-encoded bytes (camera did MJPEG, we haven't decoded yet).
    Mjpeg,
}

/// A bounding box in image coordinates.
///
/// Coordinates are in pixels, origin at top-left. Width/height are > 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl BoundingBox {
    /// Center of the box in (x, y) pixel coordinates.
    pub fn center(&self) -> (f32, f32) {
        (
            self.x as f32 + self.width as f32 / 2.0,
            self.y as f32 + self.height as f32 / 2.0,
        )
    }

    /// Intersection-over-union with another box. Returns 0.0 if no overlap.
    pub fn iou(&self, other: &Self) -> f32 {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);

        if x2 <= x1 || y2 <= y1 {
            return 0.0;
        }
        let intersection = (x2 - x1) as f32 * (y2 - y1) as f32;
        let union = (self.width * self.height + other.width * other.height) as f32 - intersection;
        if union <= 0.0 {
            0.0
        } else {
            intersection / union
        }
    }
}

/// A single detection produced by the CV inference module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    pub bbox: BoundingBox,
    /// Class label (e.g. "person", "vehicle"). Free-form string for flexibility.
    pub class: String,
    /// Class integer id (model-dependent).
    pub class_id: u32,
    /// Confidence in [0.0, 1.0].
    pub confidence: f32,
    /// Frame sequence number this detection came from.
    pub frame_seq: u64,
    /// Wall-clock time of the detection.
    pub detected_at: chrono::DateTime<chrono::Utc>,
}

/// Identifier for a tracked target. Stable across frames while tracking holds.
pub type TargetId = u64;

/// State of a tracked target — maintained by `target-tracker` between detections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetState {
    pub id: TargetId,
    pub bbox: BoundingBox,
    /// Estimated velocity in pixels per second (vx, vy).
    pub velocity: (f32, f32),
    /// Confidence of the latest detection contributing to this state.
    pub confidence: f32,
    /// Wall-clock time of last detection update.
    pub last_seen: chrono::DateTime<chrono::Utc>,
    /// Number of consecutive frames since last detection.
    pub missed_frames: u32,
}

impl TargetState {
    /// Returns true if this target has been missing for too long and should
    /// transition to LOST. Caller passes the threshold.
    pub fn is_lost(&self, max_age_ms: u64) -> bool {
        let age = (chrono::Utc::now() - self.last_seen)
            .num_milliseconds()
            .max(0) as u64;
        age > max_age_ms
    }
}

/// Region of Interest command — what the camera/gimbal should look at.
///
/// Sent to FC via `MAV_CMD_DO_SET_ROI`. None variant means "clear ROI".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RoiTarget {
    /// Point at a global GPS location.
    GlobalLatLng { lat: f64, lon: f64, alt: f32 },
    /// Point at a local NED offset from home position.
    LocalNed { north: f32, east: f32, down: f32 },
    /// Clear ROI — return to default attitude.
    None,
}

/// Target position in local NED frame (North-East-Down), in meters.
///
/// Sent to FC via `SET_POSITION_TARGET_LOCAL_NED` at 10 Hz.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct PositionTargetNED {
    pub north: f32,
    pub east: f32,
    pub down: f32,
    /// Desired yaw in radians (0 = North, positive = clockwise).
    pub yaw: f32,
}

/// Drone attitude (orientation). Units: radians.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Attitude {
    pub roll: f32,
    pub pitch: f32,
    pub yaw: f32,
    /// Angular rates, rad/s.
    pub roll_rate: f32,
    pub pitch_rate: f32,
    pub yaw_rate: f32,
}

/// Global GPS position.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct GlobalPosition {
    pub lat: f64,
    pub lon: f64,
    pub alt_msl: f32,
    pub alt_agl: f32,
}

/// Status of the FC heartbeat — last time we heard from the FC.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatStatus {
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub armed: bool,
    pub mode: FlightMode,
}

impl HeartbeatStatus {
    /// Returns true if we haven't heard from FC for longer than `timeout_ms`.
    pub fn is_stale(&self, timeout_ms: u64) -> bool {
        let age = (chrono::Utc::now() - self.last_heartbeat)
            .num_milliseconds()
            .max(0) as u64;
        age > timeout_ms
    }
}

/// ArduPilot flight modes (subset relevant to auto-targeting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FlightMode {
    #[default]
    Unknown,
    Manual,
    Stabilize,
    AltHold,
    Loiter,
    Guided,
    Rtl,
    Auto,
}

impl std::fmt::Display for FlightMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Top-level state of the commander state machine.
///
/// See `docs/ARCHITECTURE.md` §1.4 for the transition diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SystemState {
    #[default]
    Idle,
    Armed,
    Scanning,
    TargetSelected,
    Tracking,
    TrackingDegraded,
    Lost,
    Rth,
    Abort,
}

impl SystemState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Armed => "ARMED",
            Self::Scanning => "SCANNING",
            Self::TargetSelected => "TARGET_SELECTED",
            Self::Tracking => "TRACKING",
            Self::TrackingDegraded => "TRACKING_DEGRADED",
            Self::Lost => "LOST",
            Self::Rth => "RTH",
            Self::Abort => "ABORT",
        }
    }
}

impl std::fmt::Display for SystemState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_box_center() {
        let bbox = BoundingBox {
            x: 100,
            y: 200,
            width: 60,
            height: 40,
        };
        let (cx, cy) = bbox.center();
        assert_eq!(cx, 130.0);
        assert_eq!(cy, 220.0);
    }

    #[test]
    fn bounding_box_iou_identical() {
        let a = BoundingBox {
            x: 10,
            y: 10,
            width: 50,
            height: 50,
        };
        assert!((a.iou(&a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bounding_box_iou_disjoint() {
        let a = BoundingBox {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        let b = BoundingBox {
            x: 100,
            y: 100,
            width: 50,
            height: 50,
        };
        assert_eq!(a.iou(&b), 0.0);
    }

    #[test]
    fn bounding_box_iou_partial() {
        let a = BoundingBox {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let b = BoundingBox {
            x: 50,
            y: 50,
            width: 100,
            height: 100,
        };
        // Intersection: 50x50 = 2500; Union: 10000 + 10000 - 2500 = 17500
        // IoU = 2500 / 17500 ≈ 0.1428
        let iou = a.iou(&b);
        assert!(iou > 0.14 && iou < 0.15, "got {iou}");
    }

    #[test]
    fn heartbeat_staleness() {
        let hb = HeartbeatStatus {
            last_heartbeat: chrono::Utc::now(),
            armed: false,
            mode: FlightMode::Guided,
        };
        assert!(!hb.is_stale(1000));

        let old = HeartbeatStatus {
            last_heartbeat: chrono::Utc::now() - chrono::Duration::seconds(5),
            armed: false,
            mode: FlightMode::Guided,
        };
        assert!(old.is_stale(1000));
    }
}
