//! Token-bucket rate limiter for FC commands.
//!
//! Per the architecture (§1.4, Level 4): MAVLink commands to the FC are sent
//! at a fixed frequency (default 10 Hz) regardless of how often the commander
//! wants to send them. This prevents FC overload and stabilizes the control
//! loop. Excess commands are dropped (not queued) — the latest state is what
//! matters, not the queue depth.

use std::time::{Duration, Instant};
use tracing::warn;

pub struct CommandRateLimiter {
    /// Minimum interval between allowed commands.
    min_interval: Duration,
    /// Time of the last command that was allowed through.
    last_sent: Option<Instant>,
    /// Count of dropped commands since startup.
    dropped_count: u64,
    /// Count of allowed commands since startup.
    sent_count: u64,
}

impl CommandRateLimiter {
    /// Create a limiter that allows at most `rate_hz` commands per second.
    pub fn new(rate_hz: u32) -> Self {
        assert!(rate_hz > 0, "rate_hz must be > 0");
        Self {
            min_interval: Duration::from_secs_f64(1.0 / rate_hz as f64),
            last_sent: None,
            dropped_count: 0,
            sent_count: 0,
        }
    }

    /// Returns `true` if a command can be sent now (and updates internal state).
    /// Returns `false` if the command should be dropped (too soon since last).
    pub fn try_send(&mut self) -> bool {
        let now = Instant::now();
        match self.last_sent {
            None => {
                self.last_sent = Some(now);
                self.sent_count += 1;
                true
            }
            Some(last) => {
                if now.duration_since(last) >= self.min_interval {
                    self.last_sent = Some(now);
                    self.sent_count += 1;
                    true
                } else {
                    self.dropped_count += 1;
                    if self.dropped_count % 100 == 0 {
                        warn!(
                            dropped = self.dropped_count,
                            sent = self.sent_count,
                            "rate limiter has dropped {} commands ({} sent) — downstream is producing faster than FC can consume",
                            self.dropped_count,
                            self.sent_count
                        );
                    }
                    false
                }
            }
        }
    }

    /// Force-send (bypass rate limiting) — use only for safety-critical
    /// commands like ABORT / disarm.
    pub fn force_send(&mut self) {
        self.last_sent = Some(Instant::now());
        self.sent_count += 1;
    }

    pub fn sent_count(&self) -> u64 {
        self.sent_count
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn allows_first_command() {
        let mut rl = CommandRateLimiter::new(10);
        assert!(rl.try_send());
        assert_eq!(rl.sent_count(), 1);
    }

    #[test]
    fn drops_within_interval() {
        let mut rl = CommandRateLimiter::new(10); // 100ms interval
        assert!(rl.try_send());
        // Immediate retry should be dropped
        assert!(!rl.try_send());
        assert!(!rl.try_send());
        assert_eq!(rl.dropped_count(), 2);
        assert_eq!(rl.sent_count(), 1);
    }

    #[test]
    fn allows_after_interval() {
        let mut rl = CommandRateLimiter::new(50); // 20ms interval
        assert!(rl.try_send());
        sleep(Duration::from_millis(25));
        assert!(rl.try_send());
        assert_eq!(rl.sent_count(), 2);
    }

    #[test]
    fn force_send_bypasses_limit() {
        let mut rl = CommandRateLimiter::new(10);
        assert!(rl.try_send());
        rl.force_send();
        rl.force_send();
        assert_eq!(rl.sent_count(), 3);
    }
}
