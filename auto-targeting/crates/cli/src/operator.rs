//! Operator command types.
//!
//! These are the commands an operator (or future gRPC/HTTP client) can issue
//! to the running system. In Phase 5 these will be delivered over a socket.

use common::{FlightMode, TargetId};

/// Operator commands — issued by the human operator (or future gRPC/HTTP client)
/// to control the running system. Phase 5 will wire these into a real API.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum OperatorCommand {
    /// Select a detected target to track.
    SelectTarget { target_id: TargetId },
    /// Switch to a different flight mode.
    SetMode { mode: FlightMode },
    /// Abort — transition to ABORT state, trigger RTH.
    Abort,
    /// Reset — return to IDLE (only valid from ABORT after disarm).
    Reset,
    /// Disarm the drone.
    Disarm,
}
