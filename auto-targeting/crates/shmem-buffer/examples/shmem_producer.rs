//! Продюсер кадров в разделяемую память (TG26-160).
//!
//! Linux (мультипроцессно):
//!   cargo run --release -p shmem-buffer --example shmem_producer -- \
//!       --name autotarget.frames --fps 30 --seconds 10
//! Потребители параллельно:
//!   cargo run --release -p shmem-buffer --example shmem_consumer -- \
//!       --name autotarget.frames --mode next
//!
//! Паттерн кадра: каждое u32-слово = frame_id (детектор torn-read у consumer).

use clap::Parser;
use shmem_buffer::{create_in_process, create_shared, now_ns, PublishResult, RingConfig, StorageFormat};

#[derive(Parser)]
struct Args {
    /// Имя сегмента в /dev/shm.
    #[arg(long, default_value = "autotarget.frames")]
    name: String,
    #[arg(long, default_value_t = 10)]
    capacity: u32,
    #[arg(long, default_value_t = 640)]
    width: u32,
    #[arg(long, default_value_t = 480)]
    height: u32,
    #[arg(long, default_value_t = 30)]
    fps: u32,
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// mjpeg-подобный выбор формата хранения: nv12 | rgb24.
    #[arg(long, default_value = "nv12")]
    format: String,
    /// Внутрипроцессный режим (без SHM) — для quick-check на любом хосте.
    #[arg(long, default_value_t = false)]
    in_process: bool,
}

fn main() {
    let args = Args::parse();
    let format = match args.format.as_str() {
        "nv12" => StorageFormat::Nv12,
        "rgb24" => StorageFormat::Rgb24,
        other => {
            eprintln!("[!] unknown --format {other}");
            std::process::exit(2);
        }
    };
    let cfg = RingConfig {
        capacity: args.capacity,
        width: args.width,
        height: args.height,
        format,
    };
    println!(
        "=== shmem producer === {} {}x{} @{}fps cap={} fmt={:?} {}s",
        args.name, args.width, args.height, args.fps, args.capacity, format, args.seconds
    );

    let producer = if args.in_process {
        create_in_process(&cfg).expect("in-process ring")
    } else {
        match create_shared(&args.name, &cfg) {
            Ok(p) => p,
            Err(shmem_buffer::RingError::UnsupportedOs) => {
                eprintln!("[!] shared memory is Linux-only; use --in-process on this host");
                std::process::exit(2);
            }
            Err(shmem_buffer::RingError::SegmentExists(n)) => {
                eprintln!("[!] segment '{n}' already exists (stale producer?); remove: rm /dev/shm/{n}");
                std::process::exit(3);
            }
            Err(e) => panic!("create_shared: {e}"),
        }
    };

    let frame_size = producer.config().frame_size() as usize;
    println!("[+] ring created: frame_size={frame_size} B, segment in /dev/shm/{}", args.name);

    let period = std::time::Duration::from_nanos(1_000_000_000 / u64::from(args.fps.max(1)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(args.seconds);
    let mut pattern_buf = vec![0u8; frame_size];
    let mut next_report = std::time::Instant::now();

    while std::time::Instant::now() < deadline {
        let seq = producer.stats().published + 1; // следующий id (для паттерна)
        // Паттерн: каждое u32-слово = frame_id — детектор torn-read.
        for w in pattern_buf.chunks_exact_mut(4) {
            w.copy_from_slice(&(seq as u32).to_le_bytes());
        }
        match producer.publish(&pattern_buf, now_ns()) {
            Ok(PublishResult::Published { .. }) => {}
            Ok(PublishResult::Dropped { reason }) => {
                if std::time::Instant::now() >= next_report {
                    eprintln!("    [drop] {reason:?}");
                    next_report += std::time::Duration::from_secs(2);
                }
            }
            Err(e) => panic!("publish: {e}"),
        }
        std::thread::sleep(period);
    }

    let s = producer.stats();
    println!(
        "[summary] published={} dropped={}",
        s.published, s.dropped
    );
}
