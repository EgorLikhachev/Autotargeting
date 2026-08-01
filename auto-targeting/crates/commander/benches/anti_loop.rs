//! Benchmarks for the anti-loop guard — the per-command hot path.
//!
//! Run with: `cargo bench -p commander --bench anti_loop`
//! Results land in `target/criterion/`.

use commander::{AntiLoopGuard, CorrectionCommand};
use common::CommanderConfig;
use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Instant;

fn make_cmd(yaw: f32, ox: f32, oy: f32) -> CorrectionCommand {
    CorrectionCommand {
        yaw_rate_dps: yaw,
        pitch_rate_dps: 0.0,
        offset_x: ox,
        offset_y: oy,
        generated_at: Instant::now(),
    }
}

fn bench_process_allow(c: &mut Criterion) {
    // Command outside deadband — typical "allow" path
    c.bench_function("anti_loop_process_allow", |b| {
        b.iter_with_setup(
            || AntiLoopGuard::new(CommanderConfig::default()),
            |guard| {
                guard.process(make_cmd(10.0, 0.2, 0.1));
                guard
            },
        );
    });
}

fn bench_process_suppress(c: &mut Criterion) {
    // Command within deadband — "suppress" path
    c.bench_function("anti_loop_process_suppress", |b| {
        b.iter_with_setup(
            || AntiLoopGuard::new(CommanderConfig::default()),
            |guard| {
                guard.process(make_cmd(5.0, 0.02, 0.01));
                guard
            },
        );
    });
}

fn bench_process_steady_stream(c: &mut Criterion) {
    // 100 commands in same direction — should not trigger oscillation
    c.bench_function("anti_loop_100_steady", |b| {
        b.iter_with_setup(
            || AntiLoopGuard::new(CommanderConfig::default()),
            |guard| {
                for _ in 0..100 {
                    guard.process(make_cmd(10.0, 0.2, 0.1));
                }
                guard
            },
        );
    });
}

fn bench_process_oscillating_stream(c: &mut Criterion) {
    // 100 commands alternating direction — triggers oscillation detector
    c.bench_function("anti_loop_100_oscillating", |b| {
        b.iter_with_setup(
            || AntiLoopGuard::new(CommanderConfig::default()),
            |guard| {
                for i in 0..100 {
                    let yaw = if i % 2 == 0 { 15.0 } else { -15.0 };
                    let ox = if i % 2 == 0 { 0.3 } else { -0.3 };
                    guard.process(make_cmd(yaw, ox, 0.1));
                }
                guard
            },
        );
    });
}

criterion_group!(
    benches,
    bench_process_allow,
    bench_process_suppress,
    bench_process_steady_stream,
    bench_process_oscillating_stream
);
criterion_main!(benches);
