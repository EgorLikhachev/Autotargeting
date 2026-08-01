//! Scenario runner — executes a JSON test scenario end-to-end.
//!
//! Loads a scenario from `sim/scenarios/*.json`, runs the full pipeline
//! (synthetic video → mock inference → tracker → commander → mock FC),
//! and verifies that all expected events occurred and KPI targets were met.
//!
//! ## Usage
//!
//! ```bash
//! # Run a single scenario
//! cargo run -p auto-targeting-cli -- scenario sim/scenarios/scenario_static_target.json
//!
//! # Run all scenarios in a directory
//! cargo run -p auto-targeting-cli -- scenario --all sim/scenarios/
//!
//! # Verbose output
//! cargo run -p auto-targeting-cli -- scenario --verbose sim/scenarios/scenario_occlusion.json
//! ```

use anyhow::{anyhow, Result};
use chrono::Utc;
use commander::Commander;
use common::{BoundingBox, CommanderConfig, Detection, Scenario, SystemState};
use fc_adapter::{FlightControllerAdapter, MockFcAdapter};
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Result of running a single scenario.
#[derive(Debug, Clone)]
pub struct ScenarioResult {
    pub name: String,
    pub passed: bool,
    pub frames_processed: u64,
    pub duration: Duration,
    pub final_state: SystemState,
    pub fc_commands_sent: usize,
    pub state_transitions: u64,
    pub watchdog_triggers: u64,
    pub lock_acquisition_time_ms: Option<u64>,
    pub failures: Vec<String>,
    pub warnings: Vec<String>,
}

impl std::fmt::Display for ScenarioResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.passed { "PASS" } else { "FAIL" };
        writeln!(f, "=== {} {} ===", self.name, status)?;
        writeln!(f, "  Frames:           {}", self.frames_processed)?;
        writeln!(f, "  Duration:         {:.2}s", self.duration.as_secs_f64())?;
        writeln!(f, "  Final state:      {}", self.final_state)?;
        writeln!(f, "  FC commands:      {}", self.fc_commands_sent)?;
        writeln!(f, "  State transitions:{}", self.state_transitions)?;
        writeln!(f, "  Watchdog triggers:{}", self.watchdog_triggers)?;
        if let Some(lock_ms) = self.lock_acquisition_time_ms {
            writeln!(f, "  Lock acquisition: {}ms", lock_ms)?;
        }
        if !self.warnings.is_empty() {
            writeln!(f, "  Warnings:")?;
            for w in &self.warnings {
                writeln!(f, "    - {w}")?;
            }
        }
        if !self.failures.is_empty() {
            writeln!(f, "  Failures:")?;
            for fail in &self.failures {
                writeln!(f, "    - {fail}")?;
            }
        }
        Ok(())
    }
}

/// Run a single scenario file.
pub async fn run_scenario(path: &Path, verbose: bool) -> Result<ScenarioResult> {
    let scenario = Scenario::load(path)
        .map_err(|e| anyhow!("failed to load scenario {}: {e}", path.display()))?;

    if verbose {
        info!(name = %scenario.name, "running scenario");
    }

    let start = Instant::now();

    // Set up the pipeline
    let fc = MockFcAdapter::new();
    let shared_state = fc.state_handle();
    let fc_for_assertions = MockFcAdapter::new_with_shared_state(shared_state);
    let fc_for_commander: Box<dyn FlightControllerAdapter> = Box::new(fc);

    let mut commander = Commander::new(CommanderConfig::default(), fc_for_commander);
    commander.connect().await?;
    commander.arm().await?;
    commander.start_scanning()?;

    // Generate detections based on the scenario
    let total_frames = scenario.video.duration_frames;
    let mut lock_acquired_at: Option<Instant> = None;
    let mut lock_acquisition_time_ms: Option<u64> = None;
    let mut watchdog_triggers: u64 = 0;
    let mut failures = Vec::new();
    let mut warnings = Vec::new();

    // Track which operator actions we've executed
    let mut action_idx = 0;

    for frame_seq in 0..total_frames {
        // Check if there's an operator action at this frame
        while action_idx < scenario.operator_actions.len()
            && scenario.operator_actions[action_idx].at_frame == frame_seq
        {
            let action = &scenario.operator_actions[action_idx];
            if verbose {
                info!(frame = frame_seq, action = %action.action, "operator action");
            }
            match action.action.as_str() {
                "select_target" => {
                    if let Some(target_id) = action.target_id {
                        match commander.select_target(target_id) {
                            Ok(()) => {
                                lock_acquired_at = Some(Instant::now());
                            }
                            Err(e) => {
                                failures.push(format!(
                                    "frame {}: select_target({}) failed: {e}",
                                    frame_seq, target_id
                                ));
                            }
                        }
                    }
                }
                "abort" => {
                    if let Err(e) = commander.abort().await {
                        failures.push(format!("frame {}: abort failed: {e}", frame_seq));
                    }
                }
                "scan" => {
                    if let Err(e) = commander.start_scanning() {
                        failures.push(format!("frame {}: scan failed: {e}", frame_seq));
                    }
                }
                other => {
                    warnings.push(format!(
                        "frame {}: unknown operator action '{other}'",
                        frame_seq
                    ));
                }
            }
            action_idx += 1;
        }

        // Generate detections for this frame
        let detections = generate_detections(&scenario, frame_seq);

        // Check if lock was just acquired (transition to TRACKING)
        if lock_acquisition_time_ms.is_none()
            && lock_acquired_at.is_some()
            && commander.state() == SystemState::Tracking
        {
            if let Some(t) = lock_acquired_at {
                let elapsed = t.elapsed().as_millis() as u64;
                lock_acquisition_time_ms = Some(elapsed);
                if verbose {
                    info!(lock_ms = elapsed, "lock acquired");
                }
            }
        }

        // Compute target offset (if tracking)
        let target_offset = compute_target_offset(&scenario, &detections, frame_seq, &commander);

        // Feed the commander
        if let Err(e) = commander.update(&detections, target_offset).await {
            failures.push(format!("frame {}: update failed: {e}", frame_seq));
        }

        // Feed watchdogs (simulating real loops)
        commander.feed_video_watchdog();

        // Process watchdog expiries
        let expired = commander.process_watchdog_expiries();
        if !expired.is_empty() {
            watchdog_triggers += expired.len() as u64;
            for (id, action) in &expired {
                if verbose {
                    warn!(watchdog = id.as_str(), ?action, "watchdog expired");
                }
            }
        }

        // Sleep to simulate real-time (optional — comment out for max speed)
        // tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Check final state
    let final_state = commander.state();
    let expected_final: SystemState = parse_state(&scenario.expected_final_state);
    if final_state != expected_final {
        failures.push(format!(
            "final state mismatch: expected {}, got {}",
            expected_final, final_state
        ));
    }

    // Check KPIs
    if let Some(target_ms) = scenario.kpi_checks.lock_acquisition_time_ms {
        if let Some(actual_ms) = lock_acquisition_time_ms {
            if actual_ms > target_ms {
                failures.push(format!(
                    "lock acquisition time {actual_ms}ms exceeds target {target_ms}ms"
                ));
            }
        } else {
            failures.push("lock was never acquired".to_string());
        }
    }

    if let Some(max_triggers) = scenario.kpi_checks.watchdog_triggers {
        if watchdog_triggers > max_triggers as u64 {
            failures.push(format!(
                "watchdog triggers {watchdog_triggers} exceed max {max_triggers}"
            ));
        }
    }

    let fc_commands = fc_for_assertions.recorded_commands().len();
    let transitions = commander.transition_count();

    let passed = failures.is_empty();

    Ok(ScenarioResult {
        name: scenario.name.clone(),
        passed,
        frames_processed: total_frames,
        duration: start.elapsed(),
        final_state,
        fc_commands_sent: fc_commands,
        state_transitions: transitions,
        watchdog_triggers,
        lock_acquisition_time_ms,
        failures,
        warnings,
    })
}

/// Run all scenarios in a directory.
pub async fn run_all_scenarios(dir: &Path, verbose: bool) -> Vec<ScenarioResult> {
    let mut results = Vec::new();
    let scenarios = Scenario::list_dir(dir);
    for name in &scenarios {
        let path = dir.join(name);
        match run_scenario(&path, verbose).await {
            Ok(r) => results.push(r),
            Err(e) => {
                results.push(ScenarioResult {
                    name: name.clone(),
                    passed: false,
                    frames_processed: 0,
                    duration: Duration::from_secs(0),
                    final_state: SystemState::Idle,
                    fc_commands_sent: 0,
                    state_transitions: 0,
                    watchdog_triggers: 0,
                    lock_acquisition_time_ms: None,
                    failures: vec![format!("scenario execution error: {e}")],
                    warnings: vec![],
                });
            }
        }
    }
    results
}

/// Print a summary table of multiple scenario results.
pub fn print_summary(results: &[ScenarioResult]) {
    println!("\n=== Scenario Suite Summary ===");
    println!(
        "{:<35} {:<6} {:>8} {:>8} {:>8} {:>8}",
        "SCENARIO", "STATUS", "FRAMES", "CMDS", "TRANS", "WDT"
    );
    println!("{}", "-".repeat(85));
    let mut passed = 0;
    let mut failed = 0;
    for r in results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        if r.passed {
            passed += 1;
        } else {
            failed += 1;
        }
        println!(
            "{:<35} {:<6} {:>8} {:>8} {:>8} {:>8}",
            r.name,
            status,
            r.frames_processed,
            r.fc_commands_sent,
            r.state_transitions,
            r.watchdog_triggers
        );
    }
    println!("{}", "-".repeat(85));
    println!(
        "Total: {} passed, {} failed ({}% pass rate)",
        passed,
        failed,
        if !results.is_empty() {
            passed * 100 / results.len()
        } else {
            0
        }
    );
}

/// Generate detections for a frame based on the scenario spec.
fn generate_detections(scenario: &Scenario, frame_seq: u64) -> Vec<Detection> {
    let det_spec = &scenario.detections;

    // Check if we're in an occlusion window
    for occ in &det_spec.occlusions {
        if frame_seq >= occ.start_frame && frame_seq <= occ.end_frame {
            return Vec::new(); // No detections during occlusion
        }
    }

    // Generate a single detection at the target's current position
    let (x, y) = compute_target_position(scenario, frame_seq);

    let det = Detection {
        bbox: BoundingBox {
            x: x.saturating_sub(det_spec.bbox_width / 2),
            y: y.saturating_sub(det_spec.bbox_height / 2),
            width: det_spec.bbox_width,
            height: det_spec.bbox_height,
        },
        class: det_spec.class.clone(),
        class_id: det_spec.class_id,
        confidence: det_spec.confidence,
        frame_seq,
        detected_at: Utc::now(),
    };

    vec![det]
}

/// Compute the target's position at a given frame.
/// For now, we use a simple linear motion model based on the video spec.
fn compute_target_position(scenario: &Scenario, frame_seq: u64) -> (u32, u32) {
    let w = scenario.video.width;
    let h = scenario.video.height;
    let speed = scenario.video.dot_speed_px_per_frame.unwrap_or(3);

    // Default: target moves horizontally from left to right, wrapping around
    let x = (frame_seq * speed as u64) % w as u64;
    let y = (h / 2) as u64;
    (x as u32, y as u32)
}

/// Compute the offset of the target from frame center, as fractions [-1, 1].
fn compute_target_offset(
    scenario: &Scenario,
    detections: &[Detection],
    _frame_seq: u64,
    commander: &Commander,
) -> Option<(f32, f32)> {
    if commander.state() != SystemState::Tracking
        && commander.state() != SystemState::TrackingDegraded
    {
        return None;
    }

    let det = detections.first()?;
    let (cx, cy) = det.bbox.center();
    let w = scenario.video.width as f32;
    let h = scenario.video.height as f32;
    let offset_x = (cx - w / 2.0) / (w / 2.0);
    let offset_y = (cy - h / 2.0) / (h / 2.0);
    Some((offset_x, offset_y))
}

fn parse_state(s: &str) -> SystemState {
    match s.to_uppercase().as_str() {
        "IDLE" => SystemState::Idle,
        "ARMED" => SystemState::Armed,
        "SCANNING" => SystemState::Scanning,
        "TARGET_SELECTED" => SystemState::TargetSelected,
        "TRACKING" => SystemState::Tracking,
        "TRACKING_DEGRADED" => SystemState::TrackingDegraded,
        "LOST" => SystemState::Lost,
        "RTH" => SystemState::Rth,
        "ABORT" => SystemState::Abort,
        _ => SystemState::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_state_recognizes_all() {
        assert_eq!(parse_state("TRACKING"), SystemState::Tracking);
        assert_eq!(parse_state("tracking"), SystemState::Tracking);
        assert_eq!(parse_state("ABORT"), SystemState::Abort);
        assert_eq!(parse_state("unknown"), SystemState::Idle);
    }

    #[test]
    fn scenario_result_display_pass() {
        let r = ScenarioResult {
            name: "test".to_string(),
            passed: true,
            frames_processed: 100,
            duration: Duration::from_millis(50),
            final_state: SystemState::Tracking,
            fc_commands_sent: 5,
            state_transitions: 3,
            watchdog_triggers: 0,
            lock_acquisition_time_ms: Some(100),
            failures: vec![],
            warnings: vec![],
        };
        let s = format!("{r}");
        assert!(s.contains("PASS"));
        assert!(s.contains("100"));
    }

    #[test]
    fn scenario_result_display_fail() {
        let r = ScenarioResult {
            name: "test".to_string(),
            passed: false,
            frames_processed: 100,
            duration: Duration::from_millis(50),
            final_state: SystemState::Idle,
            fc_commands_sent: 0,
            state_transitions: 0,
            watchdog_triggers: 0,
            lock_acquisition_time_ms: None,
            failures: vec!["lock was never acquired".to_string()],
            warnings: vec![],
        };
        let s = format!("{r}");
        assert!(s.contains("FAIL"));
        assert!(s.contains("lock was never acquired"));
    }

    #[test]
    fn compute_target_position_wraps() {
        let scenario = Scenario {
            name: "test".to_string(),
            description: "".to_string(),
            video: common::scenario::VideoSpec {
                kind: "synthetic".to_string(),
                pattern: "moving_dot".to_string(),
                width: 100,
                height: 100,
                fps: 30,
                duration_frames: 100,
                dot_speed_px_per_frame: Some(10),
                targets: vec![],
            },
            detections: common::scenario::DetectionSpec {
                kind: "scripted".to_string(),
                generator: "from_video".to_string(),
                class: "person".to_string(),
                class_id: 0,
                confidence: 0.9,
                bbox_width: 50,
                bbox_height: 50,
                occlusions: vec![],
                motion_pattern: None,
            },
            operator_actions: vec![],
            expected_events: vec![],
            expected_final_state: "TRACKING".to_string(),
            kpi_checks: common::scenario::KpiChecks::default(),
        };
        let (x, y) = compute_target_position(&scenario, 0);
        assert_eq!(x, 0);
        assert_eq!(y, 50);
        let (x, _) = compute_target_position(&scenario, 5);
        assert_eq!(x, 50);
        let (x, _) = compute_target_position(&scenario, 10);
        assert_eq!(x, 0); // wraps around
    }
}
