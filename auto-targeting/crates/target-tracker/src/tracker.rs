//! Target tracker — owns the active target state.
//!
//! Status: 🚧 Phase 0 scaffolding only — basic skeleton.
//! Phase 3 will implement:
//! - Multi-target tracking (SORT-style assignment).
//! - Lock acquisition logic (confirmation frames).
//! - Occlusion handling (predict-only mode).
//! - Target handoff (operator selects a detection → tracker acquires).

use crate::kalman::KalmanFilter2D;
use chrono::Utc;
use common::{BoundingBox, Detection, TargetId, TargetState};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("no active target")]
    NoActiveTarget,

    #[error("target not found: {0}")]
    TargetNotFound(TargetId),

    #[error("lock not acquired yet")]
    LockNotAcquired,

    #[error("tracker error: {0}")]
    Internal(String),
}

pub type TrackerResult<T> = std::result::Result<T, TrackerError>;

/// The target tracker — owns Kalman filters for all active tracks.
///
/// Phase 0: minimal implementation, single-target tracking.
/// Phase 3: upgrade to multi-target with SORT assignment.
pub struct TargetTracker {
    /// Maximum age (ms) of a detection before target is declared LOST.
    max_target_age_ms: u64,
    /// Maximum missed frames before LOST.
    max_missed_frames: u32,
    /// Number of consecutive detections required to confirm a lock.
    lock_confirmation_frames: u32,
    /// IoU threshold for matching a new detection to an existing track.
    match_iou_threshold: f32,

    /// The currently tracked target (only one in Phase 0).
    active: Option<ActiveTrack>,
}

struct ActiveTrack {
    id: TargetId,
    kalman: KalmanFilter2D,
    last_bbox: BoundingBox,
    confidence: f32,
    last_seen: chrono::DateTime<chrono::Utc>,
    missed_frames: u32,
    confirmation_count: u32,
}

impl TargetTracker {
    pub fn new(
        max_target_age_ms: u64,
        max_missed_frames: u32,
        lock_confirmation_frames: u32,
        match_iou_threshold: f32,
    ) -> Self {
        Self {
            max_target_age_ms,
            max_missed_frames,
            lock_confirmation_frames,
            match_iou_threshold,
            active: None,
        }
    }

    pub fn from_common(cfg: &common::TrackerConfig) -> Self {
        Self::new(
            cfg.max_target_age_ms,
            cfg.max_missed_frames,
            cfg.lock_confirmation_frames,
            cfg.match_iou_threshold,
        )
    }

    /// Operator selected a detection — start tracking it.
    /// Returns the assigned TargetId.
    pub fn acquire(&mut self, detection: &Detection) -> TargetId {
        let (cx, cy) = detection.bbox.center();
        let id = next_target_id();
        let kalman = KalmanFilter2D::new(cx, cy);
        self.active = Some(ActiveTrack {
            id,
            kalman,
            last_bbox: detection.bbox,
            confidence: detection.confidence,
            last_seen: Utc::now(),
            missed_frames: 0,
            confirmation_count: 1,
        });
        tracing::info!(target_id = id, "acquired target");
        id
    }

    /// Process new detections. Updates the active track if a matching detection
    /// is found; otherwise increments missed_frames.
    pub fn update(&mut self, detections: &[Detection]) {
        let Some(track) = self.active.as_mut() else {
            return;
        };

        // Find best matching detection by IoU
        let best = detections
            .iter()
            .map(|d| (d, d.bbox.iou(&track.last_bbox)))
            .filter(|(_, iou)| *iou > self.match_iou_threshold)
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((det, _iou)) = best {
            // Update Kalman with new position
            track.kalman.update_from_bbox(&det.bbox, 1.0 / 30.0);
            track.last_bbox = det.bbox;
            track.confidence = det.confidence;
            track.last_seen = Utc::now();
            track.missed_frames = 0;
            if track.confirmation_count < self.lock_confirmation_frames {
                track.confirmation_count += 1;
            }
        } else {
            // No matching detection — predict only
            track.kalman.predict(1.0 / 30.0); // assume 30 FPS
            track.missed_frames += 1;
            // Update bbox center from Kalman prediction (size stays the same)
            let (px, py) = track.kalman.position();
            let cx = px.max(0.0) as u32;
            let cy = py.max(0.0) as u32;
            let half_w = track.last_bbox.width / 2;
            let half_h = track.last_bbox.height / 2;
            track.last_bbox.x = cx.saturating_sub(half_w);
            track.last_bbox.y = cy.saturating_sub(half_h);
        }
    }

    /// Get the current state of the active target, if any.
    pub fn active_target(&self) -> Option<TargetState> {
        self.active.as_ref().map(|t| TargetState {
            id: t.id,
            bbox: t.last_bbox,
            velocity: t.kalman.velocity(),
            confidence: t.confidence,
            last_seen: t.last_seen,
            missed_frames: t.missed_frames,
        })
    }

    /// Returns true if the active target has been confirmed (lock acquired).
    pub fn is_locked(&self) -> bool {
        self.active
            .as_ref()
            .map(|t| t.confirmation_count >= self.lock_confirmation_frames)
            .unwrap_or(false)
    }

    /// Returns true if the active target is lost (missed too many frames or
    /// too much time since last detection).
    pub fn is_lost(&self) -> bool {
        self.active
            .as_ref()
            .map(|t| t.missed_frames > self.max_missed_frames || t.is_lost(self.max_target_age_ms))
            .unwrap_or(false)
    }

    /// Clear the active target (e.g. when transitioning to LOST or RTH).
    pub fn clear(&mut self) {
        if let Some(t) = self.active.take() {
            tracing::info!(target_id = t.id, "cleared active target");
        }
    }
}

impl ActiveTrack {
    fn is_lost(&self, max_age_ms: u64) -> bool {
        let age = (Utc::now() - self.last_seen).num_milliseconds().max(0) as u64;
        age > max_age_ms
    }
}

fn next_target_id() -> TargetId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common::BoundingBox;

    fn det(x: u32, y: u32, w: u32, h: u32, conf: f32) -> Detection {
        Detection {
            bbox: BoundingBox {
                x,
                y,
                width: w,
                height: h,
            },
            class: "person".to_string(),
            class_id: 0,
            confidence: conf,
            frame_seq: 1,
            detected_at: Utc::now(),
        }
    }

    #[test]
    fn acquire_creates_active_target() {
        let mut t = TargetTracker::new(2000, 60, 3, 0.3);
        let id = t.acquire(&det(100, 100, 50, 80, 0.9));
        assert!(id > 0);
        assert!(t.active_target().is_some());
        // confirmation_count = 1, lock_confirmation = 3 → not locked yet
        assert!(!t.is_locked());
    }

    #[test]
    fn lock_acquired_after_confirmation_frames() {
        let mut t = TargetTracker::new(2000, 60, 3, 0.3);
        t.acquire(&det(100, 100, 50, 80, 0.9));
        // Two more matching detections
        t.update(&[det(101, 101, 50, 80, 0.85)]);
        assert!(!t.is_locked()); // count=2, need 3
        t.update(&[det(102, 102, 50, 80, 0.85)]);
        assert!(t.is_locked()); // count=3
    }

    #[test]
    fn missed_frames_increment_when_no_detection() {
        let mut t = TargetTracker::new(2000, 60, 1, 0.3);
        t.acquire(&det(100, 100, 50, 80, 0.9));
        t.update(&[]); // no detections
        assert_eq!(t.active_target().unwrap().missed_frames, 1);
        t.update(&[]);
        assert_eq!(t.active_target().unwrap().missed_frames, 2);
    }

    #[test]
    fn is_lost_after_max_missed_frames() {
        let mut t = TargetTracker::new(2000, 5, 1, 0.3);
        t.acquire(&det(100, 100, 50, 80, 0.9));
        for _ in 0..6 {
            t.update(&[]);
        }
        assert!(t.is_lost());
    }

    #[test]
    fn clear_removes_active_target() {
        let mut t = TargetTracker::new(2000, 60, 1, 0.3);
        t.acquire(&det(100, 100, 50, 80, 0.9));
        assert!(t.active_target().is_some());
        t.clear();
        assert!(t.active_target().is_none());
    }

    #[test]
    fn update_ignores_non_matching_detections() {
        let mut t = TargetTracker::new(2000, 60, 1, 0.3);
        t.acquire(&det(100, 100, 50, 80, 0.9));
        // Disjoint detection — should not match
        t.update(&[det(800, 800, 50, 80, 0.9)]);
        assert_eq!(t.active_target().unwrap().missed_frames, 1);
    }
}
