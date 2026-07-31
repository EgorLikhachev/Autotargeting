//! Benchmarks for the TargetTracker — the per-frame update cycle.
//!
//! Run with: `cargo bench -p target-tracker --bench tracker`
//! Results land in `target/criterion/`.

use common::{BoundingBox, Detection};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use target_tracker::TargetTracker;

fn make_detection(seq: u64, x: u32, y: u32) -> Detection {
    Detection {
        bbox: BoundingBox {
            x,
            y,
            width: 60,
            height: 120,
        },
        class: "person".to_string(),
        class_id: 0,
        confidence: 0.9,
        frame_seq: seq,
        detected_at: chrono::Utc::now(),
    }
}

fn bench_acquire(c: &mut Criterion) {
    c.bench_function("tracker_acquire", |b| {
        b.iter_with_setup(
            || TargetTracker::new(2000, 60, 3, 0.3),
            |mut t| {
                let det = make_detection(0, 600, 350);
                black_box(t.acquire(&det));
                t
            },
        );
    });
}

fn bench_update_with_matching_detection(c: &mut Criterion) {
    // One target, one matching detection — the common case
    c.bench_function("tracker_update_match", |b| {
        b.iter_with_setup(
            || {
                let mut t = TargetTracker::new(2000, 60, 1, 0.3);
                t.acquire(&make_detection(0, 600, 350));
                t
            },
            |mut t| {
                let dets = vec![make_detection(1, 605, 352)];
                t.update(&dets);
                t
            },
        );
    });
}

fn bench_update_with_empty_detections(c: &mut Criterion) {
    // Target lost — prediction only
    c.bench_function("tracker_update_empty", |b| {
        b.iter_with_setup(
            || {
                let mut t = TargetTracker::new(2000, 60, 1, 0.3);
                t.acquire(&make_detection(0, 600, 350));
                t
            },
            |mut t| {
                t.update(&[]);
                t
            },
        );
    });
}

fn bench_update_with_many_detections(c: &mut Criterion) {
    // 20 detections, only one matches — worst case for IoU matching
    c.bench_function("tracker_update_20_dets", |b| {
        b.iter_with_setup(
            || {
                let mut t = TargetTracker::new(2000, 60, 1, 0.3);
                t.acquire(&make_detection(0, 600, 350));
                t
            },
            |mut t| {
                let dets: Vec<Detection> = (0..20u32)
                    .map(|i| make_detection(i as u64, 100 + i * 50, 100 + i * 30))
                    .collect();
                t.update(&dets);
                t
            },
        );
    });
}

criterion_group!(
    benches,
    bench_acquire,
    bench_update_with_matching_detection,
    bench_update_with_empty_detections,
    bench_update_with_many_detections
);
criterion_main!(benches);
