//! Criterion-бенчмарки конверсий пикселей (перф-аудит 2026-08).
//!
//! Запуск: `cargo bench -p video-capture --bench convert`
//! Результаты → target/criterion/. Цифры RK3588 — в docs/PERF_AUDIT_2026-08.md.

use common::{Frame, FrameMetadata, PixelFormat};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn frame(w: u32, h: u32, format: PixelFormat, seed: u8) -> Frame {
    let bpp = match format {
        PixelFormat::Yuyv => 2,
        PixelFormat::Rgb24 => 3,
        _ => 1,
    };
    let data = (0..w as usize * h as usize * bpp)
        .map(|i| (i as u8).wrapping_mul(seed ^ 0x9E))
        .collect();
    Frame {
        data,
        metadata: FrameMetadata {
            width: w,
            height: h,
            format,
            captured_at: chrono::Utc::now(),
            seq: 1,
        },
    }
}

fn bench_convert(c: &mut Criterion) {
    let yuyv640 = frame(640, 480, PixelFormat::Yuyv, 1);
    let rgb640 = frame(640, 480, PixelFormat::Rgb24, 2);

    c.bench_function("yuyv_to_rgb24_640x480", |b| {
        b.iter(|| black_box(video_capture::yuyv_to_rgb24(black_box(&yuyv640)).unwrap()))
    });
    c.bench_function("yuyv_to_nv12_640x480", |b| {
        b.iter(|| black_box(video_capture::yuyv_to_nv12(black_box(&yuyv640)).unwrap()))
    });
    c.bench_function("rgb24_to_nv12_640x480", |b| {
        b.iter(|| black_box(video_capture::rgb24_to_nv12(black_box(&rgb640)).unwrap()))
    });
}

criterion_group!(benches, bench_convert);
criterion_main!(benches);
