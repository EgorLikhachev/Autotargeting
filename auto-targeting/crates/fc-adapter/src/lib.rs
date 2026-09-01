//! Flight Controller Adapter — hardware-agnostic HAL over MAVLink.
//!
//! The `FlightControllerAdapter` trait is the single abstraction that decouples
//! the rest of the system from a specific FC. The `commander` module works
//! with `Box<dyn FlightControllerAdapter>` and never sees MAVLink, UART, or
//! any specific FC implementation.
//!
//! See `docs/ARCHITECTURE.md` §1.3 for the design rationale.

pub mod ardupilot_mavlink;
pub mod mavlink_core;
pub mod mock;
pub mod rate_limiter;
pub mod sitl_mavlink;
pub mod traits;

pub use ardupilot_mavlink::{ArduPilotConfig, ArduPilotMavlinkAdapter};
pub use mock::MockFcAdapter;
pub use rate_limiter::CommandRateLimiter;
pub use sitl_mavlink::{SittlConfig, SittlMavlinkAdapter, TelemetryCache};
pub use traits::{FcError, FlightControllerAdapter};

/// Construct an adapter by name from the common FcConfig.
///
/// This is the factory function used by the CLI to pick the right adapter
/// based on `config.fc.adapter`:
///
/// - `"mock"` → `MockFcAdapter` (in-memory, for tests)
/// - `"sitl-mavlink"` → `SittlMavlinkAdapter` (UDP, for SITL)
/// - `"ardupilot-mavlink"` → `ArduPilotMavlinkAdapter` (serial/TCP/UDP, production)
pub fn build_adapter(cfg: &common::FcConfig) -> Box<dyn FlightControllerAdapter> {
    match cfg.adapter.as_str() {
        "mock" => Box::new(MockFcAdapter::new()),
        "sitl-mavlink" => Box::new(SittlMavlinkAdapter::from_common(cfg)),
        "ardupilot-mavlink" => Box::new(ArduPilotMavlinkAdapter::from_common(cfg)),
        other => {
            tracing::warn!(adapter = other, "unknown FC adapter, falling back to mock");
            Box::new(MockFcAdapter::new())
        }
    }
}
