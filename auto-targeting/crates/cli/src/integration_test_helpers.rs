//! Integration test helpers — shared utilities for end-to-end tests.
//!
//! These are exposed as a public module so that integration tests in `tests/`
//! can construct a fully-wired pipeline without duplicating setup code.

use commander::Commander;
use common::CommanderConfig;
use fc_adapter::{FlightControllerAdapter, MockFcAdapter};
use target_tracker::TargetTracker;

/// A fully-wired test harness:
/// - MockFcAdapter (in-memory, records commands) — shared state
/// - Commander (with state machine + watchdogs + anti-loop)
/// - TargetTracker (Kalman + IoU)
///
/// The harness is created in the ARMED state (connect + arm already done).
///
/// `fc_for_assertions` shares state with the commander's FC, so tests can
/// inspect recorded commands via `fc_for_assertions.recorded_commands()`.
pub struct PipelineHarness {
    pub commander: Commander,
    /// A MockFcAdapter that shares state with the commander's FC.
    /// Use this for assertions (recorded_commands, heartbeat_status, etc.).
    pub fc: MockFcAdapter,
    pub tracker: TargetTracker,
}

impl PipelineHarness {
    /// Create a new harness. The MockFcAdapter is connected and armed,
    /// and the commander is in the ARMED state.
    pub async fn new() -> Self {
        // Create the FC and grab a handle to its shared state BEFORE
        // moving it into the commander.
        let fc = MockFcAdapter::new();
        let shared_state = fc.state_handle();

        // Create a second MockFcAdapter that shares state with the first —
        // for test assertions after the first is moved into the commander.
        let fc_for_assertions = MockFcAdapter::new_with_shared_state(shared_state);

        // Wrap the original FC in a Box<dyn FlightControllerAdapter> for the commander.
        let fc_for_commander: Box<dyn FlightControllerAdapter> = Box::new(fc);

        let mut commander = Commander::new(CommanderConfig::default(), fc_for_commander);
        let tracker = TargetTracker::from_common(&common::TrackerConfig::default());

        // Connect + arm
        commander.connect().await.unwrap();
        commander.arm().await.unwrap();

        Self {
            commander,
            fc: fc_for_assertions,
            tracker,
        }
    }
}
