//! fc-bridge — CLI моста FC ↔ шина (M3).
//!
//! x86 с SITL (docker: ./sim/sitl/run_sitl.sh start):
//!   fc-bridge --adapter ardupilot-mavlink --endpoint tcpout:127.0.0.1:5760 \
//!       --bus tcp/127.0.0.1:7447
//! Без железа: --adapter mock.
//! Стенд (реальный FC): --adapter ardupilot-mavlink --endpoint serial:/dev/ttyACM0:115200.

use clap::Parser;
use fc_bridge::{BridgeConfig, FcBridge};

#[derive(Parser)]
struct Args {
    /// Адаптер FC: mock | sitl-mavlink | ardupilot-mavlink.
    #[arg(long, default_value = "mock")]
    adapter: String,
    /// Endpoint адаптера (udpin/tcpout/serial URL).
    #[arg(long, default_value = "127.0.0.1:14550")]
    endpoint: String,
    /// Endpoint шины zenoh.
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    bus: String,
    /// Частота телеметрии, Гц.
    #[arg(long, default_value_t = 10)]
    hz: u32,
    /// Прекратить после N секунд (0 — бессрочно).
    #[arg(long, default_value_t = 0)]
    seconds: u64,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let fc_cfg = common::FcConfig {
        adapter: args.adapter.clone(),
        endpoint: args.endpoint.clone(),
        ..Default::default()
    };
    let cfg = BridgeConfig {
        telemetry_hz: args.hz,
        max_duration: (args.seconds > 0).then(|| std::time::Duration::from_secs(args.seconds)),
        ..BridgeConfig::default()
    };

    println!(
        "=== fc-bridge === adapter={} endpoint={} bus={} hz={}",
        args.adapter, args.endpoint, args.bus, args.hz
    );

    let mut adapter = fc_adapter::build_adapter(&fc_cfg);
    if let Err(e) = adapter.connect().await {
        eprintln!("[!] FC connect failed: {e}");
        std::process::exit(3);
    }
    let bus = match event_bus::EventBus::connect(&args.bus).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[!] bus connect failed: {e}");
            std::process::exit(4);
        }
    };

    let bridge = FcBridge::new(cfg);
    match bridge.run(adapter.as_mut(), &bus).await {
        Ok(stats) => {
            println!(
                "[summary] TELEMETRY={} COMMANDS={} CMD_ERRORS={}",
                stats.telemetry_published, stats.commands_handled, stats.command_errors
            );
        }
        Err(e) => {
            eprintln!("[!] bridge failed: {e}");
            std::process::exit(6);
        }
    }
    let _ = bus.close().await;
    let _ = adapter.disconnect().await;
}
