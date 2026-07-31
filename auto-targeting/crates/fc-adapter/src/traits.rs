//! Trait definition for the Flight Controller Adapter (HAL).
//!
//! Any FC implementation (ArduPilot, PX4, SITL, mock) must satisfy this trait.
//! The `commander` module works only with `dyn FlightControllerAdapter` and
//! has zero knowledge of the underlying transport (UART, USB, UDP, in-memory).

use async_trait::async_trait;
use common::{Attitude, FlightMode, GlobalPosition, HeartbeatStatus, PositionTargetNED, RoiTarget};
use thiserror::Error;

pub type FcResult<T> = std::result::Result<T, FcError>;

#[derive(Debug, Error)]
pub enum FcError {
    #[error("FC connection error: {0}")]
    Connection(String),

    #[error("FC command rejected: {0}")]
    CommandRejected(String),

    #[error("FC heartbeat lost (last seen {last_seen_ms}ms ago)")]
    HeartbeatLost { last_seen_ms: u64 },

    #[error("FC communication timeout")]
    Timeout,

    #[error("FC not armed")]
    NotArmed,

    #[error("FC mode not supported: {0:?}")]
    UnsupportedMode(FlightMode),

    #[error("FC internal error: {0}")]
    Internal(String),
}

/// Hardware-agnostic flight controller abstraction.
///
/// Implementations:
/// - `ArduPilotMavlinkAdapter` — production (UART/USB to SpeedyBee F405, etc.)
/// - `SittlMavlinkAdapter` — CI/SITL (UDP to ArduPilot SITL)
/// - `MockFcAdapter` — unit tests (in-memory, records all commands)
///
/// All async methods are cancellation-safe. Cancellation mid-command should
/// not leave the FC in an inconsistent state.
#[async_trait]
pub trait FlightControllerAdapter: Send {
    /// Set Region of Interest — what the camera/gimbal should point at.
    /// `RoiTarget::None` clears the ROI.
    async fn set_roi(&mut self, roi: RoiTarget) -> FcResult<()>;

    /// Stream a position target in local NED frame. Called at 10 Hz by the
    /// commander. ArduPilot in GUIDED mode will hold this position.
    async fn set_position_target_local_ned(&mut self, target: PositionTargetNED) -> FcResult<()>;

    /// Change flight mode (GUIDED, LOITER, RTL, MANUAL, etc.).
    async fn set_mode(&mut self, mode: FlightMode) -> FcResult<()>;

    /// Arm the drone.
    async fn arm(&mut self) -> FcResult<()>;

    /// Disarm the drone. Motors stop immediately.
    async fn disarm(&mut self) -> FcResult<()>;

    /// Get the latest attitude from the FC's telemetry stream.
    /// Returns from cache — does not perform a synchronous request.
    /// May return a stale value if telemetry hasn't been received yet.
    fn attitude(&self) -> Attitude;

    /// Get the latest global position (GPS).
    fn global_position(&self) -> Option<GlobalPosition>;

    /// Get heartbeat status — last time we heard from the FC.
    fn heartbeat_status(&self) -> HeartbeatStatus;

    /// Returns true if the FC has been silent for longer than the configured
    /// heartbeat timeout. Convenience method on top of `heartbeat_status()`.
    fn is_heartbeat_stale(&self, timeout_ms: u64) -> bool {
        self.heartbeat_status().is_stale(timeout_ms)
    }

    /// Initialize connection to the FC. Called once at startup.
    /// Implementations should establish the underlying transport
    /// (open serial port, connect UDP socket, etc.) and spawn any
    /// background telemetry reader tasks.
    async fn connect(&mut self) -> FcResult<()>;

    /// Gracefully disconnect. Called on shutdown.
    async fn disconnect(&mut self) -> FcResult<()>;

    /// Human-readable name of the adapter (e.g. "ArduPilotMavlinkAdapter").
    fn name(&self) -> &'static str;
}
