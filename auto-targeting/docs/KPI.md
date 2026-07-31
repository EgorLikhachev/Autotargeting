# KPI Dashboard

> Consolidated metrics. Each KPI has a target, the phase where it is verified,
> and the current measurement (filled in as phases complete).

## Latency KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Video latency (capture → frame available) | < 50 ms | 1 | HITL-T1 | — |
| Inference latency (NPU) | < 60 ms | 2 | HITL-T1 | — |
| Lock acquisition time | < 1 s | 3, 5 | SITL | — |
| Recovery time after occlusion | < 500 ms | 3 | SITL | — |
| MAVLink command send latency | < 5 ms | 4 | Unit test | — |
| End-to-end (capture → FC cmd) | < 150 ms | 6 | HITL-T2 | — |
| RC override response | < 200 ms | 7 | HITL-T3 | — |
| RTH activation time | < 1 s | 7 | HITL-T3 | — |

## Throughput KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Inference FPS | ≥ 15 | 2 | HITL-T1 | — |
| Video FPS (sustained 5 min) | ≥ 30 | 1 | HITL-T1 | — |
| FC command rate | 10 Hz (rate-limited) | 4 | Unit test | ✅ enforced by `CommandRateLimiter` |

## Accuracy KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| mAP (test dataset, selected classes) | > 0.70 | 2 | CI benchmark | — |
| Tracking accuracy (offset from GT) | < 5% of frame | 3 | SITL replay | — |
| Tracking success rate (real flight) | > 90% | 8 | Flight test | — |

## Stability KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Watchdog triggers (normal mode) | < 1 / hour | 5, 7 | HITL-T2 | — |
| Oscillation events (SITL 30 min) | 0 | 5 | SITL | — |
| HITL 8-hour stability run | no crash | 7 | HITL-T2 | — |
| Memory growth (8 hours) | < 50 MB | 7 | HITL-T2 | — |

## Code Quality KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Unit test pass rate | 100% | all | CI | ✅ 60/60 passing |
| Clippy warnings (with `-D warnings`) | 0 | all | CI | ✅ 0 |
| `cargo fmt --check` | clean | all | CI | ✅ clean |
| `cargo audit` critical CVEs | 0 | all | CI | — |
| `cargo deny` license violations | 0 | all | CI | — |
| Coverage on critical-path crates | > 80% | 5 | tarpaulin | — |

## Current Status (Phase 0)

- ✅ Workspace builds: `cargo build --workspace`
- ✅ All tests pass: 60 tests across 7 crates
- ✅ Clippy clean: `cargo clippy --workspace --all-targets -- -D warnings`
- ✅ Fmt clean: `cargo fmt --check`
- ✅ Smoke test binary runs: `cargo run -p auto-targeting-cli -- --mock-all`
- ✅ State machine: 10 transition tests, all allowed/disallowed edges verified
- ✅ Anti-loop guard: 7 tests covering deadband, clipping, oscillation detection
- ✅ Watchdogs: 6 tests covering registration, feeding, expiry, snapshots
- ✅ Mock FC adapter: 5 tests covering command recording, heartbeat simulation
- ✅ Rate limiter: 4 tests covering allow/drop/force semantics
- ✅ Kalman filter: 5 tests including velocity convergence
- ✅ Target tracker: 6 tests covering acquire/lock/loss/clear
- ✅ NMS: 3 tests covering overlap filtering
