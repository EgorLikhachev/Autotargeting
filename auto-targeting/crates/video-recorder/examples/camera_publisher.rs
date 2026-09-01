//! camera_publisher — РЕАЛЬНЫЙ продюсер для SHM-кольца: камера → NV12 → ring.
//!
//! Дополняет синтетический `shmem_producer` живым видеоисточником —
//! проверка TG26-125 «по-настоящему»: захват V4l2DirectSource →
//! конверсия в NV12 → `FrameProducer::publish` (drop-new).
//!
//! Запуск НА Orange Pi 5:
//!   cargo run --release -p video-recorder --features v4l2-direct-cam \
//!       --example camera_publisher -- --name at125.frames --seconds 30
//!
//! PS Eye (gspca): только `--format yuyv` и `--backend` всегда direct
//! (v4l-crate зависает на gspca). Arducam: `--format mjpeg`.

/// M1: статус camera_publisher на шине (at/status/camera).
#[cfg(feature = "v4l2-direct-cam")]
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct CameraStatus {
    v: u8,
    device: String,
    width: u32,
    height: u32,
    fps_target: u32,
    format: String,
    fps_actual: f32,
    published: u64,
    dropped: u64,
    convert_errors: u64,
}

#[cfg(feature = "v4l2-direct-cam")]
fn main() {
    use clap::Parser;

    /// Камера → NV12 → SHM-кольцо.
    #[derive(Parser)]
    struct Args {
        /// Имя сегмента SHM.
        #[arg(long, default_value = "autotarget.frames")]
        name: String,
        #[arg(long, default_value = "/dev/video0")]
        device: String,
        #[arg(long, default_value_t = 640)]
        width: u32,
        #[arg(long, default_value_t = 480)]
        height: u32,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        #[arg(long, default_value_t = 10)]
        capacity: u32,
        #[arg(long, default_value_t = 30)]
        seconds: u64,
        /// Формат захвата: yuyv (PS Eye) | mjpeg (Arducam).
        #[arg(long, default_value = "yuyv")]
        format: String,
        /// Endpoint шины zenoh для публикации at/status/camera (пусто — выкл).
        #[arg(long)]
        bus: Option<String>,
    }

    let args = Args::parse();
    let src_format = match args.format.as_str() {
        "yuyv" => common::PixelFormat::Yuyv,
        "mjpeg" => common::PixelFormat::Mjpeg,
        other => {
            eprintln!("[!] unknown --format '{other}' (yuyv | mjpeg)");
            std::process::exit(2);
        }
    };

    println!(
        "=== camera_publisher === {} {}x{}@{} fmt={} cap={} {}s -> /dev/shm/{}",
        args.device,
        args.width,
        args.height,
        args.fps,
        args.format,
        args.capacity,
        args.seconds,
        args.name
    );

    let ring_cfg = shmem_buffer::RingConfig {
        capacity: args.capacity,
        width: args.width,
        height: args.height,
        format: shmem_buffer::StorageFormat::Nv12,
    };
    // Сервисный контекст: осиротевший сегмент от крэшнутого экземпляра —
    // норма; мы владелец, пересоздаём (attach-потребители переподключатся
    // по quiet-timeout/restart).
    let _ = shmem_buffer::remove_segment(&args.name);
    let producer = match shmem_buffer::create_shared(&args.name, &ring_cfg) {
        Ok(p) => p,
        Err(shmem_buffer::RingError::SegmentExists(n)) => {
            eprintln!("[!] segment '{n}' exists (stale publisher?): rm /dev/shm/{n}");
            std::process::exit(3);
        }
        Err(e) => panic!("create_shared: {e}"),
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio rt");
    rt.block_on(async {
        use video_capture::{V4l2DirectSource, VideoSource};

        // M1: статус на шину (опционально).
        let status_pub = match &args.bus {
            Some(ep) => match event_bus::EventBus::connect(ep).await {
                Ok(bus) => {
                    let p = bus
                        .publisher::<CameraStatus>(&event_bus::topics::status("camera"))
                        .await
                        .ok();
                    Some((bus, p))
                }
                Err(e) => {
                    eprintln!("[warn] bus connect failed: {e}");
                    None
                }
            },
            None => None,
        };

        let mut src = V4l2DirectSource::new(&args.device, args.width, args.height, args.fps)
            .with_format(src_format)
            .with_buffers(4);
        let mut rx = match src.start().await {
            Ok(rx) => rx,
            Err(e) => {
                eprintln!("[!] capture start failed: {e}");
                std::process::exit(4);
            }
        };
        println!("[+] capture streaming");

        // 0 = бессрочно (сервисный режим; детектор трактует так же).
        let deadline = if args.seconds == 0 {
            std::time::Instant::now() + std::time::Duration::from_secs(86_400 * 365)
        } else {
            std::time::Instant::now() + std::time::Duration::from_secs(args.seconds)
        };
        let mut convert_errors: u64 = 0;
        let mut window_frames: u64 = 0;
        let mut window_start = std::time::Instant::now();
        let mut last_status = std::time::Instant::now();
        while std::time::Instant::now() < deadline {
            let Some(frame) = rx.recv().await else { break };
            window_frames += 1;
            // Конверсия в NV12 — формат хранения кольца.
            let nv12 = match src_format {
                common::PixelFormat::Yuyv => video_capture::yuyv_to_nv12(&frame),
                common::PixelFormat::Mjpeg => video_capture::decode_mjpeg_to_nv12(&frame),
                _ => unreachable!(),
            };
            let nv12 = match nv12 {
                Ok(f) => f,
                Err(e) => {
                    convert_errors += 1;
                    eprintln!("[warn] convert: {e}");
                    continue;
                }
            };
            match producer.publish(&nv12.data, shmem_buffer::now_ns()) {
                Ok(shmem_buffer::PublishResult::Published { .. }) => {}
                Ok(shmem_buffer::PublishResult::Dropped { reason }) => {
                    tracing::debug!(?reason, "drop-new");
                }
                Err(e) => {
                    eprintln!("[!] publish: {e}");
                    break;
                }
            }

            // M1: статус раз в секунду.
            if last_status.elapsed() >= std::time::Duration::from_secs(1) {
                let s = producer.stats();
                let secs = window_start.elapsed().as_secs_f32();
                let fps = if secs > 0.0 {
                    window_frames as f32 / secs
                } else {
                    0.0
                };
                if let Some((_, Some(p))) = &status_pub {
                    let st = CameraStatus {
                        v: event_bus::CONTRACT_VERSION,
                        device: args.device.clone(),
                        width: args.width,
                        height: args.height,
                        fps_target: args.fps,
                        format: args.format.clone(),
                        fps_actual: fps,
                        published: s.published,
                        dropped: s.dropped,
                        convert_errors,
                    };
                    let _ = p.publish(&st).await;
                }
                window_frames = 0;
                window_start = std::time::Instant::now();
                last_status = std::time::Instant::now();
            }
        }
        let _ = src.stop().await;

        // Финальный статус.
        if let Some((bus, Some(p))) = status_pub {
            let s = producer.stats();
            let st = CameraStatus {
                v: event_bus::CONTRACT_VERSION,
                device: args.device.clone(),
                width: args.width,
                height: args.height,
                fps_target: args.fps,
                format: args.format.clone(),
                fps_actual: 0.0,
                published: s.published,
                dropped: s.dropped,
                convert_errors,
            };
            let _ = p.publish(&st).await;
            let _ = bus.close().await;
        }
    });

    let s = producer.stats();
    println!("[summary] published={} dropped={}", s.published, s.dropped);
}

#[cfg(not(feature = "v4l2-direct-cam"))]
fn main() {
    eprintln!("camera_publisher requires capture support:");
    eprintln!("  cargo run --release -p video-recorder --features v4l2-direct-cam --example camera_publisher -- <args>");
    std::process::exit(2);
}
