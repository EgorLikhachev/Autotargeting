//! detector — CLI-обёртка компонента детекции (TG26-35).
//!
//! На стенде (подняты camera_publisher и rknn-bridge):
//!   detector --segment autotarget.frames --backend bridge \
//!       --model ~/auto-targeting/auto-targeting/models/yolov8n_int8.rknn
//!
//! x86-dev без NPU: --backend cpu-onnx (feature cpu-onnx) или --backend mock.

use clap::Parser;
use detector::{BackendKind, Detector, DetectorConfig};

#[derive(Parser)]
struct Args {
    /// Имя сегмента SHM-кольца.
    #[arg(long, default_value = "autotarget.frames")]
    segment: String,
    /// Инференс-бэкенд: bridge | cpu-onnx | mock.
    #[arg(long, default_value = "bridge")]
    backend: String,
    #[arg(long, default_value = "/opt/auto-targeting/models/yolov8n_int8.rknn")]
    model: String,
    #[arg(long, default_value_t = 0.45)]
    conf: f32,
    #[arg(long, default_value_t = 0.45)]
    nms: f32,
    #[arg(long, default_value = "/tmp/rknn-bridge.sock")]
    bridge_socket: String,
    /// Endpoint шины (zenoh).
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    bus: String,
    /// Прекратить после N секунд (0 — пока жив стрим).
    #[arg(long, default_value_t = 0)]
    seconds: u64,
    /// Тишина кольца до завершения, сек.
    #[arg(long, default_value_t = 5)]
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
    let backend = match args.backend.as_str() {
        "bridge" | "npu" => BackendKind::Bridge,
        "cpu-onnx" | "onnx" => BackendKind::CpuOnnx,
        "mock" => BackendKind::Mock,
        other => {
            eprintln!("[!] unknown --backend '{other}' (bridge | cpu-onnx | mock)");
            std::process::exit(2);
        }
    };
    let cfg = DetectorConfig {
        segment: args.segment.clone(),
        model_path: args.model.clone(),
        backend,
        confidence_threshold: args.conf,
        nms_threshold: args.nms,
        bridge_socket: args.bridge_socket.clone(),
        quiet_timeout: std::time::Duration::from_secs(args.quiet_timeout.max(1)),
        max_duration: (args.seconds > 0).then(|| std::time::Duration::from_secs(args.seconds)),
        ..DetectorConfig::default()
    };

    println!(
        "=== detector === segment={} backend={:?} model={} bus={}",
        cfg.segment, cfg.backend, cfg.model_path, args.bus
    );

    let consumer = match shmem_buffer::attach_shared(&cfg.segment) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[!] ring attach failed: {e}");
            std::process::exit(3);
        }
    };
    let bus = match event_bus::EventBus::connect(&args.bus).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[!] bus connect failed: {e}");
            std::process::exit(4);
        }
    };
    let detector = match Detector::new(cfg) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[!] config: {e}");
            std::process::exit(5);
        }
    };

    match detector.run(&consumer, &bus).await {
        Ok(stats) => {
            println!(
                "[summary] PROCESSED={} PUBLISHED={} JUMPS={} INFER_ERRORS={} DETECTIONS={}",
                stats.frames_processed,
                stats.frames_published,
                stats.jumps,
                stats.infer_errors,
                stats.detections_total
            );
        }
        Err(e) => {
            eprintln!("[!] detector failed: {e}");
            std::process::exit(6);
        }
    }
    // Порядок важен: сначала шина, потом drop бэкенда (Drop bridge-клиента
    // завершает процесс rknn-bridge shutdown-сообщением).
    let _ = bus.close().await;
}
