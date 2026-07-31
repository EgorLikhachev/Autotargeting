//! Benchmarks for the watchdog registry — feed + check_expired hot paths.
//!
//! Run with: `cargo bench -p commander --bench watchdogs`
//! Results land in `target/criterion/`.

use commander::{WatchdogAction, WatchdogConfig, WatchdogId, WatchdogRegistry};
use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

fn make_registry() -> WatchdogRegistry {
    let reg = WatchdogRegistry::new();
    reg.register(
        WatchdogId::VideoLoop,
        WatchdogConfig::new(Duration::from_millis(100), WatchdogAction::Degrade),
    );
    reg.register(
        WatchdogId::InferenceLoop,
        WatchdogConfig::new(Duration::from_millis(200), WatchdogAction::Degrade),
    );
    reg.register(
        WatchdogId::TrackingLoop,
        WatchdogConfig::new(Duration::from_millis(50), WatchdogAction::Degrade),
    );
    reg.register(
        WatchdogId::CommandLoop,
        WatchdogConfig::new(Duration::from_millis(100), WatchdogAction::Abort),
    );
    reg.register(
        WatchdogId::FcHeartbeat,
        WatchdogConfig::new(Duration::from_millis(1000), WatchdogAction::Abort),
    );
    reg
}

fn bench_feed(c: &mut Criterion) {
    c.bench_function("watchdog_feed_single", |b| {
        b.iter_with_setup(make_registry, |reg| {
            reg.feed(WatchdogId::VideoLoop);
            reg
        });
    });
}

fn bench_feed_all(c: &mut Criterion) {
    // Feed all 5 watchdogs — simulates one full loop cycle
    c.bench_function("watchdog_feed_all_5", |b| {
        b.iter_with_setup(make_registry, |reg| {
            reg.feed(WatchdogId::VideoLoop);
            reg.feed(WatchdogId::InferenceLoop);
            reg.feed(WatchdogId::TrackingLoop);
            reg.feed(WatchdogId::CommandLoop);
            reg.feed(WatchdogId::FcHeartbeat);
            reg
        });
    });
}

fn bench_check_expired(c: &mut Criterion) {
    // Check all 5 watchdogs for expiry — runs at 10 Hz in production
    c.bench_function("watchdog_check_expired_5", |b| {
        b.iter_with_setup(make_registry, |reg| {
            let _ = reg.check_expired();
            reg
        });
    });
}

fn bench_snapshot(c: &mut Criterion) {
    // Snapshot for health reporting — runs at ~1 Hz
    c.bench_function("watchdog_snapshot_5", |b| {
        b.iter_with_setup(make_registry, |reg| {
            let _ = reg.snapshot();
            reg
        });
    });
}

criterion_group!(
    benches,
    bench_feed,
    bench_feed_all,
    bench_check_expired,
    bench_snapshot
);
criterion_main!(benches);
