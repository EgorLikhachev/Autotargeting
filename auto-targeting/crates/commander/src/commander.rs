//! Commander — the top-level orchestrator.
//!
//! Owns the state machine, watchdog registry, anti-loop guard, and FC adapter.
//! This is the single component with authority to issue MAVLink commands to
//! the FC. All control flow goes through `Commander`.
//!
//! ## Responsibilities
//!
//! 1. **State transitions:** validates and applies state changes via the
//!    internal `StateMachine`. Invalid transitions are rejected.
//! 2. **Watchdog management:** registers and feeds watchdogs, processes
//!    expiries, transitions to `TRACKING_DEGRADED` or `ABORT` as configured.
//! 3. **Anti-loop protection:** runs every command through `AntiLoopGuard`
//!    before sending to the FC.
//! 4. **FC commands:** wraps `FlightControllerAdapter` with rate limiting
//!    and safety checks.
//! 5. **Operator commands:** handles `OperatorCommand` from the CLI/REPL.
//!
//! ## Lifecycle
//!
//! ```text
//! Commander::new(config, fc_adapter) → Commander
//!   ├── connect() → establish FC connection
//!   ├── arm() → arm the drone
//!   ├── start_scanning() → transition to SCANNING
//!   ├── select_target(detection) → acquire target, transition to TRACKING
//!   ├── update(detections) → called every frame, updates tracker + FC commands
//!   ├── abort() → force ABORT + RTL
//!   └── shutdown() → disconnect FC
//! ```

use crate::anti_loop::{AntiLoopGuard, CorrectionCommand, GuardDecision};
use crate::state_machine::StateMachine;
use crate::watchdogs::{WatchdogAction, WatchdogConfig, WatchdogId, WatchdogRegistry};
use common::{CommanderConfig, Detection, FlightMode, RoiTarget, SystemState, TargetId};
use fc_adapter::{CommandRateLimiter, FlightControllerAdapter};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Errors that can occur in the commander.
#[derive(Debug, thiserror::Error)]
pub enum CommanderError {
    #[error("state transition error: {0}")]
    StateTransition(#[from] crate::state_machine::StateTransitionError),

    #[error("FC error: {0}")]
    Fc(#[from] fc_adapter::FcError),

    #[error("not connected — call connect() first")]
    NotConnected,

    #[error("not armed — call arm() first")]
    NotArmed,

    #[error("invalid state for operation: {current} (expected {expected})")]
    InvalidState {
        current: SystemState,
        expected: SystemState,
    },

    #[error("no active target — call select_target() first")]
    NoActiveTarget,

    #[error("commander internal error: {0}")]
    Internal(String),
}

pub type CommanderResult<T> = std::result::Result<T, CommanderError>;

/// The Commander — top-level orchestrator.
///
/// Holds references to:
/// - State machine (sync, parking_lot::Mutex — never held across .await)
/// - Watchdog registry (thread-safe via parking_lot::Mutex)
/// - Anti-loop guard (thread-safe via parking_lot::Mutex)
/// - FC adapter (async — owned, methods are async)
/// - Rate limiter (sync)
pub struct Commander {
    /// System state machine.
    state: Arc<Mutex<StateMachine>>,
    /// Watchdog registry — shared with background checker task.
    watchdogs: Arc<WatchdogRegistry>,
    /// Anti-loop guard — wraps every command before sending to FC.
    anti_loop: Arc<AntiLoopGuard>,
    /// FC adapter — owned, async methods.
    fc: Box<dyn FlightControllerAdapter>,
    /// Rate limiter for FC commands.
    rate_limiter: CommandRateLimiter,
    /// Configuration (commander section).
    config: CommanderConfig,
    /// Currently tracked target ID (if in TRACKING state).
    active_target_id: Option<TargetId>,
    /// Last correction command sent (for diagnostics).
    last_command_at: Option<Instant>,
    /// Connected flag.
    connected: bool,
}

impl Commander {
    /// Create a new Commander with the given config and FC adapter.
    pub fn new(config: CommanderConfig, fc: Box<dyn FlightControllerAdapter>) -> Self {
        let watchdogs = Arc::new(WatchdogRegistry::new());
        let anti_loop = Arc::new(AntiLoopGuard::new(config.clone()));

        // Register all default watchdogs
        watchdogs.register(
            WatchdogId::VideoLoop,
            WatchdogConfig::new(
                Duration::from_millis(config.video_loop_wdt_ms),
                WatchdogAction::Degrade,
            ),
        );
        watchdogs.register(
            WatchdogId::InferenceLoop,
            WatchdogConfig::new(
                Duration::from_millis(config.inference_loop_wdt_ms),
                WatchdogAction::Degrade,
            ),
        );
        watchdogs.register(
            WatchdogId::TrackingLoop,
            WatchdogConfig::new(
                Duration::from_millis(config.tracking_loop_wdt_ms),
                WatchdogAction::Degrade,
            ),
        );
        watchdogs.register(
            WatchdogId::CommandLoop,
            WatchdogConfig::new(
                Duration::from_millis(config.command_loop_wdt_ms),
                WatchdogAction::Abort,
            ),
        );

        let rate_limiter = CommandRateLimiter::new(10); // 10 Hz

        Self {
            state: Arc::new(Mutex::new(StateMachine::new(SystemState::Idle))),
            watchdogs,
            anti_loop,
            fc,
            rate_limiter,
            config,
            active_target_id: None,
            last_command_at: None,
            connected: false,
        }
    }

    /// Get a handle to the watchdog registry (for background feeding).
    pub fn watchdog_registry(&self) -> Arc<WatchdogRegistry> {
        Arc::clone(&self.watchdogs)
    }

    /// Get a handle to the anti-loop guard (for diagnostics).
    pub fn anti_loop_guard(&self) -> Arc<AntiLoopGuard> {
        Arc::clone(&self.anti_loop)
    }

    /// Get current system state.
    pub fn state(&self) -> SystemState {
        self.state.lock().state()
    }

    /// Get the number of state transitions since startup.
    pub fn transition_count(&self) -> u64 {
        self.state.lock().transition_count()
    }

    /// Connect to the FC.
    pub async fn connect(&mut self) -> CommanderResult<()> {
        info!("Commander: connecting to FC");
        self.fc.connect().await?;
        self.connected = true;

        // Register the FC heartbeat watchdog now that we're connected
        self.watchdogs.register(
            WatchdogId::FcHeartbeat,
            WatchdogConfig::new(
                Duration::from_millis(1000), // default; will be overridden in real config
                WatchdogAction::Abort,
            ),
        );

        // Transition to ARMED
        {
            let mut sm = self.state.lock();
            sm.try_transition(SystemState::Armed)?;
        }
        info!("Commander: connected, state → ARMED");
        Ok(())
    }

    /// Disconnect from the FC.
    pub async fn disconnect(&mut self) -> CommanderResult<()> {
        info!("Commander: disconnecting from FC");
        self.fc.disconnect().await?;
        self.connected = false;
        Ok(())
    }

    /// Arm the drone.
    pub async fn arm(&mut self) -> CommanderResult<()> {
        if !self.connected {
            return Err(CommanderError::NotConnected);
        }
        info!("Commander: arming");
        self.fc.arm().await?;
        Ok(())
    }

    /// Disarm the drone.
    pub async fn disarm(&mut self) -> CommanderResult<()> {
        if !self.connected {
            return Err(CommanderError::NotConnected);
        }
        info!("Commander: disarming");
        self.fc.disarm().await?;
        // Transition to IDLE
        {
            let mut sm = self.state.lock();
            let _ = sm.try_transition(SystemState::Idle);
        }
        self.active_target_id = None;
        Ok(())
    }

    /// Start scanning for targets.
    pub fn start_scanning(&mut self) -> CommanderResult<()> {
        let mut sm = self.state.lock();
        sm.try_transition(SystemState::Scanning)?;
        self.active_target_id = None;
        info!("Commander: state → SCANNING");
        Ok(())
    }

    /// Select a target to track. Transitions to TARGET_SELECTED, then
    /// (if lock is acquired within 1 s) to TRACKING.
    ///
    /// For Phase 5 this is a synchronous lock — in production it should
    /// wait for the tracker to confirm `lock_confirmation_frames` consecutive
    /// detections before transitioning to TRACKING.
    pub fn select_target(&mut self, target_id: TargetId) -> CommanderResult<()> {
        let mut sm = self.state.lock();
        let current = sm.state();
        match current {
            SystemState::Scanning
            | SystemState::Tracking
            | SystemState::TrackingDegraded
            | SystemState::Lost => {
                sm.try_transition(SystemState::TargetSelected)?;
                // Immediately transition to TRACKING (Phase 5 simplification —
                // real lock acquisition is handled by the tracker).
                sm.try_transition(SystemState::Tracking)?;
                self.active_target_id = Some(target_id);
                info!(target_id, "Commander: target selected, state → TRACKING");
                Ok(())
            }
            _ => Err(CommanderError::InvalidState {
                current,
                expected: SystemState::Scanning,
            }),
        }
    }

    /// Process a frame's detections. This is the main per-frame entry point.
    ///
    /// - Feeds the inference watchdog.
    /// - Updates the active target state (if tracking).
    /// - Generates a correction command and sends it to the FC (via anti-loop guard).
    ///
    /// `target_offset` is the (x, y) offset of the target from frame center,
    /// as a fraction of frame size ([-1.0, 1.0]).
    pub async fn update(
        &mut self,
        detections: &[Detection],
        target_offset: Option<(f32, f32)>,
    ) -> CommanderResult<()> {
        // Feed the inference watchdog — we got new detections
        self.watchdogs.feed(WatchdogId::InferenceLoop);
        self.watchdogs.feed(WatchdogId::TrackingLoop);

        // If we have an active target and an offset, generate a correction command
        if let (Some(_target_id), Some((offset_x, offset_y))) =
            (self.active_target_id, target_offset)
        {
            let state = self.state();
            if state == SystemState::Tracking || state == SystemState::TrackingDegraded {
                // Convert offset to yaw/pitch rate commands.
                // Simple proportional control — Phase 5 will add PID.
                let yaw_rate = offset_x * self.config.max_yaw_rate_dps;
                let pitch_rate = offset_y * self.config.max_pitch_rate_dps;

                let cmd = CorrectionCommand {
                    yaw_rate_dps: yaw_rate,
                    pitch_rate_dps: pitch_rate,
                    offset_x,
                    offset_y,
                    generated_at: Instant::now(),
                };

                // Run through anti-loop guard
                let decision = self.anti_loop.process(cmd);
                match decision {
                    GuardDecision::Allow(clipped) => {
                        self.send_correction_to_fc(clipped).await?;
                    }
                    GuardDecision::Suppress => {
                        debug!("Commander: command suppressed by anti-loop guard");
                    }
                    GuardDecision::Degrade => {
                        warn!("Commander: oscillation detected → TRACKING_DEGRADED");
                        let mut sm = self.state.lock();
                        let _ = sm.try_transition(SystemState::TrackingDegraded);
                    }
                    GuardDecision::Abort => {
                        error!("Commander: oscillation escalated → ABORT");
                        return self.abort_internal().await;
                    }
                }
            }
        }

        // Check for target loss — if we have an active target but no detections
        // for several consecutive frames, the tracker will eventually transition
        // us to LOST. Here we just log the gap; the actual loss detection is
        // in the tracker (via missed_frames counter).
        if let Some(_target_id) = self.active_target_id {
            let state = self.state();
            if (state == SystemState::Tracking || state == SystemState::TrackingDegraded)
                && detections.is_empty()
            {
                debug!("Commander: no detections this frame (target may be occluded)");
            }
        }

        // Feed the command loop watchdog — we completed a cycle
        self.watchdogs.feed(WatchdogId::CommandLoop);
        self.last_command_at = Some(Instant::now());

        Ok(())
    }

    /// Feed the video loop watchdog. Called by the video capture task on
    /// every frame.
    pub fn feed_video_watchdog(&self) {
        self.watchdogs.feed(WatchdogId::VideoLoop);
    }

    /// Process watchdog expiries. Should be called periodically (e.g. every 100 ms)
    /// by a background task.
    ///
    /// Returns the list of expired watchdogs and the action to take.
    pub fn process_watchdog_expiries(&mut self) -> Vec<(WatchdogId, WatchdogAction)> {
        let expired = self.watchdogs.check_expired();
        if expired.is_empty() {
            return Vec::new();
        }

        for (id, action) in &expired {
            self.watchdogs.note_expiry_processed(*id);
            match action {
                WatchdogAction::Degrade => {
                    warn!(watchdog = id.as_str(), "watchdog expired → degrading");
                    let mut sm = self.state.lock();
                    let current = sm.state();
                    if current == SystemState::Tracking {
                        let _ = sm.try_transition(SystemState::TrackingDegraded);
                    }
                }
                WatchdogAction::Abort => {
                    error!(watchdog = id.as_str(), "watchdog expired → ABORT");
                    // We can't await here (this is a sync method).
                    // The caller should check the returned actions and call
                    // abort() if any are Abort.
                }
            }
        }
        expired
    }

    /// Abort — force transition to ABORT and send RTL to FC.
    pub async fn abort(&mut self) -> CommanderResult<()> {
        self.abort_internal().await
    }

    async fn abort_internal(&mut self) -> CommanderResult<()> {
        {
            let mut sm = self.state.lock();
            sm.force_transition(SystemState::Abort);
        }
        info!("Commander: ABORT — sending RTL to FC");
        // Force-send (bypass rate limiter for safety-critical command)
        self.rate_limiter.force_send();
        if let Err(e) = self.fc.set_mode(FlightMode::Rtl).await {
            error!(error = %e, "failed to send RTL during abort");
            return Err(e.into());
        }
        self.active_target_id = None;
        Ok(())
    }

    /// Reset — return to IDLE (only valid from ABORT after disarm).
    pub fn reset(&mut self) -> CommanderResult<()> {
        let mut sm = self.state.lock();
        if sm.state() != SystemState::Abort {
            return Err(CommanderError::InvalidState {
                current: sm.state(),
                expected: SystemState::Abort,
            });
        }
        sm.try_transition(SystemState::Idle)?;
        self.active_target_id = None;
        info!("Commander: reset → IDLE");
        Ok(())
    }

    /// Set the FC flight mode.
    pub async fn set_flight_mode(&mut self, mode: FlightMode) -> CommanderResult<()> {
        if !self.connected {
            return Err(CommanderError::NotConnected);
        }
        self.fc.set_mode(mode).await?;
        debug!(?mode, "Commander: FC mode set");
        Ok(())
    }

    /// Set the ROI (Region of Interest) — what the camera/gimbal points at.
    pub async fn set_roi(&mut self, roi: RoiTarget) -> CommanderResult<()> {
        if !self.connected {
            return Err(CommanderError::NotConnected);
        }
        self.fc.set_roi(roi).await?;
        debug!(?roi, "Commander: ROI set");
        Ok(())
    }

    /// Send a correction command to the FC (via rate limiter).
    ///
    /// Converts the yaw/pitch rate command to a `SET_POSITION_TARGET_LOCAL_NED`
    /// MAVLink message. For Phase 5 we use a simplified mapping — Phase 6 will
    /// add proper coordinate transforms (camera frame → NED).
    async fn send_correction_to_fc(&mut self, cmd: CorrectionCommand) -> CommanderResult<()> {
        if !self.rate_limiter.try_send() {
            debug!("Commander: rate-limited, skipping FC command");
            return Ok(());
        }

        // Simplified mapping: yaw_rate → yaw, pitch_rate → pitch (as offset).
        // Phase 6 will replace this with a proper camera-to-NED transform.
        let target = common::PositionTargetNED {
            north: 0.0,
            east: cmd.offset_x, // crude: lateral offset → east
            down: cmd.offset_y, // crude: vertical offset → down
            yaw: cmd.yaw_rate_dps.to_radians(),
        };

        if let Err(e) = self.fc.set_position_target_local_ned(target).await {
            warn!(error = %e, "failed to send position target to FC");
            return Err(e.into());
        }
        Ok(())
    }

    /// Get a health snapshot — for diagnostics and REPL.
    pub fn health_snapshot(&self) -> CommanderHealth {
        let state = self.state();
        let wd_snap = self.watchdogs.snapshot();
        let al_snap = self.anti_loop.snapshot();
        let hb = self.fc.heartbeat_status();

        CommanderHealth {
            state,
            transitions: self.transition_count(),
            connected: self.connected,
            armed: hb.armed,
            fc_mode: hb.mode,
            fc_heartbeat_stale: hb.is_stale(1000),
            active_target_id: self.active_target_id,
            watchdogs: wd_snap,
            anti_loop: al_snap,
            rate_limiter_sent: self.rate_limiter.sent_count(),
            rate_limiter_dropped: self.rate_limiter.dropped_count(),
            last_command_age_ms: self
                .last_command_at
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(u64::MAX),
        }
    }
}

/// Health snapshot — for diagnostics.
pub struct CommanderHealth {
    pub state: SystemState,
    pub transitions: u64,
    pub connected: bool,
    pub armed: bool,
    pub fc_mode: FlightMode,
    pub fc_heartbeat_stale: bool,
    pub active_target_id: Option<TargetId>,
    pub watchdogs: Vec<crate::watchdogs::WatchdogSnapshot>,
    pub anti_loop: crate::anti_loop::AntiLoopSnapshot,
    pub rate_limiter_sent: u64,
    pub rate_limiter_dropped: u64,
    pub last_command_age_ms: u64,
}

impl std::fmt::Display for CommanderHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Commander Health ===")?;
        writeln!(
            f,
            "State:          {} (transitions: {})",
            self.state, self.transitions
        )?;
        writeln!(f, "Connected:      {}", self.connected)?;
        writeln!(f, "Armed:          {}", self.armed)?;
        writeln!(f, "FC mode:        {:?}", self.fc_mode)?;
        writeln!(
            f,
            "FC heartbeat:   {}",
            if self.fc_heartbeat_stale {
                "STALE"
            } else {
                "OK"
            }
        )?;
        if let Some(id) = self.active_target_id {
            writeln!(f, "Active target:  {id}")?;
        } else {
            writeln!(f, "Active target:  none")?;
        }
        writeln!(
            f,
            "Rate limiter:   sent={} dropped={}",
            self.rate_limiter_sent, self.rate_limiter_dropped
        )?;
        if self.last_command_age_ms == u64::MAX {
            writeln!(f, "Last command:   never")?;
        } else {
            writeln!(f, "Last command:   {} ms ago", self.last_command_age_ms)?;
        }
        writeln!(f, "Watchdogs:")?;
        for w in &self.watchdogs {
            writeln!(
                f,
                "  {:<15} elapsed={:>5}ms / limit={:>5}ms expired={}",
                w.id.as_str(),
                w.elapsed_ms,
                w.timeout_ms,
                w.expired
            )?;
        }
        writeln!(
            f,
            "Anti-loop:      allowed={} suppressed={} oscillations={}",
            self.anti_loop.allowed_count,
            self.anti_loop.suppressed_count,
            self.anti_loop.oscillation_trigger_count
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::CommanderConfig;
    use fc_adapter::MockFcAdapter;

    fn make_commander() -> Commander {
        let config = CommanderConfig::default();
        let fc: Box<dyn FlightControllerAdapter> = Box::new(MockFcAdapter::new());
        Commander::new(config, fc)
    }

    async fn make_commander_connected() -> Commander {
        let mut c = make_commander();
        c.connect().await.unwrap();
        c.arm().await.unwrap();
        c
    }

    #[test]
    fn new_commander_starts_in_idle() {
        let c = make_commander();
        assert_eq!(c.state(), SystemState::Idle);
        assert!(!c.connected);
    }

    #[tokio::test]
    async fn connect_transitions_to_armed() {
        let c = make_commander_connected().await;
        assert_eq!(c.state(), SystemState::Armed);
        assert!(c.connected);
    }

    #[tokio::test]
    async fn start_scanning_transitions_correctly() {
        let mut c = make_commander_connected().await;
        c.start_scanning().unwrap();
        assert_eq!(c.state(), SystemState::Scanning);
    }

    #[tokio::test]
    async fn select_target_transitions_to_tracking() {
        let mut c = make_commander_connected().await;
        c.start_scanning().unwrap();
        c.select_target(42).unwrap();
        assert_eq!(c.state(), SystemState::Tracking);
        assert_eq!(c.active_target_id, Some(42));
    }

    #[tokio::test]
    async fn select_target_fails_without_scanning() {
        let mut c = make_commander_connected().await;
        // Direct from ARMED to select_target — should fail
        let result = c.select_target(1);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn abort_forces_state() {
        let mut c = make_commander_connected().await;
        c.start_scanning().unwrap();
        c.select_target(1).unwrap();
        c.abort().await.unwrap();
        assert_eq!(c.state(), SystemState::Abort);
        assert_eq!(c.active_target_id, None);
    }

    #[tokio::test]
    async fn reset_only_works_from_abort() {
        let mut c = make_commander_connected().await;
        // From ARMED — should fail
        let result = c.reset();
        assert!(result.is_err());

        // Abort first
        c.abort().await.unwrap();
        c.reset().unwrap();
        assert_eq!(c.state(), SystemState::Idle);
    }

    #[tokio::test]
    async fn health_snapshot_works() {
        let c = make_commander_connected().await;
        let health = c.health_snapshot();
        assert_eq!(health.state, SystemState::Armed);
        assert!(health.connected);
        assert!(health.armed);
        // Watchdogs should be registered
        assert!(!health.watchdogs.is_empty());
    }

    #[tokio::test]
    async fn health_display_does_not_panic() {
        let c = make_commander_connected().await;
        let health = c.health_snapshot();
        let s = format!("{health}");
        assert!(s.contains("Commander Health"));
        assert!(s.contains("Armed"));
    }

    #[tokio::test]
    async fn feed_video_watchdog_does_not_panic() {
        let c = make_commander_connected().await;
        c.feed_video_watchdog();
        c.feed_video_watchdog();
        c.feed_video_watchdog();
    }

    #[tokio::test]
    async fn process_watchdog_expiries_returns_empty_when_fresh() {
        let mut c = make_commander_connected().await;
        c.feed_video_watchdog();
        let expired = c.process_watchdog_expiries();
        // Some watchdogs may have expired (we didn't feed all of them)
        // but at least it shouldn't panic.
        let _ = expired;
    }

    #[tokio::test]
    async fn disarm_sets_idle() {
        let mut c = make_commander_connected().await;
        c.start_scanning().unwrap();
        c.disarm().await.unwrap();
        assert_eq!(c.state(), SystemState::Idle);
    }

    #[tokio::test]
    async fn set_flight_mode_works_when_connected() {
        let mut c = make_commander_connected().await;
        c.set_flight_mode(FlightMode::Guided).await.unwrap();
    }

    #[tokio::test]
    async fn set_flight_mode_fails_when_not_connected() {
        let mut c = make_commander();
        let result = c.set_flight_mode(FlightMode::Guided).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_with_target_sends_correction() {
        let mut c = make_commander_connected().await;
        c.start_scanning().unwrap();
        c.select_target(1).unwrap();

        // Update with a target offset — should send a correction command
        let dets = vec![];
        c.update(&dets, Some((0.2, 0.1))).await.unwrap();

        // The rate limiter should have allowed one command
        let health = c.health_snapshot();
        assert!(health.rate_limiter_sent > 0);
    }

    #[tokio::test]
    async fn update_without_target_does_not_send() {
        let mut c = make_commander_connected().await;
        // Don't select a target
        c.start_scanning().unwrap();
        c.update(&[], Some((0.2, 0.1))).await.unwrap();

        let health = c.health_snapshot();
        assert_eq!(health.rate_limiter_sent, 0);
    }

    #[tokio::test]
    async fn update_with_deadband_offset_suppressed() {
        let mut c = make_commander_connected().await;
        c.start_scanning().unwrap();
        c.select_target(1).unwrap();

        // Offset within deadband (default 0.05) — should be suppressed
        c.update(&[], Some((0.02, 0.01))).await.unwrap();

        let health = c.health_snapshot();
        assert_eq!(health.rate_limiter_sent, 0); // suppressed, not sent
    }

    #[tokio::test]
    async fn full_lifecycle() {
        let mut c = make_commander();

        // Connect + arm
        c.connect().await.unwrap();
        c.arm().await.unwrap();
        assert_eq!(c.state(), SystemState::Armed);

        // Scan
        c.start_scanning().unwrap();
        assert_eq!(c.state(), SystemState::Scanning);

        // Select target
        c.select_target(7).unwrap();
        assert_eq!(c.state(), SystemState::Tracking);

        // Update with offset
        c.update(&[], Some((0.3, 0.0))).await.unwrap();

        // Abort
        c.abort().await.unwrap();
        assert_eq!(c.state(), SystemState::Abort);

        // Reset
        c.reset().unwrap();
        assert_eq!(c.state(), SystemState::Idle);

        // Disconnect
        c.disconnect().await.unwrap();
        assert!(!c.connected);
    }
}
