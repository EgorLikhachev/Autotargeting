//! commander-bus — сервис-бинарь commander на шине (M4/этап 8).
//!
//! Тот же контур, что example commander_bus_demo, но как bin для systemd:
//!   commander-bus --fc mock --bus tcp/127.0.0.1:7447

use clap::Parser;

#[derive(Parser)]
struct Args {
    /// Адаптер FC: mock | sitl-mavlink | ardupilot-mavlink.
    #[arg(long, default_value = "mock")]
    fc: String,
    #[arg(long, default_value = "127.0.0.1:14550")]
    endpoint: String,
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    bus: String,
    /// Ширина кадра (пиксели) — для нормализации offset.
    #[arg(long, default_value_t = 640)]
    width: u32,
    /// Высота кадра.
    #[arg(long, default_value_t = 480)]
    height: u32,
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
        frame_size: (args.width, args.height),
        ..Default::default()
    };
    println!(
        "=== commander-bus === fc={} bus={} frame=({},{})",
        args.fc, args.bus, args.width, args.height
    );
    if let Err(e) = commander::bus_runner::CommanderBus::new(cfg)
        .run(&mut commander, &bus)
        .await
    {
        eprintln!("[!] commander bus failed: {e}");
        std::process::exit(6);
    }
    let _ = bus.close().await;
}
