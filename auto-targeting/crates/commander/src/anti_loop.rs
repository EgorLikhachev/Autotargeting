//! Anti-loop protection — deadband, hysteresis, bounding limits, oscillation detector.
//!
//! Implements Levels 3 and 5 of the anti-loop protection stack described in
//! `docs/ARCHITECTURE.md` §1.4:
//!
//! - **Level 3 — Deadband & hysteresis:** if target offset is within `deadband_fraction`
//!   of the frame center, no correction command is sent. This eliminates micro-jitter.
//! - **Level 3 — Bounding limits:** yaw/pitch rate commands are clipped to
//!   `max_yaw_rate_dps` / `max_pitch_rate_dps`. Target offsets above `max_offset_fraction`
//!   are clipped and logged.
//! - **Level 5 — Oscillation detector:** maintains a ring buffer of recent yaw commands
//!   and computes the sign-change rate. If sign changes exceed `oscillation_threshold`,
//!   commands are frozen for a cooldown period. Repeated triggers escalate to ABORT.

use common::CommanderConfig;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// A yaw/pitch correction command produced by the tracker, destined for the FC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorrectionCommand {
    /// Yaw rate in degrees per second. Positive = clockwise.
    pub yaw_rate_dps: f32,
    /// Pitch rate in degrees per second. Positive = nose up.
    pub pitch_rate_dps: f32,
    /// Target offset as a fraction of frame width/height ([-1.0, 1.0]).
    pub offset_x: f32,
    pub offset_y: f32,
    /// When this command was generated.
    pub generated_at: Instant,
}

impl CorrectionCommand {
    pub fn zero() -> Self {
        Self {
            yaw_rate_dps: 0.0,
            pitch_rate_dps: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            generated_at: Instant::now(),
        }
    }
}

/// Result of passing a command through the anti-loop guard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GuardDecision {
    /// Command is allowed through (possibly clipped).
    Allow(CorrectionCommand),
    /// Command was suppressed (within deadband, or frozen by oscillation detector).
    Suppress,
    /// Oscillation detected — commander should transition to TRACKING_DEGRADED.
    Degrade,
    /// Repeated oscillation — commander should transition to ABORT.
    Abort,
}

/// The anti-loop guard — wraps the rate-limited command stream.
///
/// Stateful: holds the oscillation ring buffer and freeze state.
pub struct AntiLoopGuard {
    config: CommanderConfig,
    inner: Mutex<AntiLoopState>,
}

#[derive(Debug)]
struct AntiLoopState {
    /// Ring buffer of recent yaw command signs (true = positive, false = negative).
    /// Length = `config.oscillation_window`.
    yaw_sign_history: VecDeque<bool>,
    /// True if commands are currently frozen (oscillation cooldown).
    frozen: bool,
    /// When the current freeze ends.
    freeze_until: Option<Instant>,
    /// Number of oscillation triggers in the current 5-second window.
    recent_triggers: Vec<Instant>,
    /// Total commands suppressed since startup.
    suppressed_count: u64,
    /// Total commands allowed since startup.
    allowed_count: u64,
    /// Total oscillation triggers since startup.
    oscillation_trigger_count: u64,
}

impl AntiLoopGuard {
    pub fn new(config: CommanderConfig) -> Self {
        let window = config.oscillation_window;
        Self {
            config,
            inner: Mutex::new(AntiLoopState {
                yaw_sign_history: VecDeque::with_capacity(window),
                frozen: false,
                freeze_until: None,
                recent_triggers: Vec::new(),
                suppressed_count: 0,
                allowed_count: 0,
                oscillation_trigger_count: 0,
            }),
        }
    }

    /// Process a candidate command. Returns the decision on what to do.
    pub fn process(&self, cmd: CorrectionCommand) -> GuardDecision {
        let mut state = self.inner.lock();

        // Check if we're in a freeze period
        let now = Instant::now();
        if let Some(until) = state.freeze_until {
            if now < until {
                state.suppressed_count += 1;
                debug!("command suppressed (frozen until freeze ends)");
                return GuardDecision::Suppress;
            } else {
                // Freeze period over
                state.frozen = false;
                state.freeze_until = None;
            }
        }

        // Deadband check: if offset is within deadband, suppress
        let deadband = self.config.deadband_fraction;
        if cmd.offset_x.abs() < deadband && cmd.offset_y.abs() < deadband {
            state.suppressed_count += 1;
            debug!(
                offset_x = cmd.offset_x,
                offset_y = cmd.offset_y,
                "command suppressed (within deadband)"
            );
            return GuardDecision::Suppress;
        }

        // Bounding limits: clip yaw/pitch rates
        let max_yaw = self.config.max_yaw_rate_dps;
        let max_pitch = self.config.max_pitch_rate_dps;
        let mut clipped = cmd;
        if clipped.yaw_rate_dps.abs() > max_yaw {
            warn!(
                original = clipped.yaw_rate_dps,
                max = max_yaw,
                "yaw rate clipped to bounding limit"
            );
            clipped.yaw_rate_dps = clipped.yaw_rate_dps.signum() * max_yaw;
        }
        if clipped.pitch_rate_dps.abs() > max_pitch {
            warn!(
                original = clipped.pitch_rate_dps,
                max = max_pitch,
                "pitch rate clipped to bounding limit"
            );
            clipped.pitch_rate_dps = clipped.pitch_rate_dps.signum() * max_pitch;
        }

        // Clip target offset to max_offset_fraction
        let max_offset = self.config.max_offset_fraction;
        if clipped.offset_x.abs() > max_offset {
            warn!(
                original = clipped.offset_x,
                max = max_offset,
                "offset_x clipped"
            );
            clipped.offset_x = clipped.offset_x.signum() * max_offset;
        }
        if clipped.offset_y.abs() > max_offset {
            warn!(
                original = clipped.offset_y,
                max = max_offset,
                "offset_y clipped"
            );
            clipped.offset_y = clipped.offset_y.signum() * max_offset;
        }

        // Oscillation detector: track sign changes of yaw commands
        let yaw_sign = clipped.yaw_rate_dps > 0.0;
        if !state.yaw_sign_history.is_empty() {
            let prev = *state.yaw_sign_history.back().unwrap();
            if prev != yaw_sign && clipped.yaw_rate_dps.abs() > 0.1 {
                // Sign change detected
                let sign_changes = count_sign_changes(&state.yaw_sign_history, yaw_sign);
                let window_size = state.yaw_sign_history.len() as f32;
                if window_size > 0.0 {
                    let change_rate = sign_changes as f32 / window_size;
                    if change_rate > self.config.oscillation_threshold {
                        // Oscillation detected!
                        return self.handle_oscillation(&mut state, clipped);
                    }
                }
            }
        }

        // Push to history (maintain window size)
        state.yaw_sign_history.push_back(yaw_sign);
        if state.yaw_sign_history.len() > self.config.oscillation_window {
            state.yaw_sign_history.pop_front();
        }

        state.allowed_count += 1;
        GuardDecision::Allow(clipped)
    }

    fn handle_oscillation(
        &self,
        state: &mut AntiLoopState,
        cmd: CorrectionCommand,
    ) -> GuardDecision {
        state.oscillation_trigger_count += 1;
        let now = Instant::now();

        // Prune recent_triggers older than 5 seconds
        state
            .recent_triggers
            .retain(|t| now.duration_since(*t) < Duration::from_secs(5));

        state.recent_triggers.push(now);

        warn!(
            triggers_in_5s = state.recent_triggers.len(),
            total_triggers = state.oscillation_trigger_count,
            "oscillation detected — freezing commands for 1 second"
        );

        // Freeze for 1 second
        state.frozen = true;
        state.freeze_until = Some(now + Duration::from_secs(1));

        // Clear history to give the system a fresh start after freeze
        state.yaw_sign_history.clear();

        if state.recent_triggers.len() as u32 >= self.config.oscillation_abort_count {
            tracing::error!(
                triggers = state.recent_triggers.len(),
                threshold = self.config.oscillation_abort_count,
                "oscillation trigger count exceeded — escalating to ABORT"
            );
            return GuardDecision::Abort;
        }

        let _ = cmd; // suppress the command that triggered this
        GuardDecision::Degrade
    }

    /// Get a health snapshot.
    pub fn snapshot(&self) -> AntiLoopSnapshot {
        let s = self.inner.lock();
        AntiLoopSnapshot {
            frozen: s.frozen,
            suppressed_count: s.suppressed_count,
            allowed_count: s.allowed_count,
            oscillation_trigger_count: s.oscillation_trigger_count,
            history_len: s.yaw_sign_history.len() as u32,
        }
    }
}

fn count_sign_changes(history: &VecDeque<bool>, next: bool) -> usize {
    if history.is_empty() {
        return 0;
    }
    let mut changes = 0;
    let mut prev = *history.front().unwrap();
    for &v in history.iter().skip(1) {
        if v != prev {
            changes += 1;
        }
        prev = v;
    }
    if next != prev {
        changes += 1;
    }
    changes
}

#[derive(Debug, Clone, Serialize)]
pub struct AntiLoopSnapshot {
    pub frozen: bool,
    pub suppressed_count: u64,
    pub allowed_count: u64,
    pub oscillation_trigger_count: u64,
    pub history_len: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CommanderConfig {
        CommanderConfig {
            deadband_fraction: 0.05,
            max_yaw_rate_dps: 30.0,
            max_pitch_rate_dps: 15.0,
            max_offset_fraction: 0.30,
            oscillation_window: 10, // smaller for tests
            oscillation_threshold: 0.5,
            oscillation_abort_count: 3,
            ..Default::default()
        }
    }

    fn cmd(yaw: f32, pitch: f32, ox: f32, oy: f32) -> CorrectionCommand {
        CorrectionCommand {
            yaw_rate_dps: yaw,
            pitch_rate_dps: pitch,
            offset_x: ox,
            offset_y: oy,
            generated_at: Instant::now(),
        }
    }

    #[test]
    fn suppresses_within_deadband() {
        let guard = AntiLoopGuard::new(cfg());
        // Offset within 5% deadband → suppress
        let decision = guard.process(cmd(10.0, 0.0, 0.02, 0.03));
        assert_eq!(decision, GuardDecision::Suppress);
    }

    #[test]
    fn allows_outside_deadband() {
        let guard = AntiLoopGuard::new(cfg());
        let decision = guard.process(cmd(10.0, 0.0, 0.2, 0.1));
        assert!(matches!(decision, GuardDecision::Allow(_)));
    }

    #[test]
    fn clips_yaw_rate() {
        let guard = AntiLoopGuard::new(cfg());
        let decision = guard.process(cmd(100.0, 0.0, 0.2, 0.1));
        if let GuardDecision::Allow(c) = decision {
            assert!(
                (c.yaw_rate_dps - 30.0).abs() < 1e-6,
                "yaw should be clipped to 30"
            );
        } else {
            panic!("expected Allow, got {:?}", decision);
        }
    }

    #[test]
    fn clips_negative_yaw_rate() {
        let guard = AntiLoopGuard::new(cfg());
        let decision = guard.process(cmd(-100.0, 0.0, 0.2, 0.1));
        if let GuardDecision::Allow(c) = decision {
            assert!((c.yaw_rate_dps - (-30.0)).abs() < 1e-6);
        } else {
            panic!("expected Allow");
        }
    }

    #[test]
    fn detects_oscillation_on_sign_changes() {
        let guard = AntiLoopGuard::new(cfg());
        // Fill the window with alternating signs to trigger oscillation
        // Use offsets large enough to escape deadband
        for _ in 0..3 {
            guard.process(cmd(20.0, 0.0, 0.2, 0.1)); // positive
            guard.process(cmd(-20.0, 0.0, -0.2, -0.1)); // negative
        }
        let snap = guard.snapshot();
        assert!(
            snap.oscillation_trigger_count >= 1,
            "should have triggered at least once, got {}",
            snap.oscillation_trigger_count
        );
    }

    #[test]
    fn freeze_suppresses_subsequent_commands() {
        let guard = AntiLoopGuard::new(cfg());
        // Trigger oscillation
        for _ in 0..3 {
            guard.process(cmd(20.0, 0.0, 0.2, 0.1));
            guard.process(cmd(-20.0, 0.0, -0.2, -0.1));
        }
        // Subsequent commands should be suppressed (frozen)
        let decision = guard.process(cmd(10.0, 0.0, 0.2, 0.1));
        assert_eq!(decision, GuardDecision::Suppress);
    }

    #[test]
    fn does_not_trigger_on_steady_direction() {
        let guard = AntiLoopGuard::new(cfg());
        // All same sign — no oscillation
        for _ in 0..15 {
            let d = guard.process(cmd(15.0, 0.0, 0.2, 0.1));
            assert!(matches!(d, GuardDecision::Allow(_)));
        }
        let snap = guard.snapshot();
        assert_eq!(snap.oscillation_trigger_count, 0);
    }

    #[test]
    fn clips_offset_to_max() {
        let guard = AntiLoopGuard::new(cfg());
        // Offset 0.9 > max 0.30
        let decision = guard.process(cmd(10.0, 0.0, 0.9, 0.9));
        if let GuardDecision::Allow(c) = decision {
            assert!((c.offset_x - 0.30).abs() < 1e-6);
            assert!((c.offset_y - 0.30).abs() < 1e-6);
        } else {
            panic!("expected Allow, got {:?}", decision);
        }
    }
}
