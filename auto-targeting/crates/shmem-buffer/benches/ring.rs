//! Criterion-бенчмарки кольца (TG26-160).
//!
//! Запуск: `cargo bench -p shmem-buffer` (результаты → target/criterion/).
//! Арена в куче — протокол идентичен SHM, цифры переносимы (мемкоп
//! доминирует и в mmap-случае).

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use shmem_buffer::{
    create_in_process, now_ns, FrameConsumer, NextStep, PublishResult, RingConfig, StorageFormat,
};

fn cfg_480p_nv12() -> RingConfig {
    RingConfig {
        capacity: 10,
        width: 640,
        height: 480,
        format: StorageFormat::Nv12,
    }
}

/// Публикация 640x480 NV12 (~460 КБ): ожидание — memcpy-bound.
fn bench_publish(c: &mut Criterion) {
    let prod = create_in_process(&cfg_480p_nv12()).unwrap();
    let frame = vec![0x42u8; prod.config().frame_size() as usize];
    let mut group = c.benchmark_group("publish_640x480_nv12");
    group.throughput(Throughput::Bytes(frame.len() as u64));
    group.bench_function("publish", |b| {
        b.iter(|| {
            black_box(prod.publish(black_box(&frame), now_ns()).unwrap());
        })
    });
    group.finish();
}

/// Взятие+освобождение кадра: два атомика + срез — цель < 1 мкс.
fn bench_acquire_release(c: &mut Criterion) {
    let prod = create_in_process(&cfg_480p_nv12()).unwrap();
    let cons = prod.consumer();
    let frame = vec![0x42u8; prod.config().frame_size() as usize];
    prod.publish(&frame, 1).unwrap();
    // Убедиться, что слот — не последний опубликованный? Публикуем серию,
    // берём latest (сам записываемый путь — CAS 0→1).
    prod.publish(&frame, 2).unwrap();

    c.bench_function("acquire_latest+release", |b| {
        b.iter(|| {
            let g = black_box(cons.latest()).expect("frame");
            black_box(g.len());
        })
    });
}

/// Полный цикл publish → next_after → drop (энд-ту-энд один кадр).
fn bench_roundtrip(c: &mut Criterion) {
    let prod = create_in_process(&cfg_480p_nv12()).unwrap();
    let cons: FrameConsumer = prod.consumer();
    let frame = vec![0x37u8; prod.config().frame_size() as usize];
    let mut last = 0u64;
    c.bench_function("publish+consume_roundtrip", |b| {
        b.iter(|| {
            if let PublishResult::Published { .. } =
                prod.publish(black_box(&frame), now_ns()).unwrap()
            {
                if let NextStep::Frame(g) = cons.next_after(last) {
                    last = g.frame_id();
                    black_box(&g[..32]);
                }
            }
        })
    });
}

criterion_group!(benches, bench_publish, bench_acquire_release, bench_roundtrip);
criterion_main!(benches);
