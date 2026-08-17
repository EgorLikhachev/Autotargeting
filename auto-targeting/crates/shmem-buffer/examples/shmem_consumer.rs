//! Потребитель кадров из разделяемой памяти (TG26-160).
//!
//!   cargo run --release -p shmem-buffer --example shmem_consumer -- \
//!       --name autotarget.frames --mode next --seconds 10
//!
//! Режимы:
//!   latest — всегда свежий кадр (детектор/классификатор);
//!   next   — сквозная последовательность (трекер), с прыжком при отставании;
//!   slow   — как latest, но держит кадр --hold-ms (эмуляция медленного
//!            рекордера: проверяет, что кадр не перезаписывается под ногами).
//!
//! Каждый кадр верифицируется (все u32-слова == frame_id): детектор torn-read.
//! Итог печатается машиночитаемо: `VERIFIED=<n> TORN=0 JUMPS=<n>`.

use clap::Parser;
use shmem_buffer::{attach_shared, NextStep};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "autotarget.frames")]
    name: String,
    #[arg(long, default_value = "next")]
    mode: String,
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// Время удержания кадра (мс) — режим slow.
    #[arg(long, default_value_t = 200)]
    hold_ms: u64,
}

fn main() {
    let args = Args::parse();
    println!(
        "=== shmem consumer === {} mode={} hold={}ms",
        args.name, args.mode, args.hold_ms
    );
    let consumer = match attach_shared(&args.name) {
        Ok(c) => c,
        Err(shmem_buffer::RingError::UnsupportedOs) => {
            eprintln!("[!] shared memory is Linux-only");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("[!] attach failed: {e}");
            std::process::exit(3);
        }
    };
    println!(
        "[+] attached: capacity={} latest_id={}",
        consumer.capacity(),
        consumer.latest_id()
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(args.seconds);
    let mut verified: u64 = 0;
    let mut torn: u64 = 0;
    let mut jumps: u64 = 0;
    let mut last: u64 = 0;

    while std::time::Instant::now() < deadline {
        let guard = match args.mode.as_str() {
            "latest" | "slow" => consumer.latest(),
            "next" => loop {
                if std::time::Instant::now() > deadline {
                    break None;
                }
                match consumer.next_after(last) {
                    NextStep::Frame(g) => break Some(g),
                    NextStep::UpToDate => {
                        std::thread::yield_now();
                        continue;
                    }
                    NextStep::TooFarBehind { latest, .. } => {
                        jumps += 1;
                        last = latest;
                        continue;
                    }
                }
            },
            other => {
                eprintln!("[!] unknown --mode {other}");
                std::process::exit(2);
            }
        };
        let Some(g) = guard else { break };

        let id = g.frame_id();
        let bad = g
            .chunks_exact(4)
            .any(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) != id as u32);
        if bad {
            torn += 1;
            eprintln!("[!!] TORN READ in frame {id}");
        }
        verified += 1;
        last = last.max(id);

        if args.mode == "slow" {
            std::thread::sleep(std::time::Duration::from_millis(args.hold_ms));
        }
        drop(g);
    }

    println!(
        "[summary] VERIFIED={verified} TORN={torn} JUMPS={jumps} last={last} dropped_by_producer={}",
        consumer.dropped_frames()
    );
    if torn > 0 {
        std::process::exit(4);
    }
}
