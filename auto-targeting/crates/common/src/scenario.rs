//! SITL test scenario parser and runner.
//!
//! Reads JSON scenario files from `sim/scenarios/` and provides a structured
//! representation for the test runner (Phase 6 will implement the actual
//! execution loop).

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error("failed to read scenario file: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to parse scenario JSON: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("invalid scenario: {0}")]
    Invalid(String),
}

pub type ScenarioResult<T> = std::result::Result<T, ScenarioError>;

/// A complete test scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub description: String,
    pub video: VideoSpec,
    pub detections: DetectionSpec,
    #[serde(default)]
    pub operator_actions: Vec<OperatorAction>,
    #[serde(default)]
    pub expected_events: Vec<ExpectedEvent>,
    pub expected_final_state: String,
    #[serde(default)]
    pub kpi_checks: KpiChecks,
}

/// Video source specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub pattern: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration_frames: u64,
    #[serde(default)]
    pub dot_speed_px_per_frame: Option<u32>,
    #[serde(default)]
    pub targets: Vec<TargetSpec>,
}

/// Target specification (for multi-target scenarios).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSpec {
    pub id: u64,
    pub start_x: u32,
    pub start_y: u32,
    pub speed_x: i32,
    pub speed_y: i32,
}

/// Detection generator specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub generator: String,
    pub class: String,
    pub class_id: u32,
    pub confidence: f32,
    pub bbox_width: u32,
    pub bbox_height: u32,
    #[serde(default)]
    pub occlusions: Vec<OcclusionSpec>,
    #[serde(default)]
    pub motion_pattern: Option<MotionPatternSpec>,
}

/// Occlusion specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcclusionSpec {
    pub start_frame: u64,
    pub end_frame: u64,
    #[serde(default)]
    pub note: String,
}

/// Motion pattern specification (for oscillating targets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionPatternSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub amplitude_px: u32,
    pub frequency_hz: f32,
    pub jitter_px: u32,
    pub direction_changes_per_second: u32,
}

/// Operator action at a specific frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorAction {
    pub at_frame: u64,
    pub action: String,
    #[serde(default)]
    pub target_id: Option<u64>,
    #[serde(default)]
    pub note: String,
}

/// Expected event at a specific frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedEvent {
    /// Frame number, or a range string like "50-150".
    pub at_frame: FrameRef,
    pub event: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub note: String,
}

/// Either a single frame number or a range.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FrameRef {
    Single(u64),
    Range(String),
}

impl FrameRef {
    /// Returns true if the given frame matches this reference.
    pub fn matches(&self, frame: u64) -> bool {
        match self {
            Self::Single(f) => *f == frame,
            Self::Range(s) => {
                // Parse "start-end" format
                if let Some((start_str, end_str)) = s.split_once('-') {
                    if let (Ok(start), Ok(end)) = (start_str.parse::<u64>(), end_str.parse::<u64>())
                    {
                        return frame >= start && frame <= end;
                    }
                }
                false
            }
        }
    }
}

/// KPI checks for the scenario.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KpiChecks {
    #[serde(default)]
    pub lock_acquisition_time_ms: Option<u64>,
    #[serde(default)]
    pub tracking_accuracy_percent: Option<f32>,
    #[serde(default)]
    pub recovery_time_ms: Option<u64>,
    #[serde(default)]
    pub yaw_correction_rate_hz: Option<u32>,
    #[serde(default)]
    pub watchdog_triggers: Option<u32>,
    #[serde(default)]
    pub lost_state_transitions: Option<u32>,
    #[serde(default)]
    pub wrong_target_lock_count: Option<u32>,
    #[serde(default)]
    pub oscillation_escalation_to_abort: Option<bool>,
    #[serde(default)]
    pub max_consecutive_oscillations: Option<u32>,
}

impl Scenario {
    /// Load a scenario from a JSON file.
    pub fn load(path: impl AsRef<Path>) -> ScenarioResult<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        let scenario: Scenario = serde_json::from_str(&content)?;
        scenario.validate()?;
        Ok(scenario)
    }

    /// Validate the scenario for internal consistency.
    fn validate(&self) -> ScenarioResult<()> {
        if self.name.is_empty() {
            return Err(ScenarioError::Invalid("name is empty".to_string()));
        }
        if self.video.width == 0 || self.video.height == 0 {
            return Err(ScenarioError::Invalid(
                "video dimensions must be > 0".to_string(),
            ));
        }
        if self.video.fps == 0 {
            return Err(ScenarioError::Invalid("video fps must be > 0".to_string()));
        }
        if self.video.duration_frames == 0 {
            return Err(ScenarioError::Invalid(
                "duration_frames must be > 0".to_string(),
            ));
        }
        if !["synthetic", "replay"].contains(&self.video.kind.as_str()) {
            return Err(ScenarioError::Invalid(format!(
                "unknown video type: {} (expected 'synthetic' or 'replay')",
                self.video.kind
            )));
        }
        Ok(())
    }

    /// List all scenarios in a directory.
    pub fn list_dir(dir: impl AsRef<Path>) -> Vec<String> {
        let dir = dir.as_ref();
        let mut scenarios = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("scenario_") && name.ends_with(".json") {
                        scenarios.push(name.to_string());
                    }
                }
            }
        }
        scenarios.sort();
        scenarios
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scenario_json() -> &'static str {
        r#"{
            "name": "test_scenario",
            "description": "A test scenario",
            "video": {
                "type": "synthetic",
                "pattern": "moving_dot",
                "width": 1280,
                "height": 720,
                "fps": 30,
                "duration_frames": 300
            },
            "detections": {
                "type": "scripted",
                "generator": "from_video",
                "class": "person",
                "class_id": 0,
                "confidence": 0.9,
                "bbox_width": 60,
                "bbox_height": 120
            },
            "operator_actions": [
                {"at_frame": 10, "action": "select_target", "target_id": 1}
            ],
            "expected_events": [
                {"at_frame": 13, "event": "state_transition", "to": "TRACKING"}
            ],
            "expected_final_state": "TRACKING",
            "kpi_checks": {
                "lock_acquisition_time_ms": 1000
            }
        }"#
    }

    #[test]
    fn parse_valid_scenario() {
        let scenario: Scenario = serde_json::from_str(sample_scenario_json()).unwrap();
        assert_eq!(scenario.name, "test_scenario");
        assert_eq!(scenario.video.width, 1280);
        assert_eq!(scenario.video.fps, 30);
        assert_eq!(scenario.operator_actions.len(), 1);
        assert_eq!(scenario.expected_events.len(), 1);
    }

    #[test]
    fn load_from_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), sample_scenario_json()).unwrap();
        let scenario = Scenario::load(tmp.path()).unwrap();
        assert_eq!(scenario.name, "test_scenario");
    }

    #[test]
    fn validate_rejects_empty_name() {
        let mut scenario: Scenario = serde_json::from_str(sample_scenario_json()).unwrap();
        scenario.name = "".to_string();
        let result = scenario.validate();
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_zero_dimensions() {
        let mut scenario: Scenario = serde_json::from_str(sample_scenario_json()).unwrap();
        scenario.video.width = 0;
        let result = scenario.validate();
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_unknown_video_type() {
        let mut scenario: Scenario = serde_json::from_str(sample_scenario_json()).unwrap();
        scenario.video.kind = "magic".to_string();
        let result = scenario.validate();
        assert!(result.is_err());
    }

    #[test]
    fn frame_ref_single_matches_exact() {
        let fr = FrameRef::Single(42);
        assert!(fr.matches(42));
        assert!(!fr.matches(41));
        assert!(!fr.matches(43));
    }

    #[test]
    fn frame_ref_range_matches_inclusive() {
        let fr = FrameRef::Range("50-100".to_string());
        assert!(fr.matches(50));
        assert!(fr.matches(75));
        assert!(fr.matches(100));
        assert!(!fr.matches(49));
        assert!(!fr.matches(101));
    }

    #[test]
    fn frame_ref_range_invalid_format() {
        let fr = FrameRef::Range("not-a-range".to_string());
        assert!(!fr.matches(50));
    }

    #[test]
    fn list_dir_returns_scenario_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("scenario_a.json"), sample_scenario_json()).unwrap();
        std::fs::write(tmp.path().join("scenario_b.json"), sample_scenario_json()).unwrap();
        std::fs::write(tmp.path().join("not_a_scenario.txt"), "ignore me").unwrap();

        let scenarios = Scenario::list_dir(tmp.path());
        assert_eq!(scenarios.len(), 2);
        assert!(scenarios.contains(&"scenario_a.json".to_string()));
        assert!(scenarios.contains(&"scenario_b.json".to_string()));
    }

    #[test]
    fn kpi_checks_default_to_none() {
        let scenario: Scenario = serde_json::from_str(sample_scenario_json()).unwrap();
        assert_eq!(scenario.kpi_checks.lock_acquisition_time_ms, Some(1000));
        assert_eq!(scenario.kpi_checks.tracking_accuracy_percent, None);
    }

    #[test]
    fn occlusions_default_to_empty() {
        let scenario: Scenario = serde_json::from_str(sample_scenario_json()).unwrap();
        assert!(scenario.detections.occlusions.is_empty());
    }

    #[test]
    fn parse_scenario_with_occlusions() {
        let json = r#"{
            "name": "occlusion_test",
            "description": "test",
            "video": {"type": "synthetic", "pattern": "moving_dot", "width": 640, "height": 480, "fps": 30, "duration_frames": 300},
            "detections": {"type": "scripted", "generator": "from_video", "class": "person", "class_id": 0, "confidence": 0.9, "bbox_width": 50, "bbox_height": 100, "occlusions": [{"start_frame": 100, "end_frame": 130, "note": "1s gap"}]},
            "expected_final_state": "TRACKING"
        }"#;
        let scenario: Scenario = serde_json::from_str(json).unwrap();
        assert_eq!(scenario.detections.occlusions.len(), 1);
        assert_eq!(scenario.detections.occlusions[0].start_frame, 100);
        assert_eq!(scenario.detections.occlusions[0].end_frame, 130);
    }
}
