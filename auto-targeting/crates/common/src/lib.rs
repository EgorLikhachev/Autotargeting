//! # Common crate
//!
//! Shared types, errors, and configuration for the Auto-Targeting System.
//!
//! ## Overview
//!
//! This crate provides the foundational types used across all modules:
//!
//! - **Types**: `Frame`, `Detection`, `TargetState`, `Attitude`, `FlightMode`, etc.
//! - **Errors**: `CommonError` with variants for each module
//! - **Config**: `AppConfig` with TOML parsing + env var overrides
//! - **Scenario**: `Scenario` parser for SITL test scenarios
//!
//! ## Usage
//!
//! ```ignore
//! use common::{AppConfig, Detection, BoundingBox};
//!
//! let config = AppConfig::load_or_default(Some("config.toml"));
//! let det = Detection {
//!     bbox: BoundingBox { x: 100, y: 200, width: 50, height: 80 },
//!     class: "person".to_string(),
//!     class_id: 0,
//!     confidence: 0.92,
//!     frame_seq: 1,
//!     detected_at: chrono::Utc::now(),
//! };
//! ```
//!
//! ## Modules
//!
//! - [`config`] — TOML configuration with env overrides
//! - [`errors`] — Error types for all modules
//! - [`types`] — Core domain types (Frame, Detection, Attitude, etc.)
//! - [`scenario`] — JSON test scenario parser

pub mod config;
pub mod errors;
pub mod scenario;
pub mod types;

pub use config::{
    AppConfig, CommanderConfig, FcConfig, InferenceConfig, TrackerConfig, VideoConfig,
};
pub use errors::{CommonError, Result};
pub use scenario::{Scenario, ScenarioError};
pub use types::{
    Attitude, BoundingBox, Detection, FlightMode, Frame, FrameMetadata, GlobalPosition,
    HeartbeatStatus, PixelFormat, PositionTargetNED, RoiTarget, SystemState, TargetId, TargetState,
    Timestamp,
};
