//! Ример: освобождение слотов, зависших после крэшей потребителей (TG26-160).
//!
//!   cargo run --release -p shmem-buffer --example shmem_reaper -- \
//!       --name autotarget.frames --max-age-sec 10
//!
//! Инструмент оператора: убирает ref_count, утёкший из-за процессов,
//! умерших с живым FrameGuard, и WRITER_LOCK от мёртвого продюсера.

use clap::Parser;
use shmem_buffer::{attach_shared, recover_stale_slots};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "autotarget.frames")]
    name: String,
    /// Слот считается протухшим, если кадр старше этого возраста И
    /// держатель мёртв (двойная проверка против живых читателей).
    #[arg(long, default_value_t = 10)]
    max_age_sec: u64,
}

fn main() {
    let args = Args::parse();
    let consumer = match attach_shared(&args.name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[!] attach failed: {e}");
            std::process::exit(3);
        }
    };
    let freed = recover_stale_slots(&consumer, args.max_age_sec * 1_000_000_000);
    println!(
        "[summary] REAPED={freed} latest_id={}",
        consumer.latest_id()
    );
}
