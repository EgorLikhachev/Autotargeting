//! Commander state machine.
//!
//! Implements the deterministic state-transition logic described in
//! `docs/ARCHITECTURE.md` §1.4 (Level 2 — State Machine with deterministic
//! transitions). Any transition not in the allowed set is rejected.
//!
//! We intentionally use a hand-rolled enum-based FSM rather than a crate like
//! `statig` to keep the transition table explicit and auditable — safety-critical
//! code benefits from being readable at the cost of slightly more boilerplate.

use common::SystemState;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum StateTransitionError {
    #[error("invalid transition: {from} -> {to}")]
    Invalid { from: SystemState, to: SystemState },

    #[error("cannot transition from terminal state: {0}")]
    TerminalState(SystemState),
}

/// Pure state machine — no I/O, no side effects, fully testable.
///
/// The `commander::Commander` struct wraps this and applies side effects
/// (FC commands, watchdog resets) on each transition.
#[derive(Debug, Clone, Default)]
pub struct StateMachine {
    state: SystemState,
    /// Number of transitions since construction — useful for diagnostics.
    transition_count: u64,
}

impl StateMachine {
    pub fn new(initial: SystemState) -> Self {
        Self {
            state: initial,
            transition_count: 0,
        }
    }

    pub fn state(&self) -> SystemState {
        self.state
    }

    pub fn transition_count(&self) -> u64 {
        self.transition_count
    }

    /// Returns `Ok(())` if the transition is allowed, otherwise `Err`.
    ///
    /// Allowed transitions are defined in `is_transition_allowed`. This is the
    /// single source of truth — any state change in the system must go through
    /// this method.
    pub fn try_transition(&mut self, to: SystemState) -> Result<(), StateTransitionError> {
        if self.state == to {
            // Idempotent — no-op is always allowed
            return Ok(());
        }
        if !is_transition_allowed(self.state, to) {
            return Err(StateTransitionError::Invalid {
                from: self.state,
                to,
            });
        }
        tracing::info!(
            from = self.state.as_str(),
            to = to.as_str(),
            "state transition"
        );
        self.state = to;
        self.transition_count += 1;
        Ok(())
    }

    /// Force a transition — bypasses the allowed-transition check.
    /// Used ONLY for safety-critical overrides (e.g. operator ABORT, or
    /// watchdog-triggered ABORT). Logs a warning.
    pub fn force_transition(&mut self, to: SystemState) {
        if self.state == to {
            return;
        }
        tracing::warn!(
            from = self.state.as_str(),
            to = to.as_str(),
            "FORCED state transition (bypassing allowed-transitions check)"
        );
        self.state = to;
        self.transition_count += 1;
    }
}

/// The allowed-transition table. This is the single source of truth.
///
/// ```text
/// IDLE ──arm──► ARMED ──scan──► SCANNING
///                                 │
///                                 ▼ (target selected)
///                            TARGET_SELECTED
///                                 │
///                                 ▼ (lock acquired <1s)
///                              TRACKING ◄───┐
///                                 │         │
///                                 ▼         │ (reacquired <2s)
///                                LOST ──────┘
///                                 │
///                                 ▼ (lost >2s)
///                                RTH
///                                 │
///                                 ▼ (operator override)
///                                IDLE / ABORT
/// ```
///
/// Additional edges (not in the simplified diagram above):
/// - TRACKING ↔ TRACKING_DEGRADED (watchdog trigger / recovery)
/// - any state → ABORT (watchdog, safety violation)
/// - ABORT → IDLE (operator reset, only after disarm confirmed)
pub fn is_transition_allowed(from: SystemState, to: SystemState) -> bool {
    use SystemState::*;
    match (from, to) {
        // Normal flow
        (Idle, Armed) => true,
        (Armed, Scanning) => true,
        (Scanning, TargetSelected) => true,
        (TargetSelected, Tracking) => true,
        (TargetSelected, TrackingDegraded) => true, // degraded lock allowed
        (Tracking, TrackingDegraded) => true,
        (TrackingDegraded, Tracking) => true,
        (Tracking, Lost) => true,
        (TrackingDegraded, Lost) => true,
        (Lost, Tracking) => true, // reacquired
        (Lost, Rth) => true,      // gave up
        (Rth, Idle) => true,      // landed / disarmed

        // Operator-driven transitions (operator may always go back to scanning/idle)
        (Scanning, Idle) => true,
        (Tracking, Scanning) => true, // operator switched target mid-track
        (TrackingDegraded, Scanning) => true,
        (Lost, Scanning) => true, // operator restarts scan
        (TargetSelected, Scanning) => true,

        // Any state → ABORT (safety override)
        (_, Abort) => true,

        // ABORT can only go back to Idle (after disarm)
        (Abort, Idle) => true,

        // All other transitions are rejected
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_flow_idle_to_tracking() {
        let mut sm = StateMachine::new(SystemState::Idle);
        assert!(sm.try_transition(SystemState::Armed).is_ok());
        assert!(sm.try_transition(SystemState::Scanning).is_ok());
        assert!(sm.try_transition(SystemState::TargetSelected).is_ok());
        assert!(sm.try_transition(SystemState::Tracking).is_ok());
        assert_eq!(sm.transition_count(), 4);
    }

    #[test]
    fn rejects_invalid_transition() {
        let mut sm = StateMachine::new(SystemState::Idle);
        // Cannot go directly from IDLE to TRACKING
        let err = sm.try_transition(SystemState::Tracking).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::Invalid {
                from: SystemState::Idle,
                to: SystemState::Tracking,
            }
        );
    }

    #[test]
    fn tracking_to_lost_to_rth() {
        let mut sm = StateMachine::new(SystemState::Tracking);
        assert!(sm.try_transition(SystemState::Lost).is_ok());
        assert!(sm.try_transition(SystemState::Rth).is_ok());
    }

    #[test]
    fn lost_to_tracking_reacquired() {
        let mut sm = StateMachine::new(SystemState::Lost);
        assert!(sm.try_transition(SystemState::Tracking).is_ok());
    }

    #[test]
    fn any_state_to_abort() {
        for from in [
            SystemState::Idle,
            SystemState::Armed,
            SystemState::Scanning,
            SystemState::TargetSelected,
            SystemState::Tracking,
            SystemState::TrackingDegraded,
            SystemState::Lost,
            SystemState::Rth,
        ] {
            let mut sm = StateMachine::new(from);
            assert!(
                sm.try_transition(SystemState::Abort).is_ok(),
                "should be able to ABORT from {:?}",
                from
            );
        }
    }

    #[test]
    fn abort_only_goes_to_idle() {
        let mut sm = StateMachine::new(SystemState::Abort);
        // Cannot go to Tracking directly
        assert!(sm.try_transition(SystemState::Tracking).is_err());
        // Cannot go to Scanning
        assert!(sm.try_transition(SystemState::Scanning).is_err());
        // Can go to Idle (after disarm)
        assert!(sm.try_transition(SystemState::Idle).is_ok());
    }

    #[test]
    fn idempotent_transition_is_ok() {
        let mut sm = StateMachine::new(SystemState::Tracking);
        assert!(sm.try_transition(SystemState::Tracking).is_ok());
        assert_eq!(sm.transition_count(), 0); // no actual transition
    }

    #[test]
    fn force_transition_bypasses_check() {
        let mut sm = StateMachine::new(SystemState::Idle);
        sm.force_transition(SystemState::Tracking);
        assert_eq!(sm.state(), SystemState::Tracking);
        assert_eq!(sm.transition_count(), 1);
    }

    #[test]
    fn tracking_degraded_round_trip() {
        let mut sm = StateMachine::new(SystemState::Tracking);
        assert!(sm.try_transition(SystemState::TrackingDegraded).is_ok());
        assert!(sm.try_transition(SystemState::Tracking).is_ok());
    }

    #[test]
    fn operator_can_restart_scan_from_tracking() {
        let mut sm = StateMachine::new(SystemState::Tracking);
        assert!(sm.try_transition(SystemState::Scanning).is_ok());
    }

    #[test]
    fn full_loss_recovery_cycle() {
        let mut sm = StateMachine::new(SystemState::Tracking);
        sm.try_transition(SystemState::Lost).unwrap();
        sm.try_transition(SystemState::Tracking).unwrap(); // reacquired
        sm.try_transition(SystemState::Lost).unwrap();
        sm.try_transition(SystemState::Rth).unwrap();
        sm.try_transition(SystemState::Idle).unwrap();
        assert_eq!(sm.state(), SystemState::Idle);
        assert_eq!(sm.transition_count(), 5);
    }
}
