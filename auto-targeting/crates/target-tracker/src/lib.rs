//! Target tracker — maintains target state between detections.
//!
//! Two implementations:
//! - `TargetTracker` — single-target (Phase 0/3 baseline)
//! - `MultiTargetTracker` — multi-target with Hungarian algorithm (Phase 3 stretch)
//!
//! ## Status
//!
//! - `KalmanFilter2D` — ✅ Working
//! - `TargetTracker` — ✅ Working (single-target)
//! - `MultiTargetTracker` — ✅ Working (Hungarian algorithm for assignment)
//! - `hungarian` — ✅ Working (O(n³) Kuhn-Munkres)
//!
//! See `docs/ARCHITECTURE.md` and `docs/ADR/0002-tracking-algorithm.md`.

pub mod hungarian;
pub mod kalman;
pub mod multi_tracker;
pub mod tracker;

pub use kalman::KalmanFilter2D;
pub use multi_tracker::MultiTargetTracker;
pub use tracker::{TargetTracker, TrackerError, TrackerResult};
