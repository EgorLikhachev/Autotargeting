//! Flight Controller Adapter — hardware-agnostic HAL over MAVLink.
//!
//! The `FlightControllerAdapter` trait is the single abstraction that decouples
//! the rest of the system from a specific FC. The `commander` module works
//! with `Box<dyn FlightControllerAdapter>` and never sees MAVLink, UART, or
//! any specific FC implementation.
//!
//! See `docs/ARCHITECTURE.md` §1.3 for the design rationale.

pub mod mock;
pub mod rate_limiter;
pub mod traits;

pub use mock::MockFcAdapter;
pub use rate_limiter::CommandRateLimiter;
pub use traits::{FcError, FlightControllerAdapter};
