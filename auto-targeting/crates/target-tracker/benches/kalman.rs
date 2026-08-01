//! Benchmarks for the Kalman filter — the hot path of the target tracker.
//!
//! Run with: `cargo bench -p target-tracker`
//! Results land in `target/criterion/`.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use target_tracker::KalmanFilter2D;

fn bench_predict(c: &mut Criterion) {
    let mut kf = KalmanFilter2D::new(640.0, 360.0);
    c.bench_function("kalman_predict_100us", |b| {
        b.iter(|| {
            kf.predict(black_box(0.0001));
        });
    });

    c.bench_function("kalman_predict_33ms_30fps", |b| {
        b.iter(|| {
            kf.predict(black_box(1.0 / 30.0));
        });
    });
}

fn bench_update(c: &mut Criterion) {
    let mut kf = KalmanFilter2D::new(640.0, 360.0);
    c.bench_function("kalman_update", |b| {
        b.iter(|| {
            kf.update(black_box(645.0), black_box(362.0), black_box(1.0 / 30.0));
        });
    });
}

fn bench_predict_update_cycle(c: &mut Criterion) {
    let mut kf = KalmanFilter2D::new(640.0, 360.0);
    let dt = 1.0 / 30.0;
    c.bench_function("kalman_full_cycle", |b| {
        b.iter(|| {
            kf.predict(black_box(dt));
            kf.update(black_box(645.0), black_box(362.0), black_box(dt));
        });
    });
}

fn bench_long_sequence(c: &mut Criterion) {
    c.bench_function("kalman_100_frame_sequence", |b| {
        b.iter(|| {
            let mut kf = KalmanFilter2D::new(100.0, 100.0);
            let dt = 1.0 / 30.0;
            for i in 0..100u32 {
                kf.predict(black_box(dt));
                let x = 100.0 + 10.0 * i as f32;
                kf.update(black_box(x), black_box(100.0), black_box(dt));
            }
        });
    });
}

criterion_group!(
    benches,
    bench_predict,
    bench_update,
    bench_predict_update_cycle,
    bench_long_sequence
);
criterion_main!(benches);
