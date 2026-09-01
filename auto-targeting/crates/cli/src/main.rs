//! Auto-Targeting System — main CLI binary.
//!
//! Usage:
//!   auto-targeting --config /etc/auto-targeting/config.toml
//!   auto-targeting --mock-fc --mock-video demo
//!
//! Status: 🚧 Phase 0 — basic scaffolding + mock mode smoke test.
//! Real video/inference integration lands in Phase 5.

use anyhow::Result;
use auto_targeting_cli::args::{CliArgs, Command, RunMode};
use auto_targeting_cli::bus_console;
use auto_targeting_cli::operator::OperatorCommand;
use auto_targeting_cli::repl;
use auto_targeting_cli::scenario_runner;
use clap::Parser;
use common::AppConfig;
use fc_adapter::FlightControllerAdapter;
use parking_lot::Mutex as PlMutex;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    let args = CliArgs::parse();

    // Load config (or defaults if not provided / not parseable)
    let config_path = args.config.as_ref().and_then(|p| p.to_str());
    let config = AppConfig::load_or_default(config_path);

    // Initialize tracing
    init_tracing(&config.log_filter, config.log_file.as_deref())?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        mode = ?args.mode(),
        "auto-targeting system starting"
    );

    // Run according to mode
    match args.mode() {
        RunMode::BusMon => {
            let Command::BusMon { topics, max_len } = args.command.expect("bus-mon subcommand")
            else {
                unreachable!()
            };
            // Монитор — подключается (listener держит якорь config-svc).
            let bus = bus_console::connect_bus(&config, false).await?;
            bus_console::run_monitor(&bus, &topics, max_len).await
        }
        RunMode::ReplBus => {
            let bus = bus_console::connect_bus(&config, false).await?;
            bus_console::run_repl(&bus).await
        }
        RunMode::ConfigSvc => {
            let bus = bus_console::connect_bus(&config, true).await?;
            bus_console::run_config_service(&bus, config).await
        }
        RunMode::ConfigGet => {
            let bus = bus_console::connect_bus(&config, false).await?;
            bus_console::config_get(&bus).await
        }
        RunMode::Full => run_full(config).await,
        RunMode::MockFc => run_mock_fc(config).await,
        RunMode::MockAll => run_mock_all(config).await,
        RunMode::Repl => run_repl(config).await,
        RunMode::Scenario => run_scenario_command(args).await,
        RunMode::HealthCheck => {
            // Stub: in Phase 5 we'll expose an HTTP health endpoint.
            println!(
                "{{\"status\":\"ok\",\"version\":\"{}\"}}",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}

fn init_tracing(filter_spec: &str, log_file: Option<&str>) -> Result<()> {
    let filter = EnvFilter::try_new(filter_spec).unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = fmt::layer().compact().with_target(true);

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer);

    if let Some(path) = log_file {
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(file) => {
                let file_layer = fmt::layer().json().with_writer(std::sync::Mutex::new(file));
                subscriber.with(file_layer).init();
            }
            Err(e) => {
                warn!(path = path, error = %e, "failed to open log file, logging to stdout only");
                subscriber.init();
            }
        }
    } else {
        subscriber.init();
    }
    Ok(())
}

/// Full production mode — real video, real inference, real FC.
/// Not yet implemented (Phase 5).
async fn run_full(_config: AppConfig) -> Result<()> {
    warn!("Full mode not yet implemented (Phase 5). Use --mock-fc or --mock-all for testing.");
    Ok(())
}

/// Mock FC mode — real video + real inference, but mock FC.
/// Not yet implemented (Phase 5).
async fn run_mock_fc(_config: AppConfig) -> Result<()> {
    warn!("Mock-fc mode not yet implemented (Phase 5). Use --mock-all for now.");
    Ok(())
}

/// All-mock mode — synthetic video, mock inference, mock FC.
/// This is the Phase 0 smoke test.
async fn run_mock_all(config: AppConfig) -> Result<()> {
    info!("Running in mock-all mode (Phase 0 smoke test)");

    // Create mock FC
    let mut fc = fc_adapter::MockFcAdapter::new();
    fc.connect().await?;
    fc.arm().await?;
    fc.set_mode(common::FlightMode::Guided).await?;

    // Create state machine
    let mut sm = commander::StateMachine::new(common::SystemState::Idle);
    sm.try_transition(common::SystemState::Armed)?;
    sm.try_transition(common::SystemState::Scanning)?;
    info!(state = sm.state().as_str(), "state machine initialized");

    // Create anti-loop guard
    let anti_loop = Arc::new(commander::AntiLoopGuard::new(config.commander.clone()));

    // Create watchdog registry
    let watchdogs = Arc::new(commander::WatchdogRegistry::new());
    use std::time::Duration;
    watchdogs.register(
        commander::WatchdogId::VideoLoop,
        commander::watchdogs::WatchdogConfig::new(
            Duration::from_millis(config.commander.video_loop_wdt_ms),
            commander::watchdogs::WatchdogAction::Degrade,
        ),
    );
    watchdogs.register(
        commander::WatchdogId::FcHeartbeat,
        commander::watchdogs::WatchdogConfig::new(
            Duration::from_millis(config.fc.heartbeat_timeout_ms),
            commander::watchdogs::WatchdogAction::Abort,
        ),
    );

    // Simulate a few cycles
    for i in 0..5 {
        watchdogs.feed(commander::WatchdogId::VideoLoop);
        fc.simulate_attitude(common::Attitude {
            yaw: i as f32 * 0.1,
            ..Default::default()
        });

        let cmd = commander::anti_loop::CorrectionCommand {
            yaw_rate_dps: 5.0,
            pitch_rate_dps: 0.0,
            offset_x: 0.2,
            offset_y: 0.1,
            generated_at: std::time::Instant::now(),
        };
        let decision = anti_loop.process(cmd);
        info!(cycle = i, ?decision, "processed command");

        // In a real system, we'd send a MAVLink command here if Allow.
        if let commander::anti_loop::GuardDecision::Allow(_) = decision {
            fc.set_position_target_local_ned(common::PositionTargetNED {
                north: 1.0,
                east: 0.0,
                down: 0.0,
                yaw: 0.1 * i as f32,
            })
            .await?;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Check watchdogs
    let expired = watchdogs.check_expired();
    info!(expired_count = expired.len(), "watchdog check complete");

    // Print summary — note: we snapshot all state BEFORE calling any async
    // method, because parking_lot::MutexGuard is not Send and cannot be held
    // across .await points (clippy::await_holding_lock).
    let fc_state = fc.state_handle();
    let (cmds_len, armed, mode) = {
        let s = fc_state.lock();
        (s.commands.len(), s.armed, s.mode)
    };
    let sm_state = sm.state();
    let sm_transitions = sm.transition_count();

    let al_snap = anti_loop.snapshot();
    let wd_snap = watchdogs.snapshot();

    println!("\n=== Mock-all smoke test summary ===");
    println!("State machine:    {}", sm_state.as_str());
    println!("Transitions:      {}", sm_transitions);
    println!("FC commands sent: {}", cmds_len);
    println!("FC armed:         {}", armed);
    println!("FC mode:          {:?}", mode);

    println!("\nAnti-loop guard:");
    println!("  Allowed:         {}", al_snap.allowed_count);
    println!("  Suppressed:      {}", al_snap.suppressed_count);
    println!("  Oscillations:    {}", al_snap.oscillation_trigger_count);

    println!("\nWatchdogs:");
    for w in &wd_snap {
        println!(
            "  {:<15} elapsed={:>5}ms / limit={:>5}ms  expired={}",
            w.id.as_str(),
            w.elapsed_ms,
            w.timeout_ms,
            w.expired
        );
    }
    println!("\nAll good. ✅");

    fc.disarm().await?;
    fc.disconnect().await?;
    Ok(())
}

/// Handle an operator command — to be wired into gRPC/HTTP in Phase 5.
#[allow(dead_code)]
async fn handle_operator_command(
    _cmd: OperatorCommand,
    _sm: &mut commander::StateMachine,
    _fc: &mut fc_adapter::MockFcAdapter,
) -> Result<()> {
    // Stub for Phase 5
    Ok(())
}

/// Run the interactive REPL (operator console).
/// Uses MockFcAdapter so it works without real hardware.
async fn run_repl(config: AppConfig) -> Result<()> {
    use std::time::Duration;

    info!("Starting REPL (operator console)");

    // Create mock FC
    let mut fc = fc_adapter::MockFcAdapter::new();
    fc.connect().await?;
    fc.arm().await?;
    fc.set_mode(common::FlightMode::Guided).await?;

    // Wrap in shared state
    let state = Arc::new(PlMutex::new(commander::StateMachine::new(
        common::SystemState::Idle,
    )));
    // Transition to ARMED to match the FC state
    {
        let mut sm = state.lock();
        sm.try_transition(common::SystemState::Armed)?;
        sm.try_transition(common::SystemState::Scanning)?;
    }
    let fc = Arc::new(TokioMutex::new(fc));

    // Create watchdogs
    let watchdogs = Arc::new(commander::WatchdogRegistry::new());
    watchdogs.register(
        commander::WatchdogId::VideoLoop,
        commander::watchdogs::WatchdogConfig::new(
            Duration::from_millis(config.commander.video_loop_wdt_ms),
            commander::watchdogs::WatchdogAction::Degrade,
        ),
    );
    watchdogs.register(
        commander::WatchdogId::InferenceLoop,
        commander::watchdogs::WatchdogConfig::new(
            Duration::from_millis(config.commander.inference_loop_wdt_ms),
            commander::watchdogs::WatchdogAction::Degrade,
        ),
    );
    watchdogs.register(
        commander::WatchdogId::TrackingLoop,
        commander::watchdogs::WatchdogConfig::new(
            Duration::from_millis(config.commander.tracking_loop_wdt_ms),
            commander::watchdogs::WatchdogAction::Degrade,
        ),
    );
    watchdogs.register(
        commander::WatchdogId::CommandLoop,
        commander::watchdogs::WatchdogConfig::new(
            Duration::from_millis(config.commander.command_loop_wdt_ms),
            commander::watchdogs::WatchdogAction::Abort,
        ),
    );
    watchdogs.register(
        commander::WatchdogId::FcHeartbeat,
        commander::watchdogs::WatchdogConfig::new(
            Duration::from_millis(config.fc.heartbeat_timeout_ms),
            commander::watchdogs::WatchdogAction::Abort,
        ),
    );

    // Create anti-loop guard
    let anti_loop = Arc::new(commander::AntiLoopGuard::new(config.commander.clone()));

    // Spawn a background task that simulates the loops feeding watchdogs
    let wd_bg = Arc::clone(&watchdogs);
    tokio::spawn(async move {
        let interval = Duration::from_millis(50);
        loop {
            wd_bg.feed(commander::WatchdogId::VideoLoop);
            wd_bg.feed(commander::WatchdogId::InferenceLoop);
            wd_bg.feed(commander::WatchdogId::TrackingLoop);
            wd_bg.feed(commander::WatchdogId::CommandLoop);
            tokio::time::sleep(interval).await;
        }
    });

    let ctx = repl::ReplContext::new(state, fc, watchdogs, anti_loop);
    repl::run_repl(ctx).await
}

/// Run the scenario subcommand.
async fn run_scenario_command(args: CliArgs) -> Result<()> {
    let command = args
        .command
        .expect("command should be set in scenario mode");
    let scenario_args = match command {
        Command::Scenario { scenario_args } => scenario_args,
        _ => unreachable!("bus modes dispatched earlier"),
    };

    match (scenario_args.file, scenario_args.all) {
        (Some(path), None) => {
            // Single scenario
            let result = scenario_runner::run_scenario(&path, args.verbose > 0).await?;
            println!("{result}");
            if !result.passed {
                std::process::exit(1);
            }
            Ok(())
        }
        (None, Some(dir)) => {
            // All scenarios in directory
            let results = scenario_runner::run_all_scenarios(&dir, args.verbose > 0).await;
            scenario_runner::print_summary(&results);
            let any_failed = results.iter().any(|r| !r.passed);
            if any_failed {
                std::process::exit(1);
            }
            Ok(())
        }
        (None, None) => {
            eprintln!("Error: must specify either a scenario file or --all <dir>");
            eprintln!("Usage:");
            eprintln!("  auto-targeting scenario <file.json>");
            eprintln!("  auto-targeting scenario --all <dir>");
            std::process::exit(2);
        }
        (Some(_), Some(_)) => {
            eprintln!("Error: cannot specify both a file and --all");
            std::process::exit(2);
        }
    }
}
