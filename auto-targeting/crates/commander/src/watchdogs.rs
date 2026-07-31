//! Watchdog registry — anti-loop protection level 1.
//!
//! Each async loop (video, inference, tracking, command) registers a watchdog
//! and must "feed" (reset) it on every iteration. If a watchdog is not fed
//! within its timeout, the commander is notified and transitions to a
//! degraded or ABORT state.
//!
//! See `docs/ARCHITECTURE.md` §1.4 (Level 1 — Per-loop Watchdog Timers).

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tracing::warn;

/// Identifier for a watchdog. Adding a new loop = add a variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum WatchdogId {
    VideoLoop,
    InferenceLoop,
    TrackingLoop,
    CommandLoop,
    FcHeartbeat,
}

impl WatchdogId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::VideoLoop => "video_loop",
            Self::InferenceLoop => "inference_loop",
            Self::TrackingLoop => "tracking_loop",
            Self::CommandLoop => "command_loop",
            Self::FcHeartbeat => "fc_heartbeat",
        }
    }
}

/// Per-watchdog configuration: timeout and what state to transition to on expiry.
#[derive(Debug, Clone, Copy)]
pub struct WatchdogConfig {
    pub timeout: Duration,
    pub on_expiry: WatchdogAction,
}

/// What the commander should do when this watchdog expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    /// Transition to TRACKING_DEGRADED (continue operating, but flag the issue).
    Degrade,
    /// Transition to ABORT and trigger RTH.
    Abort,
}

impl WatchdogConfig {
    pub fn new(timeout: Duration, action: WatchdogAction) -> Self {
        Self {
            timeout,
            on_expiry: action,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WatchdogEntry {
    last_fed: DateTime<Utc>,
    config: WatchdogConfig,
    /// Number of times this watchdog has expired since startup.
    expiry_count: u64,
    /// Number of times this watchdog has been fed since startup.
    feed_count: u64,
}

/// Thread-safe registry of all watchdogs. Shared between the watchdog checker
/// task and the loops that feed them.
#[derive(Debug)]
pub struct WatchdogRegistry {
    inner: Mutex<HashMap<WatchdogId, WatchdogEntry>>,
}

impl WatchdogRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new watchdog. If already registered, overwrites config.
    pub fn register(&self, id: WatchdogId, config: WatchdogConfig) {
        let mut inner = self.inner.lock();
        inner.insert(
            id,
            WatchdogEntry {
                last_fed: Utc::now(),
                config,
                expiry_count: 0,
                feed_count: 0,
            },
        );
        tracing::info!(
            watchdog = id.as_str(),
            timeout_ms = config.timeout.as_millis(),
            action = ?config.on_expiry,
            "registered watchdog"
        );
    }

    /// Feed (reset) a watchdog. Called by the corresponding loop on each
    /// iteration. If the watchdog is not registered, this is a no-op (with warn).
    pub fn feed(&self, id: WatchdogId) {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.get_mut(&id) {
            entry.last_fed = Utc::now();
            entry.feed_count += 1;
        } else {
            warn!(
                watchdog = id.as_str(),
                "feed called on unregistered watchdog — ignored"
            );
        }
    }

    /// Check all watchdogs for expiry. Returns a list of (id, action) pairs
    /// for watchdogs that have expired since the last check.
    ///
    /// Note: expired watchdogs are NOT auto-reset here. They will keep
    /// returning as expired until `feed()` is called on them (which happens
    /// when the corresponding loop recovers).
    pub fn check_expired(&self) -> Vec<(WatchdogId, WatchdogAction)> {
        let now = Utc::now();
        let inner = self.inner.lock();
        let mut expired = Vec::new();
        for (id, entry) in inner.iter() {
            let elapsed = (now - entry.last_fed).num_milliseconds().max(0) as u64;
            if elapsed > entry.config.timeout.as_millis() as u64 {
                tracing::error!(
                    watchdog = id.as_str(),
                    elapsed_ms = elapsed,
                    limit_ms = entry.config.timeout.as_millis(),
                    "watchdog expired"
                );
                expired.push((*id, entry.config.on_expiry));
            }
        }
        expired
    }

    /// Get a snapshot of all watchdog statuses (for health reporting).
    pub fn snapshot(&self) -> Vec<WatchdogSnapshot> {
        let now = Utc::now();
        let inner = self.inner.lock();
        inner
            .iter()
            .map(|(id, entry)| {
                let elapsed_ms = (now - entry.last_fed).num_milliseconds().max(0) as u64;
                WatchdogSnapshot {
                    id: *id,
                    elapsed_ms,
                    timeout_ms: entry.config.timeout.as_millis() as u64,
                    expired: elapsed_ms > entry.config.timeout.as_millis() as u64,
                    expiry_count: entry.expiry_count,
                    feed_count: entry.feed_count,
                }
            })
            .collect()
    }

    /// Increment the expiry counter for a watchdog (called when commander
    /// processes an expiry notification).
    pub fn note_expiry_processed(&self, id: WatchdogId) {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.get_mut(&id) {
            entry.expiry_count += 1;
        }
    }
}

impl Default for WatchdogRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WatchdogSnapshot {
    pub id: WatchdogId,
    pub elapsed_ms: u64,
    pub timeout_ms: u64,
    pub expired: bool,
    pub expiry_count: u64,
    pub feed_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn cfg(timeout_ms: u64, action: WatchdogAction) -> WatchdogConfig {
        WatchdogConfig::new(Duration::from_millis(timeout_ms), action)
    }

    #[test]
    fn fresh_watchdog_not_expired() {
        let reg = WatchdogRegistry::new();
        reg.register(WatchdogId::VideoLoop, cfg(100, WatchdogAction::Degrade));
        let expired = reg.check_expired();
        assert!(expired.is_empty());
    }

    #[test]
    fn unfed_watchdog_expires() {
        let reg = WatchdogRegistry::new();
        reg.register(WatchdogId::VideoLoop, cfg(50, WatchdogAction::Degrade));
        sleep(Duration::from_millis(80));
        let expired = reg.check_expired();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, WatchdogId::VideoLoop);
        assert_eq!(expired[0].1, WatchdogAction::Degrade);
    }

    #[test]
    fn feeding_resets_watchdog() {
        let reg = WatchdogRegistry::new();
        reg.register(WatchdogId::VideoLoop, cfg(50, WatchdogAction::Degrade));
        sleep(Duration::from_millis(30));
        reg.feed(WatchdogId::VideoLoop);
        sleep(Duration::from_millis(30));
        let expired = reg.check_expired();
        assert!(expired.is_empty(), "should not be expired after feed");
    }

    #[test]
    fn multiple_watchdogs_independent() {
        let reg = WatchdogRegistry::new();
        reg.register(WatchdogId::VideoLoop, cfg(30, WatchdogAction::Degrade));
        reg.register(WatchdogId::InferenceLoop, cfg(100, WatchdogAction::Abort));

        sleep(Duration::from_millis(40));
        reg.feed(WatchdogId::InferenceLoop); // inference is healthy

        let expired = reg.check_expired();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, WatchdogId::VideoLoop);
    }

    #[test]
    fn snapshot_reports_all_watchdogs() {
        let reg = WatchdogRegistry::new();
        reg.register(WatchdogId::VideoLoop, cfg(100, WatchdogAction::Degrade));
        reg.register(WatchdogId::FcHeartbeat, cfg(1000, WatchdogAction::Abort));

        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap.iter().any(|s| s.id == WatchdogId::VideoLoop));
        assert!(snap.iter().any(|s| s.id == WatchdogId::FcHeartbeat));
    }

    #[test]
    fn feed_unknown_watchdog_is_warn_noop() {
        let reg = WatchdogRegistry::new();
        // Should not panic
        reg.feed(WatchdogId::VideoLoop);
        let expired = reg.check_expired();
        assert!(expired.is_empty());
    }

    #[test]
    fn feed_count_increments() {
        let reg = WatchdogRegistry::new();
        reg.register(WatchdogId::VideoLoop, cfg(100, WatchdogAction::Degrade));
        reg.feed(WatchdogId::VideoLoop);
        reg.feed(WatchdogId::VideoLoop);
        reg.feed(WatchdogId::VideoLoop);
        let snap = reg.snapshot();
        let v = snap.iter().find(|s| s.id == WatchdogId::VideoLoop).unwrap();
        assert_eq!(v.feed_count, 3);
    }
}
