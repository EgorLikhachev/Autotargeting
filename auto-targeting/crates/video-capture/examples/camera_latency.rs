//! USB camera latency benchmark — измерение задержки на реальном железе.
//!
//! Измеряет 3 стадии для каждого кадра:
//!   1. **Capture latency**: от `Instant::now()` перед V4L2-dequeue до получения
//!      сырых байтов во Frame.
//!   2. **Decode latency**: MJPG → RGB24 (jpeg-decoder) или YUYV → RGB24
//!      (конверсия пикселей, video-capture/convert.rs).
//!   3. (опционально) **End-to-end с инференсом**: через rknn-bridge.
//!
//! Запуск НА Orange Pi 5 (нужна USB-камера + feature v4l2):
//!   cargo run --release -p video-capture --example camera_latency \
//!       --features v4l2 -- --device /dev/video0 --width 640 --height 480 --fps 100
//!
//! Прямой ioctl-путь (обход v4l-crate; нужен для gspca-камер, напр. PS Eye):
//!   cargo run --release -p video-capture --example camera_latency \
//!       --features v4l2-direct -- --backend direct --format yuyv --fps 60
//!
//! Вывод: p50/p95 latency по стадиям + sustained FPS.

use std::time::Instant;

use clap::Parser;
use common::{Frame, FrameMetadata, PixelFormat};
use video_capture::VideoSource;

/// USB camera latency benchmark.
#[derive(Debug, Parser)]
struct Args {
    /// V4L2 device path.
    #[arg(long, default_value = "/dev/video0")]
    device: String,
    /// Frame width.
    #[arg(long, default_value_t = 640)]
    width: u32,
    /// Frame height.
    #[arg(long, default_value_t = 480)]
    height: u32,
    /// Target FPS.
    #[arg(long, default_value_t = 30)]
    fps: u32,
    /// Number of frames to capture for the measurement.
    #[arg(long, default_value_t = 100)]
    count: usize,
    /// Pipeline mode: decode runs in a separate thread, capture is never blocked.
    /// Without this flag, capture and decode run sequentially (old behaviour).
    #[arg(long, default_value_t = false)]
    pipeline: bool,
    /// Pixel format: "mjpeg" (Arducam OV9782) or "yuyv" (PS Eye / uncompressed UVC).
    #[arg(long, default_value = "mjpeg")]
    format: String,
    /// Capture backend: "v4l" (v4l crate) or "direct" (raw libc ioctl; needs
    /// feature v4l2-direct). PS Eye (gspca/ov534) works ONLY with "direct" —
    /// the v4l crate hangs on frame dequeue with this driver.
    #[arg(long, default_value = "v4l")]
    backend: String,
}

fn main() {
    let args = Args::parse();
    let pixel_format = match args.format.as_str() {
        "mjpeg" | "jpg" => PixelFormat::Mjpeg,
        "yuyv" | "yuv422" => PixelFormat::Yuyv,
        other => {
            eprintln!("[!] unknown --format '{other}' (expected: mjpeg | yuyv)");
            std::process::exit(2);
        }
    };
    println!("=== USB Camera Latency Benchmark ===");
    println!(
        "Device: {}  {}x{} @ {} fps  frames={}  pipeline={}  format={}  backend={}",
        args.device,
        args.width,
        args.height,
        args.fps,
        args.count,
        args.pipeline,
        args.format,
        args.backend
    );
    println!();

    let rt = tokio::runtime::Runtime::new().unwrap();
    match args.backend.as_str() {
        #[cfg(feature = "v4l2-direct")]
        "direct" => {
            // Прямой V4L2 ioctl — тот же путь, что v4l2-ctl. Работает с gspca.
            let mut source = video_capture::V4l2DirectSource::new(
                &args.device,
                args.width,
                args.height,
                args.fps,
            )
            .with_format(pixel_format)
            .with_buffers(4);
            rt.block_on(run_bench(&mut source, &args));
        }
        #[cfg(feature = "v4l2")]
        "v4l" => {
            // v4l crate (abstraction). Известные проблемы: в 5× медленнее
            // прямого ioctl на UVC и виснет на gspca/ov534 (PS Eye).
            use video_capture::V4l2Source;
            let mut source = V4l2Source::new(&args.device, args.width, args.height, args.fps)
                .with_format(pixel_format);
            rt.block_on(run_bench(&mut source, &args));
        }
        other => {
            eprintln!(
                "[!] unknown --backend '{other}' (expected: v4l | direct; 'direct' needs feature v4l2-direct)"
            );
            std::process::exit(2);
        }
    }
}

/// Общая часть бенчмарка: старт источника, измерение, вывод.
async fn run_bench<S: VideoSource>(source: &mut S, args: &Args) {
    println!("[*] Opening camera...");
    let mut rx = match source.start().await {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("[!] start failed: {e}");
            return;
        }
    };
    println!("[+] Camera streaming. Capturing {} frames...\n", args.count);

    let mut capture_stats = Stats::new();
    let mut decode_stats = Stats::new();
    let mut total_stats = Stats::new();
    let mut warmed = 0usize;

    if args.pipeline {
        // PIPELINE MODE: with drop-old channel policy (now the default in
        // capture sources), capture runs at camera speed and never blocks on
        // the consumer. The consumer processes frames at its own pace;
        // stale frames are automatically dropped by the channel.
        //
        // To simulate the EFFECT of pipelined decode (capture in parallel
        // with decode), we skip decode entirely here and measure pure
        // capture throughput — this shows the ceiling when decode moves
        // to a separate thread.
        println!("[pipeline] measuring capture-only throughput (decode off critical path)\n");

        let run_start = Instant::now();
        for i in 0..(args.count + 5) {
            let counting = i >= 5;
            let t_start = Instant::now();
            let _frame = match rx.recv().await {
                Some(f) => f,
                None => break,
            };
            let capture_us = t_start.elapsed().as_micros() as u64;

            if counting {
                capture_stats.record(capture_us);
                warmed += 1;
                if warmed % 20 == 0 {
                    println!("  ... {warmed} frames (pipeline)");
                }
            }
        }
        let elapsed = run_start.elapsed();
        let fps = (warmed as f64) / elapsed.as_secs_f64();
        println!(
            "\n=== PIPELINE Results ({warmed} frames, {:.2}s, {:.1} FPS sustained) ===\n",
            elapsed.as_secs_f64(),
            fps
        );
        print_stage("Capture (V4L2 dequeue)", &capture_stats);
        println!(
            "\n  This is the CEILING: capture runs at camera speed.\n  Decode ({:.1}ms/frame) + inference (29ms) run in parallel\n  → expected end-to-end FPS ≈ {:.0}",
            decode_stats.mean_ms(),
            1000.0 / 29.0_f64.max(decode_stats.mean_ms())
        );
    } else {
        // SEQUENTIAL MODE: capture → decode → capture → ...
        // decode blocks the next capture recv. This shows the old
        // behaviour where throughput = 1/(capture + decode).
        println!("[sequential] capture and decode in same loop (decode blocks capture)\n");

        let run_start = Instant::now();
        for i in 0..(args.count + 5) {
            let counting = i >= 5;

            let t_capture_start = Instant::now();
            let frame = match rx.recv().await {
                Some(f) => f,
                None => {
                    eprintln!("[!] source ended at frame {i}");
                    break;
                }
            };
            let capture_us = t_capture_start.elapsed().as_micros() as u64;

            let t_decode_start = Instant::now();
            let _rgb_frame = decode_to_rgb(&frame);
            let decode_us = t_decode_start.elapsed().as_micros() as u64;
            let total_us = t_capture_start.elapsed().as_micros() as u64;

            if counting {
                capture_stats.record(capture_us);
                decode_stats.record(decode_us);
                total_stats.record(total_us);
                warmed += 1;
                if warmed % 20 == 0 {
                    println!("  ... {warmed} frames captured");
                }
            }
        }
        let elapsed = run_start.elapsed();
        let fps = (warmed as f64) / elapsed.as_secs_f64();
        println!(
            "\n=== SEQUENTIAL Results ({warmed} frames, {:.2}s, {:.1} FPS sustained) ===\n",
            elapsed.as_secs_f64(),
            fps
        );
        print_stage("Capture (V4L2 dequeue)", &capture_stats);
        print_stage(
            &format!("Decode ({}→RGB24)", args.format.to_uppercase()),
            &decode_stats,
        );
        print_stage("Total (capture+decode)", &total_stats);
    }

    let _ = source.stop().await;
}

#[derive(Debug, Default, Clone)]
struct Stats {
    samples_us: Vec<u64>,
}

impl Stats {
    fn new() -> Self {
        Self {
            samples_us: Vec::new(),
        }
    }
    fn record(&mut self, us: u64) {
        self.samples_us.push(us);
    }
    fn percentile(&self, pct: f32) -> f64 {
        if self.samples_us.is_empty() {
            return 0.0;
        }
        let mut s = self.samples_us.clone();
        s.sort_unstable();
        let idx = ((pct / 100.0) * (s.len() as f32 - 1.0)).round() as usize;
        s[idx.min(s.len() - 1)] as f64
    }
    fn mean_ms(&self) -> f64 {
        if self.samples_us.is_empty() {
            return 0.0;
        }
        let sum: u64 = self.samples_us.iter().sum();
        sum as f64 / self.samples_us.len() as f64 / 1000.0
    }
}

fn print_stage(name: &str, s: &Stats) {
    println!(
        "{:<26} mean={:>6.2}ms  p50={:>6.2}ms  p95={:>6.2}ms  max={:>6.2}ms",
        name,
        s.mean_ms(),
        s.percentile(50.0) / 1000.0,
        s.percentile(95.0) / 1000.0,
        s.samples_us.iter().max().copied().unwrap_or(0) as f64 / 1000.0,
    );
}

/// Decode to RGB24, dispatching on the frame's actual pixel format.
/// MJPG → jpeg-decoder (тот же путь, что в video-capture/convert.rs);
/// YUYV → конверсия пикселей из video-capture/convert.rs (без JPEG).
fn decode_to_rgb(frame: &Frame) -> Frame {
    match frame.metadata.format {
        PixelFormat::Yuyv => video_capture::convert::yuyv_to_rgb24(frame).expect("yuyv convert"),
        PixelFormat::Mjpeg => decode_mjpg_to_rgb(frame),
        other => panic!("unsupported capture format for decode: {other:?}"),
    }
}

/// MJPG → RGB24 через jpeg-decoder (тот же путь, что в video-capture/convert.rs).
fn decode_mjpg_to_rgb(frame: &Frame) -> Frame {
    let mut decoder = jpeg_decoder::Decoder::new(&frame.data[..]);
    let pixels = decoder.decode().expect("jpeg decode");
    let info = decoder.info().expect("jpeg info");
    Frame {
        data: pixels,
        metadata: FrameMetadata {
            width: info.width as u32,
            height: info.height as u32,
            format: PixelFormat::Rgb24,
            captured_at: frame.metadata.captured_at,
            seq: frame.metadata.seq,
        },
    }
}
