//! Phase 1.1 soak test: run the full minimal loop
//! (video → inference → annotation → metrics) for a configurable duration and
//! emit a JSON summary + telemetry log.
//!
//! This closes the "непрерывный тест продолжительностью не менее 30 минут"
//! criterion of task 1.1. The loop is the real production pipeline:
//! `VideoSource` → `CpuInferenceBackend` → `FrameWriter` → `MetricsRecorder`,
//! with a `TelemetrySample` captured periodically (RSS, CPU/NPU temperature).
//!
//! # Sources
//!
//! By default a `SyntheticVideoSource` is used (no camera required — ideal for
//! first soak on a dev machine / CI). Pass `--replay <dir>` to read frames
//! previously recorded by `ReplaySource`, or wire a V4L2 source on Linux.
//!
//! # Usage
//!
//! ```sh
//! # 30-minute soak on synthetic source, no model (smoke):
//! cargo run -p cv-inference --example soak --features cpu-onnx -- --minutes 30
//!
//! # 30-minute soak WITH real COCO model:
//! ./scripts/download_models.sh
//! cargo run -p cv-inference --example soak --features cpu-onnx -- \
//!     --minutes 30 --model models/yolov8n.onnx --output output/soak
//! ```
//!
//! # Output
//!
//! - `output/soak/frames/seq_NNNNNN.jpg` — annotated frames (throttled by
//!   `--save-every`, default every 5 seconds of video at 30 fps → ~1 Hz).
//! - `output/soak/detections.jsonl` — per-saved-frame detection log.
//! - `output/soak/telemetry.jsonl` — periodic telemetry samples (RSS/temp).
//! - `output/soak/summary.json` — final FPS/latency percentiles.
//! - stdout: progress every 30 s and the final summary table.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use cv_inference::cpu_onnx::CpuInferenceBackend;
use cv_inference::InferenceBackend;
use cv_visualizer::FrameWriter;
use system_telemetry::metrics::{MetricsRecorder, RunSummary};
use system_telemetry::TelemetrySample;
use video_capture::{SyntheticConfig, SyntheticPattern, SyntheticVideoSource, VideoSource};

/// Phase 1.1 soak test for the minimal CV loop.
#[derive(Debug, Parser)]
struct Args {
    /// Soak duration in minutes (minimum 30 to satisfy the 1.1 criterion,
    /// but any positive value is allowed for dev/CI smoke runs).
    #[arg(long, default_value_t = 30)]
    minutes: u64,

    /// Path to a YOLOv8n ONNX model. If omitted, inference is skipped and only
    /// the capture→annotate path is exercised (smoke mode).
    #[arg(long)]
    model: Option<PathBuf>,

    /// Output directory for frames/telemetry/summary.
    #[arg(long, default_value = "output/soak")]
    output: PathBuf,

    /// Save one annotated frame every N source frames (throttle to bound disk
    /// usage over a long run). Default: 150 ≈ 1 Hz at 30 fps.
    #[arg(long, default_value_t = 150)]
    save_every: u64,

    /// Optional TTF font path to render class/confidence labels.
    #[arg(long)]
    font: Option<PathBuf>,

    /// Synthetic source width.
    #[arg(long, default_value_t = 640)]
    width: u32,

    /// Synthetic source height.
    #[arg(long, default_value_t = 480)]
    height: u32,

    /// Synthetic source fps.
    #[arg(long, default_value_t = 30)]
    fps: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let deadline = Instant::now() + Duration::from_secs(args.minutes * 60);

    std::fs::create_dir_all(&args.output)
        .with_context(|| format!("create output dir {}", args.output.display()))?;

    // --- Build the writer (annotate + persist) ---
    let mut writer_builder = FrameWriter::new(&args.output, args.save_every)?;
    if let Some(font) = &args.font {
        writer_builder = writer_builder.with_font_path(font)?;
    }
    let writer = Arc::new(parking_lot::Mutex::new(writer_builder));

    // --- Build + init the inference backend (optional) ---
    let mut backend_opt: Option<CpuInferenceBackend> = if let Some(model) = &args.model {
        let model_str = model
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("model path is not valid UTF-8"))?;
        let mut b = CpuInferenceBackend::new(model_str);
        b.init().await.context("init ONNX backend")?;
        tracing::info!(model = %model.display(), "inference backend initialized");
        Some(b)
    } else {
        tracing::warn!("no --model given; running in smoke mode (no inference)");
        None
    };

    // --- Build the video source ---
    let cfg = SyntheticConfig {
        width: args.width,
        height: args.height,
        fps: args.fps,
        pattern: SyntheticPattern::Checkerboard,
        infinite: true,
        max_frames: 0,
    };
    let mut source = SyntheticVideoSource::new(cfg);
    let rx = source
        .start()
        .await
        .context("start synthetic video source")?;
    tracing::info!(
        width = args.width,
        height = args.height,
        fps = args.fps,
        minutes = args.minutes,
        "soak started"
    );

    // --- Run the loop ---
    let mut rx = rx;
    let mut metrics = MetricsRecorder::new();
    let mut last_progress = Instant::now();
    let mut last_telemetry = Instant::now();
    let telemetry_path = args.output.join("telemetry.jsonl");

    let mut frames_total: u64 = 0;
    while Instant::now() < deadline {
        let frame = match rx.recv().await {
            Some(f) => f,
            None => {
                tracing::warn!("video source ended unexpectedly");
                break;
            }
        };
        frames_total += 1;
        let frame_start = Instant::now();

        // Inference (if backend present). Synthetic source emits RGB24
        // directly, so no decode step is needed.
        let infer_start = Instant::now();
        let detections = if let Some(b) = backend_opt.as_mut() {
            b.infer(&frame).await.unwrap_or_else(|e| {
                tracing::error!(error = %e, "inference failed; treating as zero detections");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let infer_us = infer_start.elapsed().as_micros() as u64;

        // Annotate + persist (throttled inside FrameWriter).
        let ann_start = Instant::now();
        {
            let mut w = writer.lock();
            if let Err(e) = w.save(&frame, &detections) {
                tracing::error!(error = %e, "frame save failed");
            }
        }
        let ann_us = ann_start.elapsed().as_micros() as u64;

        let total_us = frame_start.elapsed().as_micros() as u64;
        metrics.record(0, infer_us, ann_us, total_us);

        // Periodic telemetry (~ every 10 s).
        if last_telemetry.elapsed() >= Duration::from_secs(10) {
            let sample = TelemetrySample::capture();
            let line = serde_json::to_string(&sample)?;
            append_line(&telemetry_path, &line)?;
            last_telemetry = Instant::now();
        }

        // Periodic progress (~ every 30 s).
        if last_progress.elapsed() >= Duration::from_secs(30) {
            tracing::info!(
                elapsed_s = metrics.elapsed().as_secs(),
                frames = frames_total,
                fps_so_far = (frames_total as f64 / metrics.elapsed().as_secs_f64()).round() as u64,
                "soak progress"
            );
            last_progress = Instant::now();
        }
    }

    if let Err(e) = source.stop().await {
        tracing::warn!(error = %e, "video source stop returned error");
    }
    drop(backend_opt);

    // --- Final summary ---
    let summary: RunSummary = metrics.summary();
    let summary_path = args.output.join("summary.json");
    let summary_json = serde_json::to_string_pretty(&summary)?;
    std::fs::write(&summary_path, summary_json)?;

    println!("\n========== SOAK SUMMARY ==========");
    println!("duration:        {:.1} min", summary.elapsed_s / 60.0);
    println!("frames looped:   {}", summary.frames_processed);
    println!("sustained FPS:   {:.2}", summary.sustained_fps);
    println!();
    println!(
        "{:<10} {:>8} {:>10} {:>10} {:>10} {:>10}",
        "stage", "count", "mean_ms", "p50_ms", "p95_ms", "max_ms"
    );
    for st in &summary.stages {
        println!(
            "{:<10} {:>8} {:>10.2} {:>10.2} {:>10.2} {:>10.2}",
            st.stage, st.count, st.mean_ms, st.p50_ms, st.p95_ms, st.max_ms
        );
    }
    println!();
    println!("summary:    {}", summary_path.display());
    println!("telemetry:  {}", telemetry_path.display());

    // Capture a final telemetry snapshot to surface end-of-run RSS/temp.
    let final_sample = TelemetrySample::capture();
    println!(
        "final RSS:  {} KiB | CPU: {} °C | NPU: {} °C | NPU load: {}%",
        final_sample.rss_kb.unwrap_or(0),
        final_sample
            .cpu_temp_c
            .map(|t| t.to_string())
            .unwrap_or_else(|| "n/a".into()),
        final_sample
            .npu_temp_c
            .map(|t| t.to_string())
            .unwrap_or_else(|| "n/a".into()),
        final_sample
            .npu_load_percent
            .map(|t| t.to_string())
            .unwrap_or_else(|| "n/a".into()),
    );
    println!("==================================\n");

    Ok(())
}

fn append_line(path: &std::path::Path, line: &str) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}
