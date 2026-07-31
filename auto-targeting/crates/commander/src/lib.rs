//! Commander — top-level state machine + watchdogs + anti-loop protection.
//!
//! The commander is the only module with authority to issue MAVLink commands
//! to the FC. It owns the system state machine, runs watchdog timers on every
//! async loop, and applies the deadband / hysteresis / rate-limiting /
//! oscillation detector stack described in `docs/ARCHITECTURE.md` §1.4.

pub mod anti_loop;
pub mod commander;
pub mod state_machine;
pub mod watchdogs;

pub use anti_loop::{AntiLoopGuard, CorrectionCommand, GuardDecision};
pub use commander::{Commander, CommanderError, CommanderHealth, CommanderResult};
pub use state_machine::{StateMachine, StateTransitionError};
pub use watchdogs::{
    WatchdogAction, WatchdogConfig, WatchdogId, WatchdogRegistry, WatchdogSnapshot,
};
