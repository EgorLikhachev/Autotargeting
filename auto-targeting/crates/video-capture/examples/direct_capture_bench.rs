//! Minimal benchmark for V4l2DirectSource (direct libc ioctl).
//! Compile: cargo run --release -p video-capture --features v4l2-direct \
//!     --example direct_capture_bench -- --device /dev/video0

#![cfg(feature = "v4l2-direct")]

use clap::Parser;
use common::PixelFormat;
use video_capture::{V4l2DirectSource, VideoSource};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "/dev/video0")]
    device: String,
    #[arg(long, default_value_t = 640)]
    width: u32,
    #[arg(long, default_value_t = 480)]
    height: u32,
    #[arg(long, default_value_t = 100)]
    fps: u32,
    #[arg(long, default_value_t = 100)]
    count: usize,
}

fn main() {
    // Init tracing to stderr to see capture thread errors.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::DEBUG.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    println!("=== Direct V4L2 ioctl Benchmark ===");
    println!(
        "{} {}x{} @ {}fps, {} frames",
        args.device, args.width, args.height, args.fps, args.count
    );

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut src = V4l2DirectSource::new(&args.device, args.width, args.height, args.fps)
            .with_format(PixelFormat::Mjpeg)
            .with_buffers(4);
        let mut rx = src.start().await.expect("start failed");
        println!("[+] Streaming. Capturing...");
        // warmup 5
        for _ in 0..5 {
            let _ = rx.recv().await;
        }
        let start = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..args.count {
            match rx.recv().await {
                Some(f) => {
                    n += 1;
                    if n % 20 == 0 {
                        println!("  ... {n} frames");
                    }
                    let _ = f;
                }
                None => break,
            }
        }
        let elapsed = start.elapsed();
        let fps = n as f64 / elapsed.as_secs_f64();
        let interval = elapsed.as_millis() as f64 / n as f64;
        println!("\n=== DIRECT ioctl Results ===");
        println!("  {n} frames in {:.2}s", elapsed.as_secs_f64());
        println!("  sustained FPS: {:.1}", fps);
        println!("  mean frame interval: {:.1}ms", interval);
        println!("  expected capture latency: ~{:.0}ms", interval / 2.0);
        let _ = src.stop().await;
    });
}
