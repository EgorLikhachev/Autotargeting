# Hypotheses Log

> Living document. Every architectural assumption is recorded here BEFORE it
> enters production code. Each hypothesis has a unique ID (H-NNN), a priority,
> and a lifecycle: `OPEN` → `TESTING` → `CONFIRMED` / `REFUTED` / `MITIGATED`.
>
> **Critical** hypotheses block Flight Readiness (see SAFETY.md §Flight
> Readiness Criteria, item E1).

## Lifecycle

```
OPEN ──(start testing)──► TESTING ──(verified)──► CONFIRMED
                                  └──(falsified)──┬──► REFUTED
                                                   └──► MITIGATED (workaround applied)
```

## Format

```markdown
## H-NNN: <short title>

- **Priority:** CRITICAL | HIGH | MEDIUM | LOW
- **Owner:** <name or @github>
- **Created:** YYYY-MM-DD
- **Phase:** <Roadmap phase number>
- **Related ADR:** ADR-XXXX or —

### Hypothesis
<Concretely falsifiable statement. Avoid "works well" — only measurable claims.>

### Method of verification
<Step-by-step test plan. Must be reproducible by another developer.>

### Predicted outcome
<What we expect to see, written BEFORE the test — prevents confirmation bias.>

### Test result
<Filled in after testing: numbers, observations, links to logs/benchmarks.>

### Status: OPEN | TESTING | CONFIRMED | REFUTED | MITIGATED

### Mitigation plan (if refuted)
<Actionable fallback, already designed.>
```

---

## H-001: Mature Rust bindings exist for RKNPU2 SDK

- **Priority:** CRITICAL
- **Owner:** TBD
- **Created:** 2026-08-01
- **Phase:** 2 (CV/Inference)
- **Related ADR:** ADR-0001 (pending)

### Hypothesis
Production-ready Rust crates exist (e.g. `rknn-rs`, `rusty-rknn`) that allow
running YOLOv8 INT8 inference on RK3588S NPU directly from Rust, without a
C++ bridge. If true, we can simplify the architecture by removing
`rknn-bridge/`.

### Method of verification
1. Search crates.io and GitHub for `rknn`, `rknpu`, `rockchip npu`.
2. For each candidate: check stars, last commit, open issues (esp. RK3588S
   support), docs.
3. If a crate looks mature: build a minimal example — load YOLOv8n RKNN
   model, infer one frame.
4. Benchmark latency on Orange Pi 5 vs the C++ bridge baseline.

### Predicted outcome
Per devops advice (advice #1): mature Rust bindings probably do NOT exist.
Expect experimental crates with last commit > 1 year ago, no RK3588S support.

### Test result
_(not yet tested)_

### Status: OPEN

### Mitigation plan (if refuted)
Implement C++ microservice `rknn-bridge/` as designed. Communicate with
Rust orchestrator via Unix socket + shared memory. This is already the
default architecture.

---

## H-002: ArduPilot handles 10 Hz `SET_POSITION_TARGET_LOCAL_NED` without overload

- **Priority:** CRITICAL
- **Owner:** TBD
- **Created:** 2026-08-01
- **Phase:** 4 (FC Adapter)
- **Related ADR:** —

### Hypothesis
ArduPilot on SpeedyBee F405 (STM32F4 @ 168 MHz) can process a 10 Hz stream
of `SET_POSITION_TARGET_LOCAL_NED` MAVLink messages without degrading other
functions (stabilization, GPS, telemetry). Per-message processing latency
< 10 ms.

### Method of verification
1. Connect SpeedyBee F405 to PC via USB (real FC, not SITL).
2. Flash latest stable ArduPilot Plane firmware.
3. Run Rust test: stream `SET_POSITION_TARGET_LOCAL_NED` at 10 Hz for 5 min.
4. Measure: per-message processing latency (from ArduPilot logs), FC CPU
   load (via `STATS` MAVLink message), heartbeat misses.
5. Repeat at 20 Hz, 50 Hz to find the ceiling.

### Predicted outcome
10 Hz should be fine. 50 Hz likely overloads.

### Test result
_(not yet tested)_

### Status: OPEN

---

## H-003: Arducam UC-852 supports V4L2 + MJPEG at 720p@30 FPS on Orange Pi 5

- **Priority:** HIGH
- **Owner:** TBD
- **Created:** 2026-08-01
- **Phase:** 1 (Video Capture)
- **Related ADR:** —

### Hypothesis
Arducam UC-852 is recognized as a standard UVC device in Linux, supports
MJPEG format at 720p (1280×720) @ 30 FPS via V4L2 API. No proprietary
drivers required.

### Method of verification
1. Plug camera into Orange Pi 5.
2. `lsusb` — verify device is recognized.
3. `v4l2-ctl --list-formats-ext -d /dev/video0` — print supported formats.
4. `ffplay /dev/video0` — visually verify image.
5. Run `video-capture-test` binary, measure latency.

### Predicted outcome
UVC-compliant, MJPEG 720p@30 should work out of the box.

### Test result
_(not yet tested — requires physical hardware)_

### Status: OPEN

---

## H-004: `mavlink` Rust crate is stable and supports all required messages

- **Priority:** HIGH
- **Owner:** TBD
- **Created:** 2026-08-01
- **Phase:** 4 (FC Adapter)
- **Related ADR:** —

### Hypothesis
The `mavlink` crate (https://crates.io/crates/mavlink) is stable, supports
MAVLink v2 over serial and UDP, and exposes all message types we need:
`HEARTBEAT`, `ATTITUDE`, `GLOBAL_POSITION_INT`, `SET_POSITION_TARGET_LOCAL_NED`,
`COMMAND_LONG` (for `MAV_CMD_DO_SET_ROI`).

### Method of verification
1. Add `mavlink = "0.13"` (or latest) to `fc-adapter/Cargo.toml`.
2. Write a 50-line test: connect to SITL (UDP), send HEARTBEAT, receive
   ATTITUDE, send SET_POSITION_TARGET_LOCAL_NED.
3. Run against ArduPilot SITL in Docker.

### Predicted outcome
Per devops advice (advice #4): should work fine.

### Test result
_(not yet tested)_

### Status: OPEN

---

## Template (copy this for new hypotheses)

```markdown
## H-NNN: <short title>

- **Priority:** CRITICAL | HIGH | MEDIUM | LOW
- **Owner:** <name>
- **Created:** YYYY-MM-DD
- **Phase:** <number>
- **Related ADR:** ADR-XXXX or —

### Hypothesis
<Falsifiable statement.>

### Method of verification
<Reproducible test plan.>

### Predicted outcome
<What you expect, written before testing.>

### Test result
<Numbers, observations, links.>

### Status: OPEN

### Mitigation plan (if refuted)
<Actionable fallback.>
```
