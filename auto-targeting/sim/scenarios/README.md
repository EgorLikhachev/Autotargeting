# SITL Test Scenarios

This directory contains JSON-based test scenarios for regression testing
of the auto-targeting pipeline. Each scenario defines:

- A video source (synthetic pattern or replay file)
- A detection generator (scripted or from video)
- Operator actions (when to select target, abort, etc.)
- Expected events (state transitions, FC commands, watchdog triggers)
- KPI checks (lock acquisition time, tracking accuracy, etc.)

## Available scenarios

| Scenario | Description | Tests |
|---|---|---|
| `scenario_static_target.json` | Static target at frame center | Basic lock acquisition |
| `scenario_moving_target.json` | Target moving horizontally | Tracker prediction, yaw correction |
| `scenario_occlusion.json` | Target disappears for 1 s | Kalman prediction, recovery |
| `scenario_multiple_targets.json` | 3 targets, operator selects one | Target selection, identity preservation |
| `scenario_oscillation_test.json` | Erratic target motion | Anti-loop guard, oscillation detector |

## Format

```json
{
  "name": "scenario_name",
  "description": "Human-readable description",
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
    "confidence": 0.92,
    "bbox_width": 60,
    "bbox_height": 120
  },
  "operator_actions": [
    { "at_frame": 10, "action": "select_target", "target_id": 1 }
  ],
  "expected_events": [
    { "at_frame": 13, "event": "state_transition", "to": "TRACKING" }
  ],
  "expected_final_state": "TRACKING",
  "kpi_checks": {
    "lock_acquisition_time_ms": 1000,
    "tracking_accuracy_percent": 95
  }
}
```

## Running scenarios

Scenario runner (Phase 6 TODO):

```bash
# Run a single scenario
cargo run -p auto-targeting-cli -- scenario sim/scenarios/scenario_static_target.json

# Run all scenarios (regression suite)
cargo run -p auto-targeting-cli -- scenario --all
```

## Adding new scenarios

1. Copy `scenario_static_target.json` as a template.
2. Modify the video/detection/action/event fields.
3. Add the scenario to the regression suite in CI.
4. Verify it passes with `cargo run -- scenario <new_scenario>.json`.

## Notes

- All scenarios use synthetic video — no real camera or SITL required.
- Scenarios are deterministic — same input always produces same output.
- KPI checks are enforced; a scenario that doesn't meet its KPIs is a failure.
