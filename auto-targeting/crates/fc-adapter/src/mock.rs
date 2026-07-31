//! In-memory mock FlightControllerAdapter.
//!
//! Used for:
//! - Unit tests across the workspace (deterministic, no network, no FC).
//! - The `cli --mock-fc` smoke test (no hardware required).
//!
//! Records every command received so tests can assert on the command stream.
//! Simulates heartbeat: `heartbeat_status()` is fresh for `heartbeat_timeout_ms`
//! after `connect()` is called. Tests can call `simulate_heartbeat_loss()` to
//! trigger watchdog scenarios.

use crate::traits::{FcError, FlightControllerAdapter};
use async_trait::async_trait;
use common::{Attitude, FlightMode, GlobalPosition, HeartbeatStatus, PositionTargetNED, RoiTarget};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;
use tracing::debug;

/// A recorded FC command — for test assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedCommand {
    SetRoi(RoiTarget),
    SetPositionTargetLocalNed(PositionTargetNED),
    SetMode(FlightMode),
    Arm,
    Disarm,
}

/// Internal mutable state — shared via Arc<Mutex<...>> so tests can inspect
/// after the adapter has been moved into a commander.
#[derive(Debug, Default)]
pub struct MockFcState {
    pub commands: Vec<RecordedCommand>,
    pub armed: bool,
    pub mode: FlightMode,
    pub attitude: Attitude,
    pub global_position: Option<GlobalPosition>,
    pub connected: bool,
    pub last_heartbeat: Option<Instant>,
}

#[derive(Clone)]
pub struct MockFcAdapter {
    state: Arc<Mutex<MockFcState>>,
    /// Optional delay injected before each command — for testing latency.
    artificial_delay_ms: u64,
}

impl MockFcAdapter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockFcState::default())),
            artificial_delay_ms: 0,
        }
    }

    /// Create a mock with an artificial delay per command (simulates slow FC).
    pub fn with_delay_ms(delay_ms: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(MockFcState::default())),
            artificial_delay_ms: delay_ms,
        }
    }

    /// Get a clone of the shared state handle — for test assertions.
    pub fn state_handle(&self) -> Arc<Mutex<MockFcState>> {
        Arc::clone(&self.state)
    }

    /// Simulate a heartbeat loss — sets last_heartbeat to None.
    /// After this, `is_heartbeat_stale(timeout)` will return true.
    pub fn simulate_heartbeat_loss(&self) {
        let mut s = self.state.lock();
        s.last_heartbeat = None;
    }

    /// Simulate an incoming attitude update — as if the FC telemetry stream
    /// pushed a new attitude. Useful for tests that check commander response
    /// to attitude changes.
    pub fn simulate_attitude(&self, attitude: Attitude) {
        let mut s = self.state.lock();
        s.attitude = attitude;
        s.last_heartbeat = Some(Instant::now());
    }

    /// Simulate a GPS position update.
    pub fn simulate_global_position(&self, pos: GlobalPosition) {
        let mut s = self.state.lock();
        s.global_position = Some(pos);
    }

    /// Inject a delay if configured (simulates slow FC).
    async fn maybe_delay(&self) {
        if self.artificial_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.artificial_delay_ms)).await;
        }
    }

    /// Get a snapshot of all recorded commands (for test assertions).
    pub fn recorded_commands(&self) -> Vec<RecordedCommand> {
        self.state.lock().commands.clone()
    }

    /// Clear all recorded commands.
    pub fn clear_recorded(&self) {
        self.state.lock().commands.clear();
    }
}

impl Default for MockFcAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FlightControllerAdapter for MockFcAdapter {
    async fn set_roi(&mut self, roi: RoiTarget) -> Result<(), FcError> {
        self.maybe_delay().await;
        let mut s = self.state.lock();
        if !s.connected {
            return Err(FcError::Connection("not connected".to_string()));
        }
        debug!(?roi, "mock FC: set_roi");
        s.commands.push(RecordedCommand::SetRoi(roi));
        Ok(())
    }

    async fn set_position_target_local_ned(
        &mut self,
        target: PositionTargetNED,
    ) -> Result<(), FcError> {
        self.maybe_delay().await;
        let mut s = self.state.lock();
        if !s.connected {
            return Err(FcError::Connection("not connected".to_string()));
        }
        s.commands
            .push(RecordedCommand::SetPositionTargetLocalNed(target));
        Ok(())
    }

    async fn set_mode(&mut self, mode: FlightMode) -> Result<(), FcError> {
        self.maybe_delay().await;
        let mut s = self.state.lock();
        if !s.connected {
            return Err(FcError::Connection("not connected".to_string()));
        }
        debug!(?mode, "mock FC: set_mode");
        s.mode = mode;
        s.commands.push(RecordedCommand::SetMode(mode));
        Ok(())
    }

    async fn arm(&mut self) -> Result<(), FcError> {
        self.maybe_delay().await;
        let mut s = self.state.lock();
        if !s.connected {
            return Err(FcError::Connection("not connected".to_string()));
        }
        debug!("mock FC: arm");
        s.armed = true;
        s.commands.push(RecordedCommand::Arm);
        Ok(())
    }

    async fn disarm(&mut self) -> Result<(), FcError> {
        self.maybe_delay().await;
        let mut s = self.state.lock();
        if !s.connected {
            return Err(FcError::Connection("not connected".to_string()));
        }
        debug!("mock FC: disarm");
        s.armed = false;
        s.commands.push(RecordedCommand::Disarm);
        Ok(())
    }

    fn attitude(&self) -> Attitude {
        self.state.lock().attitude
    }

    fn global_position(&self) -> Option<GlobalPosition> {
        self.state.lock().global_position
    }

    fn heartbeat_status(&self) -> HeartbeatStatus {
        let s = self.state.lock();
        HeartbeatStatus {
            // Convert Instant to chrono::DateTime by storing the offset from now
            last_heartbeat: s
                .last_heartbeat
                .map(|t| {
                    let elapsed = t.elapsed();
                    chrono::Utc::now() - chrono::Duration::from_std(elapsed).unwrap_or_default()
                })
                .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::seconds(60)),
            armed: s.armed,
            mode: s.mode,
        }
    }

    async fn connect(&mut self) -> Result<(), FcError> {
        let mut s = self.state.lock();
        s.connected = true;
        s.last_heartbeat = Some(Instant::now());
        s.mode = FlightMode::Stabilize;
        debug!("mock FC: connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), FcError> {
        let mut s = self.state.lock();
        s.connected = false;
        debug!("mock FC: disconnected");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "MockFcAdapter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_commands_in_order() {
        let mut fc = MockFcAdapter::new();
        fc.connect().await.unwrap();

        fc.arm().await.unwrap();
        fc.set_mode(FlightMode::Guided).await.unwrap();
        fc.set_roi(RoiTarget::LocalNed {
            north: 1.0,
            east: 2.0,
            down: 0.0,
        })
        .await
        .unwrap();
        fc.set_position_target_local_ned(PositionTargetNED {
            north: 1.0,
            east: 2.0,
            down: 0.0,
            yaw: 0.5,
        })
        .await
        .unwrap();
        fc.disarm().await.unwrap();

        let cmds = fc.recorded_commands();
        assert_eq!(cmds.len(), 5);
        assert_eq!(cmds[0], RecordedCommand::Arm);
        assert!(matches!(
            cmds[1],
            RecordedCommand::SetMode(FlightMode::Guided)
        ));
        assert!(matches!(cmds[2], RecordedCommand::SetRoi(_)));
        assert!(matches!(
            cmds[3],
            RecordedCommand::SetPositionTargetLocalNed(_)
        ));
        assert_eq!(cmds[4], RecordedCommand::Disarm);
    }

    #[tokio::test]
    async fn rejects_commands_when_not_connected() {
        let mut fc = MockFcAdapter::new();
        // No connect() called
        let res = fc.arm().await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(err, FcError::Connection(_)));
    }

    #[tokio::test]
    async fn heartbeat_is_fresh_after_connect() {
        let mut fc = MockFcAdapter::new();
        fc.connect().await.unwrap();
        assert!(!fc.is_heartbeat_stale(1000));
    }

    #[tokio::test]
    async fn heartbeat_loss_detected() {
        let mut fc = MockFcAdapter::new();
        fc.connect().await.unwrap();
        assert!(!fc.is_heartbeat_stale(1000));

        fc.simulate_heartbeat_loss();
        assert!(fc.is_heartbeat_stale(1000));
    }

    #[tokio::test]
    async fn simulated_attitude_updates_state() {
        let mut fc = MockFcAdapter::new();
        fc.connect().await.unwrap();

        let att = Attitude {
            roll: 0.1,
            pitch: -0.05,
            yaw: 1.2,
            ..Default::default()
        };
        fc.simulate_attitude(att);
        let current = fc.attitude();
        assert_eq!(current.roll, 0.1);
        assert_eq!(current.yaw, 1.2);
    }

    #[tokio::test]
    async fn shared_state_visible_through_handle() {
        let mut fc = MockFcAdapter::new();
        let handle = fc.state_handle();
        fc.connect().await.unwrap();
        fc.arm().await.unwrap();

        // Inspect state through the handle without touching fc
        let s = handle.lock();
        assert!(s.armed);
        assert_eq!(s.commands.len(), 1);
    }
}
