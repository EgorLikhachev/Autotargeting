//! tracker — CLI компонента сопровождения (M2).
//!
//! На стенде (подняны детектор и шина):
//!   tracker --seconds 30
//! Наблюдать: bus_dump --listen (видит at/tracks + at/status/tracker).

use clap::Parser;
use tracker_crate::{Tracker, TrackerConfig};

#[derive(Parser)]
struct Args {
    /// Endpoint шины.
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    bus: String,
    /// Возраст трека до LOST, мс.
    #[arg(long, default_value_t = 2000)]
    max_age_ms: u64,
    /// Пропущенных кадров до LOST.
    #[arg(long, default_value_t = 60)]
    max_missed: u32,
    /// IoU-порог сопоставления.
    #[arg(long, default_value_t = 0.3)]
    iou: f32,
    /// Только подтверждённые (locked) треки.
    #[arg(long, default_value_t = false)]
    locked_only: bool,
    /// Прекратить после N секунд (0 — пока есть детекции).
    #[arg(long, default_value_t = 0)]
    seconds: u64,
    /// Тишина детекций до завершения, сек.
    #[arg(long, default_value_t = 0)]
    quiet_timeout: u64,
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
    let cfg = TrackerConfig {
        bus: args.bus.clone(),
        max_target_age_ms: args.max_age_ms,
        max_missed_frames: args.max_missed,
        match_iou_threshold: args.iou,
        locked_only: args.locked_only,
        max_duration: (args.seconds > 0).then(|| std::time::Duration::from_secs(args.seconds)),
        quiet_timeout: (args.quiet_timeout > 0)
            .then(|| std::time::Duration::from_secs(args.quiet_timeout)),
        ..TrackerConfig::default()
    };

    println!(
        "=== tracker === bus={} iou={} max_age={}ms locked_only={}",
        cfg.bus, cfg.match_iou_threshold, cfg.max_target_age_ms, cfg.locked_only
    );

    let bus = match event_bus::EventBus::connect(&cfg.bus).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[!] bus connect failed: {e}");
            std::process::exit(3);
        }
    };
    let tracker = match Tracker::new(cfg) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[!] config: {e}");
            std::process::exit(4);
        }
    };
    if let Err(e) = tracker.run(&bus).await {
        eprintln!("[!] tracker failed: {e}");
        std::process::exit(5);
    }
    let _ = bus.close().await;
}
