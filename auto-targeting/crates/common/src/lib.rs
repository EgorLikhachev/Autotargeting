//! Common types, errors, and configuration shared across all modules.

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
