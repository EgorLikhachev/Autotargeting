//! End-to-end integration tests for the auto-targeting pipeline.
//!
//! These tests exercise the full stack:
//!   SyntheticVideoSource → MockInferenceBackend → TargetTracker → Commander → MockFcAdapter
//!
//! No real hardware required — all components are mocked. The tests verify
//! that the modules compose correctly and that the commander issues the
//! expected FC commands in response to detections.

use auto_targeting_cli::integration_test_helpers;
use common::{BoundingBox, Detection, FlightMode, SystemState};
use std::time::Duration;
use target_tracker::TargetTracker;
use video_capture::{SyntheticPattern, SyntheticVideoSource};

/// Test: a single detection → tracker acquires lock → commander transitions to TRACKING.
#[tokio::test]
async fn test_single_detection_acquires_lock_and_transitions_to_tracking() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;

    // Simulate one detection at frame center
    let det = Detection {
        bbox: BoundingBox {
            x: 600,
            y: 350,
            width: 80,
            height: 120,
        },
        class: "person".to_string(),
        class_id: 0,
        confidence: 0.9,
        frame_seq: 0,
        detected_at: chrono::Utc::now(),
    };

    // Operator selects target
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(1).unwrap();

    // Feed detection to tracker
    let acquired_id = harness.tracker.acquire(&det);
    harness.tracker.update(std::slice::from_ref(&det));

    // Verify tracker has the target
    let target = harness
        .tracker
        .active_target()
        .expect("target should be active");
    // id — глобальный счётчик (параллельные тесты инкрементят): сравниваем
    // с захваченным значением, не с абсолютом (фикс флaky-изоляции).
    assert_eq!(target.id, acquired_id);

    // Commander should be in TRACKING state
    assert_eq!(harness.commander.state(), SystemState::Tracking);
}

/// Test: target offset → commander sends correction to FC.
#[tokio::test]
async fn test_target_offset_triggers_fc_correction() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;

    // Select target
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(1).unwrap();

    // Initial command count
    let initial_commands = harness.fc.recorded_commands().len();

    // Update with offset (target is off-center)
    harness
        .commander
        .update(&[], Some((0.3, 0.0)))
        .await
        .unwrap();

    // FC should have received a command
    let final_commands = harness.fc.recorded_commands().len();
    assert!(
        final_commands > initial_commands,
        "FC should have received a correction command"
    );
}

/// Test: offset within deadband → no command sent.
#[tokio::test]
async fn test_deadband_suppresses_small_offsets() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(1).unwrap();

    let initial = harness.fc.recorded_commands().len();

    // Offset within deadband (default 0.05)
    harness
        .commander
        .update(&[], Some((0.02, 0.01)))
        .await
        .unwrap();

    let final_count = harness.fc.recorded_commands().len();
    assert_eq!(
        initial, final_count,
        "no command should be sent within deadband"
    );
}

/// Test: oscillation pattern → anti-loop guard triggers degrade.
#[tokio::test]
async fn test_oscillation_triggers_degrade() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(1).unwrap();

    // Send alternating large offsets to trigger oscillation
    for _ in 0..5 {
        harness
            .commander
            .update(&[], Some((0.5, 0.0)))
            .await
            .unwrap();
        harness
            .commander
            .update(&[], Some((-0.5, 0.0)))
            .await
            .unwrap();
    }

    // State should have transitioned to TRACKING_DEGRADED or ABORT
    let state = harness.commander.state();
    assert!(
        state == SystemState::TrackingDegraded || state == SystemState::Abort,
        "expected degrade or abort, got {state}"
    );
}

/// Test: abort → state transitions to ABORT + RTL command sent.
#[tokio::test]
async fn test_abort_sends_rtl() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(1).unwrap();

    let initial = harness.fc.recorded_commands().len();
    harness.commander.abort().await.unwrap();

    assert_eq!(harness.commander.state(), SystemState::Abort);

    let final_count = harness.fc.recorded_commands().len();
    assert!(final_count > initial, "RTL command should have been sent");

    // Verify a SetMode(Rtl) command was recorded
    let cmds = harness.fc.recorded_commands();
    let has_rtl = cmds.iter().any(|c| {
        matches!(
            c,
            fc_adapter::mock::RecordedCommand::SetMode(FlightMode::Rtl)
        )
    });
    assert!(has_rtl, "expected a SetMode(Rtl) command");
}

/// Test: synthetic video source produces frames with increasing sequence numbers.
#[tokio::test]
async fn test_synthetic_video_source_produces_frames() {
    use video_capture::{SyntheticConfig, VideoSource};

    let mut source = SyntheticVideoSource::new(SyntheticConfig {
        width: 320,
        height: 240,
        fps: 100,
        pattern: SyntheticPattern::Gradient,
        infinite: false,
        max_frames: 5,
    });

    let mut rx = source.start().await.unwrap();

    let mut last_seq = None;
    for _ in 0..5 {
        let frame = rx.recv().await.expect("frame");
        if let Some(last) = last_seq {
            assert_eq!(frame.metadata.seq, last + 1);
        }
        last_seq = Some(frame.metadata.seq);
    }
}

/// Test: full lifecycle — arm, scan, select, track, abort, reset.
#[tokio::test]
async fn test_full_lifecycle_end_to_end() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;

    // Initial state
    assert_eq!(harness.commander.state(), SystemState::Armed);

    // Start scanning
    harness.commander.start_scanning().unwrap();
    assert_eq!(harness.commander.state(), SystemState::Scanning);

    // Select target
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(42).unwrap();
    assert_eq!(harness.commander.state(), SystemState::Tracking);

    // Update with offset
    harness
        .commander
        .update(&[], Some((0.2, 0.1)))
        .await
        .unwrap();

    // Abort
    harness.commander.abort().await.unwrap();
    assert_eq!(harness.commander.state(), SystemState::Abort);

    // Reset
    harness.commander.reset().unwrap();
    assert_eq!(harness.commander.state(), SystemState::Idle);
}

/// Test: health snapshot is populated.
#[tokio::test]
async fn test_health_snapshot() {
    let harness = integration_test_helpers::PipelineHarness::new().await;
    let health = harness.commander.health_snapshot();

    assert_eq!(health.state, SystemState::Armed);
    assert!(health.connected);
    assert!(health.armed);
    assert!(!health.watchdogs.is_empty());
}

/// Test: tracker correctly identifies lost target after max missed frames.
#[tokio::test]
async fn test_tracker_declares_lost() {
    let mut tracker = TargetTracker::new(2000, 5, 1, 0.3);

    // Acquire target
    let det = Detection {
        bbox: BoundingBox {
            x: 100,
            y: 100,
            width: 50,
            height: 80,
        },
        class: "person".to_string(),
        class_id: 0,
        confidence: 0.9,
        frame_seq: 0,
        detected_at: chrono::Utc::now(),
    };
    tracker.acquire(&det);
    assert!(!tracker.is_lost());

    // Simulate missed frames
    for _ in 0..6 {
        tracker.update(&[]);
    }

    assert!(tracker.is_lost(), "tracker should declare target lost");
}

/// Test: rate limiter drops excessive commands.
#[tokio::test]
async fn test_rate_limiter_drops_excess() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(1).unwrap();

    // Send many updates rapidly — rate limiter (10 Hz) should drop most
    for _ in 0..20 {
        harness
            .commander
            .update(&[], Some((0.3, 0.0)))
            .await
            .unwrap();
    }

    let health = harness.commander.health_snapshot();
    // Should have sent ~1-2 commands (the loop runs fast, but rate limiter
    // allows only 1 per 100ms)
    assert!(
        health.rate_limiter_sent < 20,
        "rate limiter should have dropped most commands, sent={}",
        health.rate_limiter_sent
    );
    assert!(health.rate_limiter_dropped > 0, "should have dropped some");
}

/// Test: watchdog expiration triggers degrade.
#[tokio::test]
async fn test_watchdog_expiration_triggers_degrade() {
    let mut harness = integration_test_helpers::PipelineHarness::new().await;
    harness.commander.start_scanning().unwrap();
    harness.commander.select_target(1).unwrap();
    assert_eq!(harness.commander.state(), SystemState::Tracking);

    // Don't feed watchdogs — wait for expiration
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Process expiries — video loop should have expired (100ms timeout)
    let expired = harness.commander.process_watchdog_expiries();
    assert!(!expired.is_empty(), "should have expired watchdogs");

    // Should have transitioned to TRACKING_DEGRADED
    assert_eq!(
        harness.commander.state(),
        SystemState::TrackingDegraded,
        "watchdog expiry should degrade tracking"
    );
}
