//! Standalone Phase 1.1 example: run the baseline COCO YOLOv8n ONNX model on a
//! single JPEG image and print detections.
//!
//! This closes the "запустить готовую модель на изображении со стенда"
//! criterion of task 1.1 — a minimal end-to-end: file → frame → model →
//! detections.
//!
//! # Requirements
//!
//! - Feature `cpu-onnx` enabled: `--features cpu-onnx`.
//! - An ONNX model at `models/yolov8n.onnx` (run `scripts/download_models.sh`).
//! - A JPEG image (any size; letterboxed to 640 internally).
//!
//! # Usage
//!
//! ```sh
//! cargo run -p cv-inference --example onnx_infer --features cpu-onnx -- \
//!     models/yolov8n.onnx path/to/image.jpg
//! ```
//!
//! # Output
//!
//! Prints one line per detection: `class conf (x,y,w,h)`, then a timing summary.

use std::path::PathBuf;
use std::time::Instant;

use chrono::Utc;
use common::{Frame, FrameMetadata, PixelFormat};
use cv_inference::cpu_onnx::CpuInferenceBackend;
use cv_inference::InferenceBackend;

fn main() -> anyhow::Result<()> {
    // Initialize structured tracing to stderr at info level.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: {} <model.onnx> <image.jpg>", args[0]);
        eprintln!();
        eprintln!("Download the model first:  ./scripts/download_models.sh");
        std::process::exit(2);
    }
    let model_path = PathBuf::from(&args[1]);
    let image_path = PathBuf::from(&args[2]);

    if !model_path.exists() {
        anyhow::bail!(
            "model not found at {}. Run scripts/download_models.sh first.",
            model_path.display()
        );
    }
    if !image_path.exists() {
        anyhow::bail!("image not found at {}", image_path.display());
    }

    // 1) Decode the JPEG into an RGB24 Frame. We use jpeg-decoder (already a
    //    workspace dep via video-capture) to stay in pure Rust.
    let jpeg_bytes = std::fs::read(&image_path)?;
    let t_decode = Instant::now();
    let mut decoder = jpeg_decoder::Decoder::new(&jpeg_bytes[..]);
    let pixels = decoder.decode()?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG has no image info"))?;
    let decode_ms = t_decode.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "[onnx_infer] decoded JPEG {}x{} ({:.1} ms)",
        info.width, info.height, decode_ms
    );

    let frame = Frame {
        data: pixels,
        metadata: FrameMetadata {
            width: info.width as u32,
            height: info.height as u32,
            format: PixelFormat::Rgb24,
            captured_at: Utc::now(),
            seq: 1,
        },
    };

    // 2) Build + init the backend.
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut backend = CpuInferenceBackend::new(model_path.to_str().unwrap());
        let t_init = Instant::now();
        backend.init().await?;
        let init_ms = t_init.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "[onnx_infer] backend '{}' initialized ({:.1} ms)",
            backend.name(),
            init_ms
        );

        // 3) Run inference, measuring latency.
        let t_infer = Instant::now();
        let dets = backend.infer(&frame).await?;
        let infer_ms = t_infer.elapsed().as_secs_f64() * 1000.0;

        // 4) Print detections.
        println!("--- {} detection(s) ---", dets.len());
        for d in &dets {
            println!(
                "{:<14} {:.3}  bbox=({},{},{},{})",
                d.class, d.confidence, d.bbox.x, d.bbox.y, d.bbox.width, d.bbox.height
            );
        }
        println!("--- end ---");
        println!();
        println!("decode: {:.2} ms", decode_ms);
        println!("init:   {:.2} ms (one-time)", init_ms);
        println!("infer:  {:.2} ms", infer_ms);

        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}
