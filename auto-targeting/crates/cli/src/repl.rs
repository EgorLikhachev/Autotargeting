//! Interactive REPL (Read-Eval-Print Loop) for operator commands.
//!
//! Provides a command-line interface for an operator to control the running
//! auto-targeting system in real time. Commands:
//!
//! - `help` — list available commands
//! - `status` — print current system state (state machine, watchdogs, FC)
//! - `arm` — arm the drone
//! - `disarm` — disarm the drone
//! - `set-mode <mode>` — change FC flight mode (guided, rtl, loiter, manual, auto)
//! - `select-target <id>` — select a target to track
//! - `abort` — trigger ABORT (RTH)
//! - `reset` — return to IDLE (only after ABORT + disarm)
//! - `scan` — start scanning for targets
//! - `watchdogs` — print watchdog statuses
//! - `quit` / `exit` — shutdown the system
//!
//! ## Usage
//!
//! The REPL runs in its own tokio task. Commands are dispatched to the
//! commander state machine and the FC adapter. State changes are logged.
//!
//! ```ignore
//! auto-targeting> status
//! State: TRACKING
//! FC: armed=true mode=Guided
//! Watchdogs: video_loop=OK fc_heartbeat=OK
//! ```

use crate::operator::OperatorCommand;
use anyhow::{anyhow, Result};
use chrono::Utc;
use common::{FlightMode, SystemState};
use fc_adapter::{FlightControllerAdapter, MockFcAdapter};
use parking_lot::Mutex as PlMutex;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tracing::warn;

/// Shared system context — what the REPL has access to.
///
/// The state machine uses `parking_lot::Mutex` (sync, never held across await).
/// The FC adapter uses `tokio::sync::Mutex` (async, because FC methods are async
/// and we need to hold the lock while awaiting).
pub struct ReplContext {
    pub state: Arc<PlMutex<commander::StateMachine>>,
    pub fc: Arc<TokioMutex<MockFcAdapter>>,
    pub watchdogs: Arc<commander::WatchdogRegistry>,
    pub anti_loop: Arc<commander::AntiLoopGuard>,
}

impl ReplContext {
    pub fn new(
        state: Arc<PlMutex<commander::StateMachine>>,
        fc: Arc<TokioMutex<MockFcAdapter>>,
        watchdogs: Arc<commander::WatchdogRegistry>,
        anti_loop: Arc<commander::AntiLoopGuard>,
    ) -> Self {
        Self {
            state,
            fc,
            watchdogs,
            anti_loop,
        }
    }
}

/// Run the REPL loop. Blocks until `quit` is entered or EOF.
pub async fn run_repl(ctx: ReplContext) -> Result<()> {
    let mut rl = DefaultEditor::new()?;

    println!("\n=== Auto-Targeting Operator Console ===");
    println!("Type 'help' for available commands.\n");

    loop {
        let readline = rl.readline("auto-targeting> ");
        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line);
                if let Err(e) = handle_command(line, &ctx).await {
                    println!("ERROR: {e}");
                }
                if line == "quit" || line == "exit" {
                    println!("Shutting down...");
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C — type 'quit' to exit cleanly");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("CTRL-D — exiting");
                break;
            }
            Err(e) => {
                warn!(error = %e, "readline error");
                break;
            }
        }
    }
    Ok(())
}

/// Parse and execute a single command.
async fn handle_command(input: &str, ctx: &ReplContext) -> Result<()> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(());
    }
    let cmd = parts[0];
    let args = &parts[1..];

    match cmd {
        "help" | "h" | "?" => {
            print_help();
            Ok(())
        }
        "status" | "st" => {
            print_status(ctx).await;
            Ok(())
        }
        "arm" => {
            let mut fc = ctx.fc.lock().await;
            fc.arm().await.map_err(|e| anyhow!("arm failed: {e}"))?;
            println!("OK: armed");
            // Try state transition too
            let mut sm = ctx.state.lock();
            let _ = sm.try_transition(SystemState::Armed);
            Ok(())
        }
        "disarm" => {
            let mut fc = ctx.fc.lock().await;
            fc.disarm()
                .await
                .map_err(|e| anyhow!("disarm failed: {e}"))?;
            println!("OK: disarmed");
            // Transition back to IDLE
            let mut sm = ctx.state.lock();
            let _ = sm.try_transition(SystemState::Idle);
            Ok(())
        }
        "set-mode" | "mode" => {
            if args.is_empty() {
                return Err(anyhow!("usage: set-mode <guided|rtl|loiter|manual|auto>"));
            }
            let mode = parse_flight_mode(args[0])?;
            let mut fc = ctx.fc.lock().await;
            fc.set_mode(mode)
                .await
                .map_err(|e| anyhow!("set_mode failed: {e}"))?;
            println!("OK: mode set to {mode:?}");
            Ok(())
        }
        "scan" => {
            let mut sm = ctx.state.lock();
            sm.try_transition(SystemState::Scanning)
                .map_err(|e| anyhow!("state transition failed: {e}"))?;
            println!("OK: scanning for targets");
            Ok(())
        }
        "select-target" | "select" => {
            if args.is_empty() {
                return Err(anyhow!("usage: select-target <id>"));
            }
            let id: u64 = args[0]
                .parse()
                .map_err(|e| anyhow!("invalid target id '{}': {e}", args[0]))?;
            let mut sm = ctx.state.lock();
            // From SCANNING we go to TARGET_SELECTED
            sm.try_transition(SystemState::TargetSelected)
                .map_err(|e| anyhow!("state transition failed: {e}"))?;
            println!("OK: target {id} selected — transition to TARGET_SELECTED");
            // Simulate lock acquisition
            sm.try_transition(SystemState::Tracking)
                .map_err(|e| anyhow!("lock acquisition failed: {e}"))?;
            println!("OK: lock acquired — now TRACKING target {id}");
            // Record the command for posterity
            let _ = OperatorCommand::SelectTarget { target_id: id };
            Ok(())
        }
        "abort" => {
            // Force transition — ABORT is allowed from any state.
            // Drop the sm guard before awaiting FC commands.
            {
                let mut sm = ctx.state.lock();
                sm.force_transition(SystemState::Abort);
                println!("!! ABORT !! — state set to ABORT");
            }

            // Try to send RTL to FC (force-send bypasses rate limit)
            let mut fc = ctx.fc.lock().await;
            match fc.set_mode(FlightMode::Rtl).await {
                Ok(()) => println!("OK: RTL command sent to FC"),
                Err(e) => println!("WARN: failed to send RTL: {e}"),
            }
            Ok(())
        }
        "reset" => {
            let mut sm = ctx.state.lock();
            if sm.state() != SystemState::Abort {
                return Err(anyhow!(
                    "reset only valid from ABORT state (current: {})",
                    sm.state()
                ));
            }
            sm.try_transition(SystemState::Idle)
                .map_err(|e| anyhow!("reset failed: {e}"))?;
            println!("OK: reset to IDLE");
            Ok(())
        }
        "watchdogs" | "wd" => {
            print_watchdogs(ctx);
            Ok(())
        }
        "anti-loop" | "al" => {
            print_anti_loop(ctx);
            Ok(())
        }
        "feed-watchdog" => {
            if args.is_empty() {
                return Err(anyhow!(
                    "usage: feed-watchdog <video|inference|tracking|command|heartbeat>"
                ));
            }
            let wd = match args[0] {
                "video" => commander::WatchdogId::VideoLoop,
                "inference" => commander::WatchdogId::InferenceLoop,
                "tracking" => commander::WatchdogId::TrackingLoop,
                "command" => commander::WatchdogId::CommandLoop,
                "heartbeat" | "fc" => commander::WatchdogId::FcHeartbeat,
                other => return Err(anyhow!("unknown watchdog: {other}")),
            };
            ctx.watchdogs.feed(wd);
            println!("OK: fed {wd:?}");
            Ok(())
        }
        "simulate-heartbeat-loss" => {
            // Test helper: simulate FC heartbeat loss
            let fc = ctx.fc.lock().await;
            fc.simulate_heartbeat_loss();
            println!("OK: simulated heartbeat loss — watchdog should trigger soon");
            Ok(())
        }
        "simulate-attitude" => {
            if args.len() < 3 {
                return Err(anyhow!("usage: simulate-attitude <roll> <pitch> <yaw>"));
            }
            let roll: f32 = args[0].parse().map_err(|e| anyhow!("roll: {e}"))?;
            let pitch: f32 = args[1].parse().map_err(|e| anyhow!("pitch: {e}"))?;
            let yaw: f32 = args[2].parse().map_err(|e| anyhow!("yaw: {e}"))?;
            let fc = ctx.fc.lock().await;
            fc.simulate_attitude(common::Attitude {
                roll,
                pitch,
                yaw,
                ..Default::default()
            });
            println!("OK: simulated attitude roll={roll} pitch={pitch} yaw={yaw}");
            Ok(())
        }
        "quit" | "exit" | "q" => {
            println!("Bye.");
            Ok(())
        }
        other => Err(anyhow!("unknown command: '{other}' — type 'help' for list")),
    }
}

fn parse_flight_mode(s: &str) -> Result<FlightMode> {
    Ok(match s.to_lowercase().as_str() {
        "guided" => FlightMode::Guided,
        "rtl" => FlightMode::Rtl,
        "loiter" => FlightMode::Loiter,
        "manual" => FlightMode::Manual,
        "auto" => FlightMode::Auto,
        "stabilize" | "stab" => FlightMode::Stabilize,
        other => return Err(anyhow!("unknown flight mode: {other}")),
    })
}

fn print_help() {
    println!("Available commands:");
    println!("  help                       — show this help");
    println!("  status                     — show system state");
    println!("  arm                        — arm the drone");
    println!("  disarm                     — disarm the drone");
    println!(
        "  set-mode <mode>            — change FC mode (guided|rtl|loiter|manual|auto|stabilize)"
    );
    println!("  scan                       — start scanning for targets");
    println!("  select-target <id>         — select target by ID, transition to TRACKING");
    println!("  abort                      — ABORT (force transition + RTL)");
    println!("  reset                      — return to IDLE (after ABORT + disarm)");
    println!("  watchdogs                  — show watchdog statuses");
    println!("  anti-loop                  — show anti-loop guard stats");
    println!("  feed-watchdog <name>       — feed a watchdog (video|inference|tracking|command|heartbeat)");
    println!("  simulate-heartbeat-loss    — test: simulate FC heartbeat loss");
    println!("  simulate-attitude <r p y>  — test: inject attitude update");
    println!("  quit                       — exit");
}

async fn print_status(ctx: &ReplContext) {
    // Snapshot state machine fields first, then drop the guard before awaiting.
    let (state_name, transition_count) = {
        let sm = ctx.state.lock();
        (sm.state(), sm.transition_count())
    };
    let fc = ctx.fc.lock().await;
    let hb = fc.heartbeat_status();
    let att = fc.attitude();
    let recorded = fc.recorded_commands();

    println!(
        "\n=== System Status @ {} ===",
        Utc::now().format("%H:%M:%S")
    );
    println!(
        "State machine:    {} (transitions: {})",
        state_name, transition_count
    );
    println!("FC armed:         {}", hb.armed);
    println!("FC mode:          {:?}", hb.mode);
    println!(
        "FC heartbeat:     {}",
        if hb.is_stale(1000) { "STALE" } else { "OK" }
    );
    println!(
        "FC attitude:      roll={:.2} pitch={:.2} yaw={:.2}",
        att.roll, att.pitch, att.yaw
    );
    println!("FC commands sent: {}", recorded.len());
    println!();
}

fn print_watchdogs(ctx: &ReplContext) {
    let snap = ctx.watchdogs.snapshot();
    println!("\n=== Watchdogs ===");
    println!(
        "{:<15} {:>10} {:>10} {:>10} {:>10}",
        "NAME", "ELAPSED", "LIMIT", "EXPIRED", "FEEDS"
    );
    for w in &snap {
        println!(
            "{:<15} {:>8}ms {:>8}ms {:>10} {:>10}",
            w.id.as_str(),
            w.elapsed_ms,
            w.timeout_ms,
            w.expired,
            w.feed_count
        );
    }
    let expired_count = snap.iter().filter(|w| w.expired).count();
    println!("\nExpired: {expired_count}/{}", snap.len());
}

fn print_anti_loop(ctx: &ReplContext) {
    let snap = ctx.anti_loop.snapshot();
    println!("\n=== Anti-Loop Guard ===");
    println!("Frozen:                 {}", snap.frozen);
    println!("Commands allowed:       {}", snap.allowed_count);
    println!("Commands suppressed:    {}", snap.suppressed_count);
    println!("Oscillation triggers:   {}", snap.oscillation_trigger_count);
    println!("History length:         {}", snap.history_len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::CommanderConfig;

    fn make_ctx() -> ReplContext {
        let state = Arc::new(PlMutex::new(commander::StateMachine::new(
            SystemState::Idle,
        )));
        let fc = MockFcAdapter::new();
        let fc = Arc::new(TokioMutex::new(fc));
        let watchdogs = Arc::new(commander::WatchdogRegistry::new());
        let anti_loop = Arc::new(commander::AntiLoopGuard::new(CommanderConfig::default()));
        ReplContext::new(state, fc, watchdogs, anti_loop)
    }

    #[tokio::test]
    async fn help_command_works() {
        let ctx = make_ctx();
        // Connect FC first
        ctx.fc.lock().await.connect().await.unwrap();
        let result = handle_command("help", &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn status_command_works() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        let result = handle_command("status", &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn unknown_command_returns_error() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        let result = handle_command("nonexistent", &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown command"));
    }

    #[tokio::test]
    async fn arm_command_transitions_state() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        handle_command("arm", &ctx).await.unwrap();
        assert_eq!(ctx.state.lock().state(), SystemState::Armed);
        assert!(ctx.fc.lock().await.heartbeat_status().armed);
    }

    #[tokio::test]
    async fn scan_after_arm_works() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        handle_command("arm", &ctx).await.unwrap();
        handle_command("scan", &ctx).await.unwrap();
        assert_eq!(ctx.state.lock().state(), SystemState::Scanning);
    }

    #[tokio::test]
    async fn select_target_transitions_to_tracking() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        handle_command("arm", &ctx).await.unwrap();
        handle_command("scan", &ctx).await.unwrap();
        handle_command("select-target 42", &ctx).await.unwrap();
        assert_eq!(ctx.state.lock().state(), SystemState::Tracking);
    }

    #[tokio::test]
    async fn abort_forces_state() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        handle_command("arm", &ctx).await.unwrap();
        handle_command("scan", &ctx).await.unwrap();
        handle_command("abort", &ctx).await.unwrap();
        assert_eq!(ctx.state.lock().state(), SystemState::Abort);
    }

    #[tokio::test]
    async fn reset_only_works_from_abort() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        handle_command("arm", &ctx).await.unwrap();

        // reset from ARMED should fail
        let result = handle_command("reset", &ctx).await;
        assert!(result.is_err());

        // abort, then reset should work
        handle_command("abort", &ctx).await.unwrap();
        handle_command("reset", &ctx).await.unwrap();
        assert_eq!(ctx.state.lock().state(), SystemState::Idle);
    }

    #[tokio::test]
    async fn set_mode_parses_correctly() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        handle_command("set-mode guided", &ctx).await.unwrap();
        assert_eq!(
            ctx.fc.lock().await.heartbeat_status().mode,
            FlightMode::Guided
        );

        handle_command("set-mode rtl", &ctx).await.unwrap();
        assert_eq!(ctx.fc.lock().await.heartbeat_status().mode, FlightMode::Rtl);
    }

    #[tokio::test]
    async fn set_mode_invalid_returns_error() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        let result = handle_command("set-mode nonsense", &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn feed_watchdog_works() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        ctx.watchdogs.register(
            commander::WatchdogId::VideoLoop,
            commander::watchdogs::WatchdogConfig::new(
                std::time::Duration::from_millis(100),
                commander::watchdogs::WatchdogAction::Degrade,
            ),
        );
        handle_command("feed-watchdog video", &ctx).await.unwrap();
        let snap = ctx.watchdogs.snapshot();
        let v = snap
            .iter()
            .find(|w| w.id == commander::WatchdogId::VideoLoop)
            .unwrap();
        assert_eq!(v.feed_count, 1);
    }

    #[tokio::test]
    async fn simulate_heartbeat_loss_works() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        assert!(!ctx.fc.lock().await.is_heartbeat_stale(1000));
        handle_command("simulate-heartbeat-loss", &ctx)
            .await
            .unwrap();
        assert!(ctx.fc.lock().await.is_heartbeat_stale(1000));
    }

    #[tokio::test]
    async fn simulate_attitude_updates_state() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        handle_command("simulate-attitude 0.1 0.2 1.5", &ctx)
            .await
            .unwrap();
        let att = ctx.fc.lock().await.attitude();
        assert!((att.roll - 0.1).abs() < 1e-3);
        assert!((att.pitch - 0.2).abs() < 1e-3);
        assert!((att.yaw - 1.5).abs() < 1e-3);
    }

    #[test]
    fn parse_flight_mode_recognizes_all_modes() {
        assert_eq!(parse_flight_mode("guided").unwrap(), FlightMode::Guided);
        assert_eq!(parse_flight_mode("GUIDED").unwrap(), FlightMode::Guided);
        assert_eq!(parse_flight_mode("rtl").unwrap(), FlightMode::Rtl);
        assert_eq!(parse_flight_mode("loiter").unwrap(), FlightMode::Loiter);
        assert_eq!(parse_flight_mode("manual").unwrap(), FlightMode::Manual);
        assert_eq!(parse_flight_mode("auto").unwrap(), FlightMode::Auto);
        assert_eq!(
            parse_flight_mode("stabilize").unwrap(),
            FlightMode::Stabilize
        );
        assert_eq!(parse_flight_mode("stab").unwrap(), FlightMode::Stabilize);
        assert!(parse_flight_mode("nonsense").is_err());
    }

    #[tokio::test]
    async fn watchdogs_command_does_not_panic() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        ctx.watchdogs.register(
            commander::WatchdogId::VideoLoop,
            commander::watchdogs::WatchdogConfig::new(
                std::time::Duration::from_millis(100),
                commander::watchdogs::WatchdogAction::Degrade,
            ),
        );
        let result = handle_command("watchdogs", &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn anti_loop_command_does_not_panic() {
        let ctx = make_ctx();
        ctx.fc.lock().await.connect().await.unwrap();
        let result = handle_command("anti-loop", &ctx).await;
        assert!(result.is_ok());
    }
}
