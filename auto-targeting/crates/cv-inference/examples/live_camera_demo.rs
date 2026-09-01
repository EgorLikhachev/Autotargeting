//! LIVE CAMERA DEMO: камера → NPU → детекции → аннотированные JPEG → MP4.
//!
//! Захватывает кадры с USB-камеры через V4l2DirectSource, декодирует MJPG,
//! отправляет в rknn-bridge (NPU), получает детекции, рисует bbox и сохраняет
//! аннотированные кадры в output/live/frames/. Затем ffmpeg собирает MP4.
//!
//! Запуск НА Orange Pi 5:
//!   # 1. Запустить rknn-bridge в фоне
//!   cd ~/auto-targeting/auto-targeting/rknn-bridge/build
//!   nohup ./rknn-bridge >/tmp/bridge.log 2>&1 &
//!   # 2. Запустить этот пример
//!   cargo run --release -p cv-inference --example live_camera_demo \
//!       --features "cpu-onnx" -- --seconds 15 --device /dev/video0

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use cv_visualizer::FrameWriter;
use parking_lot::Mutex;

/// Live camera demo with NPU inference.
#[derive(Parser)]
struct Args {
    /// Duration in seconds.
    #[arg(long, default_value_t = 15)]
    seconds: u64,
    /// V4L2 device.
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
    /// Output dir.
    #[arg(long, default_value = "output/live")]
    output: PathBuf,
    /// Font path for labels.
    #[arg(long)]
    font: Option<PathBuf>,
    /// Capture pixel format: "mjpeg" (Arducam OV9782) or "yuyv" (PS Eye).
    #[arg(long, default_value = "mjpeg")]
    format: String,
    /// Capture backend: "v4l" (v4l crate) or "direct" (raw ioctl; needs
    /// feature v4l2-direct-cam). PS Eye (gspca/ov534) requires "direct".
    #[arg(long, default_value = "v4l")]
    backend: String,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    std::fs::create_dir_all(&args.output)?;

    println!("=== LIVE CAMERA DEMO ===");
    println!(
        "Device: {} {}x{} @ {}fps, {}s",
        args.device, args.width, args.height, args.fps, args.seconds
    );

    // Check rknn-bridge is running.
    let bridge_sock = std::path::Path::new("/tmp/rknn-bridge.sock");
    if !bridge_sock.exists() {
        anyhow::bail!("rknn-bridge socket not found at /tmp/rknn-bridge.sock. Start it first.");
    }
    println!("[+] rknn-bridge socket found");

    // Build visualizer.
    let mut writer = FrameWriter::new(&args.output, 1)?; // save every frame
    if let Some(font) = &args.font {
        writer = writer.with_font_path(font)?;
    }
    let writer = Arc::new(Mutex::new(writer));

    // Telemetry file.
    let telemetry_path = args.output.join("telemetry.jsonl");

    #[cfg(unix)]
    {
        run_live_demo(args, writer, telemetry_path)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (writer, telemetry_path);
        eprintln!("Live camera demo requires Unix (for UnixStream + V4L2). Not available on this platform.");
    }
    Ok(())
}

/// Прокачка кадров из source-канала в demo-канал.
///
/// Break — ТОЛЬКО когда consumer отвалился (Closed). Full (consumer занят
/// инференсом) — это НОРМАЛЬНАЯ ситуация для realtime: дропаем кадр и
/// продолжаем захват. Раньше здесь был `try_send(frame).is_err() => break`,
/// который рвал захват на 5-м кадре (канал depth=4 + 1 в полёте), как только
/// инференс оказывался медленнее камеры.
#[cfg(unix)]
async fn pump_frames(
    rx: &mut tokio::sync::mpsc::Receiver<Frame>,
    tx: &tokio::sync::mpsc::Sender<Frame>,
) {
    use tokio::sync::mpsc::error::TrySendError;
    while let Some(frame) = rx.recv().await {
        match tx.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Closed(_)) => break, // consumer dropped
            Err(TrySendError::Full(_)) => {}       // drop frame, keep capturing
        }
    }
}

#[cfg(unix)]
fn run_live_demo(
    args: Args,
    writer: Arc<Mutex<FrameWriter>>,
    telemetry_path: PathBuf,
) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    // Build V4L2 capture source.
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<Frame>(4);

    let dev = args.device.clone();
    let w = args.width;
    let h = args.height;
    let fps_val = args.fps;
    let pixel_format = match args.format.as_str() {
        "mjpeg" | "jpg" => PixelFormat::Mjpeg,
        "yuyv" | "yuv422" => PixelFormat::Yuyv,
        other => anyhow::bail!("unknown --format '{other}' (expected: mjpeg | yuyv)"),
    };
    let backend = args.backend.clone();
    std::thread::spawn(move || {
        use video_capture::VideoSource;
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            match backend.as_str() {
                "direct" => {
                    #[cfg(feature = "v4l2-direct-cam")]
                    {
                        let mut src = video_capture::V4l2DirectSource::new(&dev, w, h, fps_val)
                            .with_format(pixel_format)
                            .with_buffers(4);
                        match src.start().await {
                            Ok(mut rx) => pump_frames(&mut rx, &frame_tx).await,
                            Err(e) => eprintln!("[!] V4l2DirectSource error: {e}"),
                        }
                        let _ = src.stop().await;
                    }
                    #[cfg(not(feature = "v4l2-direct-cam"))]
                    eprintln!("[!] --backend direct requires feature v4l2-direct-cam");
                }
                _ => {
                    #[cfg(feature = "v4l2-cam")]
                    {
                        let mut src = video_capture::V4l2Source::new(&dev, w, h, fps_val)
                            .with_format(pixel_format);
                        match src.start().await {
                            Ok(mut rx) => pump_frames(&mut rx, &frame_tx).await,
                            Err(e) => eprintln!("[!] V4L2 source error: {e}"),
                        }
                        let _ = src.stop().await;
                    }
                    #[cfg(not(feature = "v4l2-cam"))]
                    eprintln!("[!] default --backend v4l requires feature v4l2-cam");
                }
            }
        });
    });

    // Connect to rknn-bridge.
    println!("[*] Connecting to rknn-bridge...");
    let mut sock = UnixStream::connect("/tmp/rknn-bridge.sock")?;

    // INIT
    let model = "/home/orangepi/auto-targeting/auto-targeting/models/yolov8n_int8.rknn";
    let init_msg = format!(
        r#"{{"type":"init","model_path":"{}","input_width":640,"input_height":640,"input_format":"rgb24","confidence_threshold":0.35,"nms_threshold":0.45}}"#,
        model
    );
    let init_bytes = init_msg.as_bytes();
    sock.write_all(&(init_bytes.len() as u32).to_be_bytes())?;
    sock.write_all(init_bytes)?;

    let mut len_buf = [0u8; 4];
    sock.read_exact(&mut len_buf)?;
    let resp_len = u32::from_be_bytes(len_buf) as usize;
    let mut resp_buf = vec![0u8; resp_len];
    sock.read_exact(&mut resp_buf)?;
    let resp_str = String::from_utf8_lossy(&resp_buf);
    println!(
        "[+] init response: {}",
        &resp_str[..resp_str.len().min(200)]
    );
    if !resp_str.contains(r#""ok":true"#) {
        anyhow::bail!("init failed");
    }

    // MAIN LOOP
    println!(
        "[+] Starting capture+inference loop for {}s...",
        args.seconds
    );
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let start = Instant::now();
    let mut total_frames = 0u64;
    let mut total_detections = 0u64;
    let mut last_telemetry = Instant::now();
    let mut latencies: Vec<u64> = Vec::new();

    while Instant::now() < deadline {
        let frame = match frame_rx.blocking_recv() {
            Some(f) => f,
            None => {
                eprintln!("[!] Frame source ended");
                break;
            }
        };
        total_frames += 1;

        // Decode → RGB24: MJPG через jpeg-decoder, YUYV через конверсию пикселей.
        let rgb_frame = match frame.metadata.format {
            PixelFormat::Yuyv => match video_capture::convert::yuyv_to_rgb24(&frame) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("yuyv convert err: {e}");
                    continue;
                }
            },
            PixelFormat::Mjpeg => {
                let mut decoder = jpeg_decoder::Decoder::new(&frame.data[..]);
                let pixels = match decoder.decode() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("decode err: {e}");
                        continue;
                    }
                };
                let info = decoder.info().unwrap();
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
            other => {
                eprintln!("unsupported capture format: {other:?}");
                continue;
            }
        };

        // Resize to 640x640 for NPU (simple stretch for demo)
        // In production: letterbox. Here: just send raw — NPU will accept it
        // if width=height=640. For 640x480 we need letterbox, skip for simplicity.
        let (rw, rh) = (rgb_frame.metadata.width, rgb_frame.metadata.height);
        let send_frame = if rw != 640 || rh != 640 {
            // Quick resize via image crate
            let img = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_raw(
                rw,
                rh,
                rgb_frame.data.clone(),
            )
            .unwrap();
            let resized =
                image::imageops::resize(&img, 640, 640, image::imageops::FilterType::Nearest);
            Frame {
                data: resized.into_raw(),
                metadata: FrameMetadata {
                    width: 640,
                    height: 640,
                    format: PixelFormat::Rgb24,
                    captured_at: rgb_frame.metadata.captured_at,
                    seq: rgb_frame.metadata.seq,
                },
            }
        } else {
            rgb_frame.clone()
        };

        // Send to rknn-bridge for inference
        let t_infer_start = Instant::now();
        use base64::{engine::general_purpose, Engine as _};
        let frame_b64 = general_purpose::STANDARD.encode(&send_frame.data);
        let infer_msg = format!(
            r#"{{"type":"infer","frame_seq":{},"captured_at_ms":{},"frame_data_b64":"{}"}}"#,
            total_frames,
            chrono::Utc::now().timestamp_millis(),
            frame_b64
        );
        let infer_bytes = infer_msg.as_bytes();
        if sock
            .write_all(&(infer_bytes.len() as u32).to_be_bytes())
            .is_err()
        {
            break;
        }
        if sock.write_all(infer_bytes).is_err() {
            break;
        }

        // Read response
        if sock.read_exact(&mut len_buf).is_err() {
            break;
        }
        let resp_len = u32::from_be_bytes(len_buf) as usize;
        if resp_len > 10_000_000 {
            break;
        } // sanity
        let mut resp_buf = vec![0u8; resp_len];
        if sock.read_exact(&mut resp_buf).is_err() {
            break;
        }
        let infer_resp = String::from_utf8_lossy(&resp_buf);

        let infer_ms = t_infer_start.elapsed().as_millis() as u64;
        latencies.push(infer_ms);

        // Parse detections count
        let n_dets = infer_resp.matches("\"bbox\"").count();
        total_detections += n_dets as u64;

        // Parse detections (simple JSON extraction)
        let detections = parse_detections(&infer_resp, total_frames);

        // Annotate + save frame
        {
            let mut w = writer.lock();
            if let Err(e) = w.save(&send_frame, &detections) {
                eprintln!("save err: {e}");
            }
        }

        // Progress
        let elapsed = start.elapsed().as_secs_f64();
        if total_frames % 30 == 0 {
            println!(
                "  frame {}: {:.1}s elapsed, {} detections so far, last infer={}ms",
                total_frames, elapsed, total_detections, infer_ms
            );
        }

        // Telemetry every 5s
        if last_telemetry.elapsed() >= Duration::from_secs(5) {
            let sample = TelemetrySample::capture();
            let line = serde_json::to_string(&sample)?;
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&telemetry_path)?;
            writeln!(f, "{line}")?;
            last_telemetry = Instant::now();
        }
    }

    // SHUTDOWN bridge
    let shutdown_msg = r#"{"type":"shutdown"}"#;
    let _ = sock.write_all(&(shutdown_msg.len() as u32).to_be_bytes());
    let _ = sock.write_all(shutdown_msg.as_bytes());

    // Summary
    let elapsed = start.elapsed().as_secs_f64();
    let avg_latency = if !latencies.is_empty() {
        latencies.iter().sum::<u64>() as f64 / latencies.len() as f64
    } else {
        0.0
    };
    let max_latency = latencies.iter().max().copied().unwrap_or(0);
    let min_latency = latencies.iter().min().copied().unwrap_or(0);

    println!("\n=== LIVE CAMERA DEMO SUMMARY ===");
    println!("Duration:        {:.1}s", elapsed);
    println!("Frames captured: {}", total_frames);
    println!("Sustained FPS:   {:.1}", total_frames as f64 / elapsed);
    println!("Total detections: {}", total_detections);
    println!(
        "Inference latency: avg={:.0}ms  min={}ms  max={}ms",
        avg_latency, min_latency, max_latency
    );

    // Final telemetry
    let final_sample = TelemetrySample::capture();
    println!("\nFinal telemetry:");
    println!("  RSS:    {} KB", final_sample.rss_kb.unwrap_or(0));
    println!("  CPU:    {} °C", final_sample.cpu_temp_c.unwrap_or(0.0));
    println!("  NPU:    {} °C", final_sample.npu_temp_c.unwrap_or(0.0));

    // Save summary
    let summary = serde_json::json!({
        "duration_s": elapsed,
        "frames_captured": total_frames,
        "sustained_fps": total_frames as f64 / elapsed,
        "total_detections": total_detections,
        "inference_latency_ms": {
            "avg": avg_latency,
            "min": min_latency,
            "max": max_latency,
        },
        "telemetry": {
            "rss_kb": final_sample.rss_kb,
            "cpu_temp_c": final_sample.cpu_temp_c,
            "npu_temp_c": final_sample.npu_temp_c,
        }
    });
    let summary_path = args.output.join("summary.json");
    std::fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)?;
    println!("\nArtifacts:");
    println!("  {}", args.output.join("frames/").display());
    println!("  {}", summary_path.display());
    println!("  {}", telemetry_path.display());
    println!(
        "\nNext: ./scripts/make_video.sh {} 30",
        args.output.display()
    );

    Ok(())
}

/// Simple JSON detection parser (extracts bbox/class/confidence).
fn parse_detections(json: &str, frame_seq: u64) -> Vec<common::Detection> {
    let mut dets = Vec::new();
    let now = chrono::Utc::now();
    // Very basic string-based extraction. Not robust, but works for our compact JSON.
    let mut pos = 0;
    while let Some(bbox_start) = json[pos..].find("\"bbox\":{") {
        let abs_start = pos + bbox_start;
        // Find class
        if let Some(class_pos) = json[abs_start..].find("\"class\":\"") {
            let cs = abs_start + class_pos + 9;
            let ce = json[cs..].find('"').map(|e| cs + e).unwrap_or(cs);
            let class = &json[cs..ce];
            // Find confidence
            if let Some(conf_pos) = json[ce..].find("\"confidence\":") {
                let conf_start = ce + conf_pos + 13;
                let conf_end = json[conf_start..]
                    .find(|c: char| !c.is_ascii_digit() && c != '.')
                    .map(|e| conf_start + e)
                    .unwrap_or(conf_start + 5);
                let conf: f32 = json[conf_start..conf_end].parse().unwrap_or(0.0);
                dets.push(common::Detection {
                    bbox: common::BoundingBox {
                        x: 0,
                        y: 0,
                        width: 640,
                        height: 640,
                    },
                    class: class.to_string(),
                    class_id: 0,
                    confidence: conf,
                    frame_seq,
                    detected_at: now,
                });
            }
        }
        pos = abs_start + 10;
    }
    dets
}
