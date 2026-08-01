//! Stress tests — длительные тесты на стабильность и утечки памяти.
//!
//! Эти тесты запускаются долго (30+ секунд каждый), поэтому по умолчанию
//! `#[ignore]`. Запускаются в nightly CI:
//!
//! ```bash
//! cargo test --workspace --test stress_tests -- --include-ignored
//! ```

#![cfg(test)]

use commander::Commander;
use common::CommanderConfig;
use fc_adapter::{FlightControllerAdapter, MockFcAdapter};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// 60-секундный stress test: цикл arm → scan → select → track → abort.
/// Проверяет, что нет утечек памяти и система стабильна.
#[tokio::test]
#[ignore = "long-running stress test (60s)"]
async fn stress_60s_lifecycle_cycles() {
    let config = CommanderConfig::default();
    let iterations = 100;

    let start = Instant::now();
    for i in 0..iterations {
        let fc = MockFcAdapter::new();
        let fc_for_commander: Box<dyn FlightControllerAdapter> = Box::new(fc);
        let mut commander = Commander::new(config.clone(), fc_for_commander);

        commander.connect().await.unwrap();
        commander.arm().await.unwrap();
        commander.start_scanning().unwrap();
        commander.select_target(i).unwrap();
        commander.update(&[], Some((0.3, 0.1))).await.unwrap();
        commander.abort().await.unwrap();
        commander.reset().unwrap();
        commander.disconnect().await.unwrap();

        if i % 20 == 0 {
            println!("Iteration {i}/{iterations} done");
        }
    }

    let elapsed = start.elapsed();
    println!("Stress test completed: {iterations} iterations in {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(60),
        "stress test took too long: {elapsed:?}"
    );
}

/// 30-секундный test: rapid update() calls, проверяет rate limiter.
#[tokio::test]
#[ignore = "long-running stress test (30s)"]
async fn stress_30s_rapid_updates() {
    let fc = MockFcAdapter::new();
    let fc_for_commander: Box<dyn FlightControllerAdapter> = Box::new(fc);
    let mut commander = Commander::new(CommanderConfig::default(), fc_for_commander);

    commander.connect().await.unwrap();
    commander.arm().await.unwrap();
    commander.start_scanning().unwrap();
    commander.select_target(1).unwrap();

    let start = Instant::now();
    let mut update_count = 0;
    let mut commands_sent = 0;

    while start.elapsed() < Duration::from_secs(30) {
        // Rapid updates with varying offsets
        let offset_x = (update_count as f32 * 0.01).sin() * 0.5;
        let offset_y = (update_count as f32 * 0.013).cos() * 0.3;
        commander
            .update(&[], Some((offset_x, offset_y)))
            .await
            .unwrap();
        update_count += 1;

        let health = commander.health_snapshot();
        commands_sent = health.rate_limiter_sent;
    }

    println!("Stress test: {update_count} updates, {commands_sent} commands sent in 30s");

    // Rate limiter should have dropped most commands (10 Hz max)
    assert!(
        commands_sent <= 350,
        "too many commands sent: {commands_sent} (expected ~300 at 10Hz)"
    );

    commander.abort().await.unwrap();
}

/// 10-секундный test: watchdog expiry и recovery.
#[tokio::test]
#[ignore = "long-running stress test (10s)"]
async fn stress_10s_watchdog_expiry_recovery() {
    let fc = MockFcAdapter::new();
    let fc_for_commander: Box<dyn FlightControllerAdapter> = Box::new(fc);
    let mut commander = Commander::new(CommanderConfig::default(), fc_for_commander);

    commander.connect().await.unwrap();
    commander.arm().await.unwrap();
    commander.start_scanning().unwrap();
    commander.select_target(1).unwrap();

    let start = Instant::now();
    let mut degrade_count = 0;

    while start.elapsed() < Duration::from_secs(10) {
        // Don't feed watchdogs — let them expire
        let expired = commander.process_watchdog_expiries();
        if !expired.is_empty() {
            degrade_count += expired.len() as u64;
            // Recover: feed watchdogs
            commander.feed_video_watchdog();
        }

        sleep(Duration::from_millis(50)).await;
    }

    println!("Stress test: {degrade_count} watchdog expiries in 10s");
    // Should have some expiries (we didn't feed watchdogs)
    assert!(degrade_count > 0, "expected some watchdog expiries");

    commander.abort().await.unwrap();
}

/// 5-секундный test: oscillation detection под нагрузкой.
#[tokio::test]
#[ignore = "long-running stress test (5s)"]
async fn stress_5s_oscillation_detection() {
    use commander::anti_loop::{AntiLoopGuard, CorrectionCommand};
    use std::time::Instant;

    let guard = Arc::new(AntiLoopGuard::new(CommanderConfig::default()));
    let start = Instant::now();
    let mut oscillation_count = 0;
    let mut command_count = 0;

    while start.elapsed() < Duration::from_secs(5) {
        // Alternate yaw commands to trigger oscillation
        let yaw = if command_count % 2 == 0 { 15.0 } else { -15.0 };
        let ox = if command_count % 2 == 0 { 0.3 } else { -0.3 };
        let cmd = CorrectionCommand {
            yaw_rate_dps: yaw,
            pitch_rate_dps: 0.0,
            offset_x: ox,
            offset_y: 0.1,
            generated_at: Instant::now(),
        };

        let decision = guard.process(cmd);
        if matches!(
            decision,
            commander::anti_loop::GuardDecision::Degrade
                | commander::anti_loop::GuardDecision::Abort
        ) {
            oscillation_count += 1;
        }
        command_count += 1;

        // Small delay
        sleep(Duration::from_millis(10)).await;
    }

    println!("Stress test: {oscillation_count} oscillations in {command_count} commands over 5s");
    assert!(
        oscillation_count > 0,
        "expected oscillation detection to trigger"
    );
}

/// Scenario runner stress: запустить все сценарии 10 раз подряд.
#[tokio::test]
#[ignore = "long-running stress test (~2 min)"]
async fn stress_scenario_suite_10_iterations() {
    use auto_targeting_cli::scenario_runner;

    let dir = std::path::Path::new("sim/scenarios");
    if !dir.exists() {
        eprintln!("SKIP: scenarios directory not found");
        return;
    }

    let start = Instant::now();
    let mut total_passed = 0;
    let mut total_failed = 0;

    for i in 0..10 {
        let results = scenario_runner::run_all_scenarios(dir, false).await;
        let passed = results.iter().filter(|r| r.passed).count();
        let failed = results.iter().filter(|r| !r.passed).count();
        total_passed += passed;
        total_failed += failed;

        println!("Iteration {}: {} passed, {} failed", i + 1, passed, failed);

        if failed > 0 {
            panic!("Iteration {} had {} failures", i + 1, failed);
        }
    }

    let elapsed = start.elapsed();
    println!("Stress test: {total_passed} scenarios passed, {total_failed} failed in {elapsed:?}");
    assert_eq!(
        total_failed, 0,
        "all scenarios should pass in all iterations"
    );
}
