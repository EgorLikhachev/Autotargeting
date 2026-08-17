//! Criterion-бенчмарки yolov8-горячих путей (перф-аудит 2026-08).
//!
//! `cargo bench -p yolov8 --bench postprocess`

use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Реалистичный выход YOLOv8n: [84 x 8400], большинство якорей ниже порога,
/// часть — уверенные детекции с «правдоподобным» шумом.
fn synthetic_output(seed: u64) -> Vec<f32> {
    let num_anchors = 8400usize;
    let rows = 4 + 80usize;
    let mut out = vec![0f32; rows * num_anchors];
    // Детерминированный LCG.
    let mut s = seed;
    let mut rnd = || -> f32 {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((s >> 33) as f32) / ((u64::MAX >> 33) as f32)
    };
    for a in 0..num_anchors {
        out[a] = 32.0 + rnd() * 576.0; // cx
        out[num_anchors + a] = 32.0 + rnd() * 576.0; // cy
        out[2 * num_anchors + a] = 8.0 + rnd() * 120.0; // w
        out[3 * num_anchors + a] = 8.0 + rnd() * 120.0; // h
        // Классы: низкий фон + редкие пики выше 0.35.
        for c in 0..80usize {
            let v = if (a + c) % 997 == 0 {
                0.55 + rnd() * 0.4
            } else {
                rnd() * 0.28
            };
            out[(4 + c) * num_anchors + a] = v;
        }
    }
    out
}

fn bench_yolov8(c: &mut Criterion) {
    let output = synthetic_output(42);

    c.bench_function("postprocess_8400_conf035", |b| {
        b.iter(|| {
            black_box(yolov8::postprocess(
                black_box(&output),
                black_box(0.35),
                black_box(0.45),
                black_box(1.0),
            ))
        })
    });

    // letterbox 640x480 -> 640x640.
    let rgb = vec![77u8; 640 * 480 * 3];
    c.bench_function("letterbox_640x480", |b| {
        b.iter(|| black_box(yolov8::letterbox(black_box(&rgb), black_box(640), black_box(480))))
    });

    let lb = vec![77u8; 640 * 640 * 3];
    c.bench_function("rgb_to_nchw_f32_640", |b| {
        b.iter(|| black_box(yolov8::rgb_to_nchw_f32(black_box(&lb))))
    });
}

criterion_group!(benches, bench_yolov8);
criterion_main!(benches);
