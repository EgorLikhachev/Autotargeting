//! commander_bus_demo — CLI для M4: commander на шине (замкнутый контур).
//!
//! Против SITL (поднят fc-bridge на tcpout:127.0.0.1:5760):
//!   commander_bus_demo --fc ardupilot-mavlink --endpoint tcpout:127.0.0.1:5760 \
//!       --bus tcp/127.0.0.1:7447 --seconds 30
//! Без железа: --fc mock.

use clap::Parser;

#[derive(Parser)]
struct Args {
    /// Адаптер FC командира: mock | sitl-mavlink | ardupilot-mavlink.
    #[arg(long, default_value = "mock")]
    fc: String,
    /// Endpoint адаптера FC командира.
    #[arg(long, default_value = "127.0.0.1:14550")]
    endpoint: String,
    /// Endpoint шины.
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    bus: String,
    /// Прекратить после N секунд (0 — бессрочно).
    #[arg(long, default_value_t = 0)]
    seconds: u64,
    /// Центр кадра X (пиксели).
    #[arg(long, default_value_t = 320.0)]
    center_x: f32,
    /// Центр кадра Y.
    #[arg(long, default_value_t = 240.0)]
    center_y: f32,
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
        adapter: args.fc.clone(),
        endpoint: args.endpoint.clone(),
        ..Default::default()
    };
    let adapter = fc_adapter::build_adapter(&fc_cfg);
    let mut commander = commander::Commander::new(common::CommanderConfig::default(), adapter);

    let bus = match event_bus::EventBus::connect(&args.bus).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[!] bus connect failed: {e}");
            std::process::exit(4);
        }
    };
    let cfg = commander::bus_runner::CommanderBusConfig {
        frame_center: (args.center_x, args.center_y),
        max_duration: (args.seconds > 0).then(|| std::time::Duration::from_secs(args.seconds)),
        ..Default::default()
    };

    println!(
        "=== commander_bus_demo === fc={} endpoint={} bus={} center=({},{})",
        args.fc, args.endpoint, args.bus, args.center_x, args.center_y
    );
    match commander::bus_runner::CommanderBus::new(cfg)
        .run(&mut commander, &bus)
        .await
    {
        Ok(stats) => {
            println!(
                "[summary] TRACKS={} TELEMETRY={} CORRECTIONS_SENT={} SUPPRESSED={}",
                stats.tracks_received,
                stats.telemetry_received,
                stats.corrections_sent,
                stats.corrections_suppressed
            );
        }
        Err(e) => {
            eprintln!("[!] commander bus failed: {e}");
            std::process::exit(6);
        }
    }
    let _ = bus.close().await;
}
