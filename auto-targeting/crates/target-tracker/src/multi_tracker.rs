//! Multi-target tracker — manages N simultaneous tracks with Hungarian assignment.
//!
//! Extends the single-target `TargetTracker` to support multiple concurrent
//! targets. Uses the Hungarian algorithm for optimal detection-to-track
//! assignment (instead of greedy IoU matching).
//!
//! ## Algorithm
//!
//! 1. Build a cost matrix where `cost[i][j] = 1 - IoU(track_i, detection_j)`.
//! 2. Solve with the Hungarian algorithm → optimal one-to-one assignment.
//! 3. Update matched tracks with their detections.
//! 4. Increment `missed_frames` for unmatched tracks.
//! 5. Remove tracks that have been missing too long.
//! 6. Create new tracks for unmatched detections (optional — off by default).
//!
//! ## Usage
//!
//! ```ignore
//! use target_tracker::MultiTargetTracker;
//! use common::{Detection, BoundingBox};
//!
//! let mut tracker = MultiTargetTracker::new(2000, 60, 0.3);
//!
//! // Process detections
//! let dets = vec![Detection { bbox: BoundingBox { x: 100, y: 100, width: 50, height: 80 }, ..Default::default() }];
//! tracker.update(&dets);
//!
//! // Get all active tracks
//! let tracks = tracker.active_tracks();
//! ```

use crate::hungarian;
use crate::kalman::KalmanFilter2D;
use chrono::Utc;
use common::{BoundingBox, Detection, TargetId, TargetState};
use std::collections::HashMap;
use tracing::{debug, info};

/// A single tracked target.
struct Track {
    id: TargetId,
    kalman: KalmanFilter2D,
    last_bbox: BoundingBox,
    confidence: f32,
    last_seen: chrono::DateTime<chrono::Utc>,
    missed_frames: u32,
    confirmation_count: u32,
}

impl Track {
    fn new(id: TargetId, detection: &Detection) -> Self {
        let (cx, cy) = detection.bbox.center();
        let kalman = KalmanFilter2D::new(cx, cy);
        Self {
            id,
            kalman,
            last_bbox: detection.bbox,
            confidence: detection.confidence,
            last_seen: Utc::now(),
            missed_frames: 0,
            confirmation_count: 1,
        }
    }

    fn update_with_detection(&mut self, detection: &Detection) {
        self.kalman.update_from_bbox(&detection.bbox, 1.0 / 30.0);
        self.last_bbox = detection.bbox;
        self.confidence = detection.confidence;
        self.last_seen = Utc::now();
        self.missed_frames = 0;
        self.confirmation_count = self.confirmation_count.saturating_add(1);
    }

    fn predict_only(&mut self) {
        self.kalman.predict(1.0 / 30.0);
        self.missed_frames = self.missed_frames.saturating_add(1);
    }

    fn to_state(&self) -> TargetState {
        TargetState {
            id: self.id,
            bbox: self.last_bbox,
            velocity: self.kalman.velocity(),
            confidence: self.confidence,
            last_seen: self.last_seen,
            missed_frames: self.missed_frames,
        }
    }
}

/// Multi-target tracker using Hungarian algorithm for assignment.
pub struct MultiTargetTracker {
    tracks: HashMap<TargetId, Track>,
    max_target_age_ms: u64,
    max_missed_frames: u32,
    match_iou_threshold: f32,
    lock_confirmation_frames: u32,
    next_id: TargetId,
    /// If true, create new tracks for unmatched detections.
    /// Default: false (operator must explicitly create tracks).
    auto_create_tracks: bool,
    /// B3 аудита: потолок числа активных треков. Без него Hungarian O(n³)
    /// при over-detect модели (1207 треков в soak) съедал кадр.
    max_tracks: usize,
}

impl MultiTargetTracker {
    pub fn new(max_target_age_ms: u64, max_missed_frames: u32, match_iou_threshold: f32) -> Self {
        Self {
            tracks: HashMap::new(),
            max_target_age_ms,
            max_missed_frames,
            match_iou_threshold,
            lock_confirmation_frames: 3,
            next_id: 1,
            auto_create_tracks: false,
            max_tracks: 64,
        }
    }

    pub fn from_common(cfg: &common::TrackerConfig) -> Self {
        Self::new(
            cfg.max_target_age_ms,
            cfg.max_missed_frames,
            cfg.match_iou_threshold,
        )
    }

    /// Enable automatic track creation for unmatched detections.
    /// B3: потолок треков (default 64). Новые сверх лимита — reject с логом.
    #[must_use]
    pub fn with_max_tracks(mut self, max_tracks: usize) -> Self {
        self.max_tracks = max_tracks.max(1);
        self
    }

    pub fn with_auto_create(mut self, enabled: bool) -> Self {
        self.auto_create_tracks = enabled;
        self
    }

    /// Manually create a track for a detection (operator-selected target).
    /// Returns the assigned TargetId.
    pub fn create_track(&mut self, detection: &Detection) -> TargetId {
        let id = self.next_id;
        self.next_id += 1;
        let track = Track::new(id, detection);
        self.tracks.insert(id, track);
        info!(target_id = id, "created new track");
        id
    }

    /// Process new detections: assign to existing tracks via Hungarian algorithm,
    /// update matched tracks, age unmatched tracks.
    pub fn update(&mut self, detections: &[Detection]) {
        if self.tracks.is_empty() {
            // Bootstrap: кадровая цена ошибки — кап и здесь (B3: раньше
            // первый кадр с ~1000 детекций создавал 1000 треков в обход
            // гейта в основной ветке).
            if self.auto_create_tracks {
                let mut rejected = 0usize;
                for det in detections {
                    if self.tracks.len() >= self.max_tracks {
                        rejected += 1;
                        continue;
                    }
                    self.create_track(det);
                }
                if rejected > 0 {
                    debug!(
                        rejected,
                        active = self.tracks.len(),
                        limit = self.max_tracks,
                        "bootstrap: new tracks rejected (max_tracks)"
                    );
                }
            }
            return;
        }

        if detections.is_empty() {
            // No detections — age all tracks
            for track in self.tracks.values_mut() {
                track.predict_only();
            }
            self.remove_lost_tracks();
            return;
        }

        // Build cost matrix: cost[i][j] = 1 - IoU(track_i, detection_j).
        // NOTE(B3): n ограничен max_tracks — этого достаточно для бюджета
        // кадра; полноценный sparse-solver — отдельная оптимизация.
        let track_ids: Vec<TargetId> = self.tracks.keys().copied().collect();
        let n = track_ids.len();
        let m = detections.len();

        let mut cost_matrix: Vec<Vec<f32>> = Vec::with_capacity(n);
        for &tid in &track_ids {
            let track = &self.tracks[&tid];
            let mut row = Vec::with_capacity(m);
            for det in detections {
                let iou = track.last_bbox.iou(&det.bbox);
                // Cost = 1 - IoU. High IoU → low cost.
                row.push(1.0 - iou);
            }
            cost_matrix.push(row);
        }

        // Solve assignment
        let assignment = hungarian::solve(&cost_matrix);

        // Apply assignments
        let mut matched_detections: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let mut matched_tracks: std::collections::HashSet<TargetId> =
            std::collections::HashSet::new();

        for (i, &assign) in assignment.iter().enumerate() {
            if let Some(j) = assign {
                // Only accept the match if IoU is above threshold
                let iou = 1.0 - cost_matrix[i][j];
                if iou >= self.match_iou_threshold {
                    let tid = track_ids[i];
                    if let Some(track) = self.tracks.get_mut(&tid) {
                        track.update_with_detection(&detections[j]);
                        matched_detections.insert(j);
                        matched_tracks.insert(tid);
                        debug!(track_id = tid, det_idx = j, iou, "matched");
                    }
                }
            }
        }

        // Age unmatched tracks
        for &tid in &track_ids {
            if !matched_tracks.contains(&tid) {
                if let Some(track) = self.tracks.get_mut(&tid) {
                    track.predict_only();
                }
            }
        }

        // Optionally create new tracks for unmatched detections.
        // B3: потолок — сверх лимита reject (защита Hungarian O(n³) и
        // памяти от over-detect моделей).
        if self.auto_create_tracks {
            let mut rejected = 0usize;
            for (j, det) in detections.iter().enumerate() {
                if !matched_detections.contains(&j) {
                    if self.tracks.len() >= self.max_tracks {
                        rejected += 1;
                        continue;
                    }
                    self.create_track(det);
                }
            }
            if rejected > 0 {
                debug!(
                    rejected,
                    active = self.tracks.len(),
                    limit = self.max_tracks,
                    "new tracks rejected (max_tracks)"
                );
            }
        }

        // Remove lost tracks
        self.remove_lost_tracks();
    }

    /// Remove tracks that have been missing too long.
    fn remove_lost_tracks(&mut self) {
        let max_age = self.max_target_age_ms;
        let max_missed = self.max_missed_frames;
        let before = self.tracks.len();

        self.tracks.retain(|_, track| {
            let age_ms = (Utc::now() - track.last_seen).num_milliseconds().max(0) as u64;
            !(track.missed_frames > max_missed || age_ms > max_age)
        });

        let removed = before - self.tracks.len();
        if removed > 0 {
            debug!(
                removed,
                remaining = self.tracks.len(),
                "removed lost tracks"
            );
        }
    }

    /// Get all active tracks as TargetState.
    pub fn active_tracks(&self) -> Vec<TargetState> {
        self.tracks.values().map(|t| t.to_state()).collect()
    }

    /// Get a specific track by ID.
    pub fn get_track(&self, id: TargetId) -> Option<TargetState> {
        self.tracks.get(&id).map(|t| t.to_state())
    }

    /// Number of active tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Check if a track is "locked" (confirmed).
    pub fn is_locked(&self, id: TargetId) -> bool {
        self.tracks
            .get(&id)
            .map(|t| t.confirmation_count >= self.lock_confirmation_frames)
            .unwrap_or(false)
    }

    /// Remove a specific track (e.g., operator deselected it).
    pub fn remove_track(&mut self, id: TargetId) -> bool {
        self.tracks.remove(&id).is_some()
    }

    /// Clear all tracks.
    pub fn clear(&mut self) {
        let count = self.tracks.len();
        self.tracks.clear();
        if count > 0 {
            info!(cleared = count, "cleared all tracks");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_det(x: u32, y: u32, w: u32, h: u32, conf: f32) -> Detection {
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
            frame_seq: 0,
            detected_at: Utc::now(),
        }
    }

    #[test]
    fn empty_tracker_has_no_tracks() {
        let t = MultiTargetTracker::new(2000, 60, 0.3);
        assert_eq!(t.track_count(), 0);
        assert!(t.active_tracks().is_empty());
    }

    #[test]
    fn create_track_adds_one() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        let id = t.create_track(&make_det(100, 100, 50, 80, 0.9));
        assert!(id > 0);
        assert_eq!(t.track_count(), 1);
        assert!(t.get_track(id).is_some());
    }

    #[test]
    fn update_matches_existing_track() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        t.create_track(&make_det(100, 100, 50, 80, 0.9));

        // Detection at slightly different position — should match
        t.update(&[make_det(102, 101, 50, 80, 0.85)]);

        let tracks = t.active_tracks();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].missed_frames, 0);
    }

    #[test]
    fn update_does_not_match_far_detection() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        t.create_track(&make_det(100, 100, 50, 80, 0.9));

        // Detection at completely different position — should not match
        t.update(&[make_det(800, 600, 50, 80, 0.9)]);

        // The original track should have missed_frames = 1
        // (the new detection won't create a track without auto_create)
        let tracks = t.active_tracks();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].missed_frames, 1);
    }

    #[test]
    fn update_with_empty_detections_ages_tracks() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        t.create_track(&make_det(100, 100, 50, 80, 0.9));

        t.update(&[]);
        t.update(&[]);
        t.update(&[]);

        let tracks = t.active_tracks();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].missed_frames, 3);
    }

    #[test]
    fn lost_tracks_are_removed() {
        let mut t = MultiTargetTracker::new(2000, 5, 0.3);
        t.create_track(&make_det(100, 100, 50, 80, 0.9));

        // Exceed max_missed_frames (5)
        for _ in 0..7 {
            t.update(&[]);
        }

        assert_eq!(t.track_count(), 0);
    }

    #[test]
    fn multiple_tracks_matched_correctly() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        let _id1 = t.create_track(&make_det(100, 100, 50, 80, 0.9));
        let _id2 = t.create_track(&make_det(500, 400, 50, 80, 0.85));

        // Detections swapped positions slightly — Hungarian should still match correctly
        t.update(&[
            make_det(102, 101, 50, 80, 0.88),
            make_det(498, 402, 50, 80, 0.82),
        ]);

        let tracks = t.active_tracks();
        assert_eq!(tracks.len(), 2);
        // Both should have missed_frames = 0
        assert!(tracks.iter().all(|tr| tr.missed_frames == 0));
    }

    #[test]
    fn auto_create_tracks_for_unmatched_detections() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3).with_auto_create(true);

        t.update(&[
            make_det(100, 100, 50, 80, 0.9),
            make_det(500, 400, 50, 80, 0.85),
        ]);

        assert_eq!(t.track_count(), 2);
    }

    #[test]
    fn remove_track_works() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        let id = t.create_track(&make_det(100, 100, 50, 80, 0.9));

        assert!(t.remove_track(id));
        assert_eq!(t.track_count(), 0);
        assert!(!t.remove_track(id)); // already removed
    }

    #[test]
    fn clear_removes_all() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        t.create_track(&make_det(100, 100, 50, 80, 0.9));
        t.create_track(&make_det(500, 400, 50, 80, 0.85));

        t.clear();
        assert_eq!(t.track_count(), 0);
    }

    #[test]
    fn is_locked_after_confirmation_frames() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        let id = t.create_track(&make_det(100, 100, 50, 80, 0.9));

        // confirmation_count = 1, need 3
        assert!(!t.is_locked(id));

        t.update(&[make_det(101, 101, 50, 80, 0.85)]);
        assert!(!t.is_locked(id)); // 2

        t.update(&[make_det(102, 102, 50, 80, 0.85)]);
        assert!(t.is_locked(id)); // 3 → locked
    }

    #[test]
    fn three_tracks_three_detections_optimal_assignment() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        let _id1 = t.create_track(&make_det(100, 100, 50, 80, 0.9));
        let _id2 = t.create_track(&make_det(300, 300, 50, 80, 0.85));
        let _id3 = t.create_track(&make_det(500, 500, 50, 80, 0.8));

        // Detections in scrambled order
        t.update(&[
            make_det(500, 500, 50, 80, 0.78), // should match track 3
            make_det(100, 100, 50, 80, 0.88), // should match track 1
            make_det(300, 300, 50, 80, 0.83), // should match track 2
        ]);

        // All 3 tracks should still be active with no missed frames
        let tracks = t.active_tracks();
        assert_eq!(tracks.len(), 3);
        assert!(tracks.iter().all(|tr| tr.missed_frames == 0));
    }

    #[test]
    fn more_tracks_than_detections() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        t.create_track(&make_det(100, 100, 50, 80, 0.9));
        t.create_track(&make_det(300, 300, 50, 80, 0.85));
        t.create_track(&make_det(500, 500, 50, 80, 0.8));

        // Only 1 detection — 2 tracks should age
        t.update(&[make_det(100, 100, 50, 80, 0.88)]);

        let tracks = t.active_tracks();
        assert_eq!(tracks.len(), 3);
        let matched = tracks.iter().filter(|tr| tr.missed_frames == 0).count();
        let aged = tracks.iter().filter(|tr| tr.missed_frames > 0).count();
        assert_eq!(matched, 1);
        assert_eq!(aged, 2);
    }

    #[test]
    fn more_detections_than_tracks() {
        let mut t = MultiTargetTracker::new(2000, 60, 0.3);
        t.create_track(&make_det(100, 100, 50, 80, 0.9));

        // 3 detections, 1 track — extra detections ignored (no auto_create)
        t.update(&[
            make_det(100, 100, 50, 80, 0.88),
            make_det(300, 300, 50, 80, 0.7),
            make_det(500, 500, 50, 80, 0.6),
        ]);

        assert_eq!(t.track_count(), 1);
    }

    /// B3: потолок треков — flood детекций не разгоняет n (Hungarian O(n³)).
    #[test]
    fn max_tracks_caps_flood() {
        let mut t = MultiTargetTracker::new(60_000, 1000, 0.3)
            .with_auto_create(true)
            .with_max_tracks(64);
        // 500 разнесённых детекций — все захотят новые треки.
        for i in 0..500u32 {
            let d = Detection {
                bbox: common::BoundingBox {
                    x: (i % 25) * 40,
                    y: (i / 25) * 40,
                    width: 20,
                    height: 20,
                },
                class: "x".into(),
                class_id: i,
                confidence: 0.9,
                frame_seq: 1,
                detected_at: Utc::now(),
            };
            t.update(&[d]);
        }
        assert!(t.track_count() <= 64, "got {}", t.track_count());
        assert!(t.track_count() > 0);
    }
}
