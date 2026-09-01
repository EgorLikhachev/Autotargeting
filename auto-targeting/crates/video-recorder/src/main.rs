//! video-recorder — CLI-обёртка рекордера (TG26-125).
//!
//! Запуск НА Orange Pi 5 (нужны запущенный продюсер и ffmpeg):
//!   video-recorder --name autotarget.frames --out output/rec.mp4 \
//!       --fps 30 --seconds 20 --osd \
//!       --font /usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf

use clap::Parser;
use video_recorder::{attach, ReadMode, Recorder, RecorderConfig};

#[derive(Parser)]
struct Args {
    /// Имя сегмента SHM.
    #[arg(long, default_value = "autotarget.frames")]
    name: String,
    /// Выходной MP4.
    #[arg(long, default_value = "output/rec.mp4")]
    out: String,
    /// Номинальный FPS контейнера.
    #[arg(long, default_value_t = 30)]
    fps: u32,
    /// Прекратить после N секунд (0 — пока жив стрим).
    #[arg(long, default_value_t = 0)]
    seconds: u64,
    /// Режим чтения: next (последовательный) | latest.
    #[arg(long, default_value = "next")]
    mode: String,
    /// Прожигать OSD.
    #[arg(long, default_value_t = true)]
    osd: bool,
    /// TTF-шрифт для OSD.
    #[arg(long)]
    font: Option<String>,
    /// Завершение при тишине стрима, сек.
    #[arg(long, default_value_t = 5)]
    quiet_timeout: u64,
    /// Endpoint шины zenoh для at/status/recorder (пусто — статусы выкл, M1).
    #[arg(long)]
    bus: Option<String>,
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
    let mode = match args.mode.as_str() {
        "next" | "sequential" => ReadMode::Sequential,
        "latest" => ReadMode::Latest,
        other => {
            eprintln!("[!] unknown --mode '{other}' (next | latest)");
            std::process::exit(2);
        }
    };
    let cfg = RecorderConfig {
        segment: args.name.clone(),
        output: args.out.clone(),
        fps: args.fps,
        mode,
        osd: args.osd,
        font: args.font.clone(),
        max_duration: (args.seconds > 0).then(|| std::time::Duration::from_secs(args.seconds)),
        quiet_timeout: Some(std::time::Duration::from_secs(args.quiet_timeout)),
        bus: args.bus.clone(),
    };

    println!(
        "=== video-recorder === segment={} out={} fps={} mode={:?} osd={}",
        cfg.segment, cfg.output, cfg.fps, cfg.mode, cfg.osd
    );
    if !video_recorder::FfmpegRawWriter::ffmpeg_available() {
        eprintln!("[!] ffmpeg not found in PATH — cannot encode");
        std::process::exit(3);
    }

    let consumer = match attach(&cfg.segment) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[!] attach failed: {e}");
            std::process::exit(4);
        }
    };
    let recorder = match Recorder::new(cfg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[!] config: {e}");
            std::process::exit(5);
        }
    };

    match recorder.run(&consumer).await {
        Ok(stats) => {
            println!(
                "[summary] RECORDED={} OSD={} JUMPS={} received={}",
                stats.frames_written, stats.osd_frames, stats.jumps, stats.frames_received
            );
        }
        Err(e) => {
            eprintln!("[!] recording failed: {e}");
            std::process::exit(6);
        }
    }
}
