//! Target tracker — maintains target state between detections.
//!
//! Status: 🚧 Phase 0 scaffolding only.
//! Phase 3 will implement:
//! - `KalmanFilter2D` — predicts target position between detections.
//! - `SortTracker` — DeepSORT-style multi-object tracker (or KCF fallback).
//! - `TargetTracker` — main entry point, owns the active target state.
//!
//! See `docs/ARCHITECTURE.md` §1.1 module 3.

pub mod kalman;
pub mod tracker;

pub use kalman::KalmanFilter2D;
pub use tracker::{TargetTracker, TrackerError, TrackerResult};
