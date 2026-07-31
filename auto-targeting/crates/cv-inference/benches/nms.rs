//! Benchmarks for Non-Maximum Suppression (NMS).
//!
//! Run with: `cargo bench -p cv-inference`
//! Results land in `target/criterion/`.

use common::{BoundingBox, Detection};
use criterion::{criterion_group, criterion_main, Criterion};
use cv_inference::non_max_suppression;

fn make_detections(n: usize, overlap: bool) -> Vec<Detection> {
    (0..n)
        .map(|i| {
            let x = if overlap {
                100 + (i % 5) * 3
            } else {
                100 + i * 100
            };
            let y = if overlap {
                100 + (i % 5) * 3
            } else {
                100 + i * 100
            };
            Detection {
                bbox: BoundingBox {
                    x: x as u32,
                    y: y as u32,
                    width: 60,
                    height: 80,
                },
                class: "person".to_string(),
                class_id: 0,
                confidence: 1.0 - i as f32 * 0.01,
                frame_seq: 0,
                detected_at: chrono::Utc::now(),
            }
        })
        .collect()
}

fn bench_nms_few_disjoint(c: &mut Criterion) {
    // 5 disjoint detections — typical case, no overlap
    c.bench_function("nms_5_disjoint", |b| {
        b.iter_with_setup(
            || make_detections(5, false),
            |mut dets| {
                non_max_suppression(&mut dets, 0.45);
                dets
            },
        );
    });
}

fn bench_nms_few_overlapping(c: &mut Criterion) {
    // 5 overlapping detections — needs filtering
    c.bench_function("nms_5_overlapping", |b| {
        b.iter_with_setup(
            || make_detections(5, true),
            |mut dets| {
                non_max_suppression(&mut dets, 0.45);
                dets
            },
        );
    });
}

fn bench_nms_many_overlapping(c: &mut Criterion) {
    // 50 overlapping detections — stress test
    c.bench_function("nms_50_overlapping", |b| {
        b.iter_with_setup(
            || make_detections(50, true),
            |mut dets| {
                non_max_suppression(&mut dets, 0.45);
                dets
            },
        );
    });
}

fn bench_nms_many_disjoint(c: &mut Criterion) {
    // 100 disjoint detections — large frame with many targets
    c.bench_function("nms_100_disjoint", |b| {
        b.iter_with_setup(
            || make_detections(100, false),
            |mut dets| {
                non_max_suppression(&mut dets, 0.45);
                dets
            },
        );
    });
}

criterion_group!(
    benches,
    bench_nms_few_disjoint,
    bench_nms_few_overlapping,
    bench_nms_many_overlapping,
    bench_nms_many_disjoint
);
criterion_main!(benches);
