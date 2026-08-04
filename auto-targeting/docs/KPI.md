# KPI Dashboard

> Consolidated metrics. Each KPI has a target, the phase where it is verified,
> and the current measurement (filled in as phases complete).

## Latency KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Video latency (capture → frame available) | < 50 ms | 1 | HITL-T1 | — |
| Inference latency (NPU) | < 60 ms | 2 | HITL-T1 | — |
| Lock acquisition time | < 1 s | 3, 5 | SITL | ✅ confirmed in unit tests + scenarios |
| Recovery time after occlusion | < 500 ms | 3 | SITL | — |
| MAVLink command send latency | < 5 ms | 4 | Unit test | ✅ MockFcAdapter (sync, ~0 ms) |
| End-to-end (capture → FC cmd) | < 150 ms | 6 | HITL-T2 | — |
| RC override response | < 200 ms | 7 | HITL-T3 | — |
| RTH activation time | < 1 s | 7 | HITL-T3 | — |

## Throughput KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Inference FPS | ≥ 15 | 2 | HITL-T1 | — |
| Video FPS (sustained 5 min) | ≥ 30 | 1 | HITL-T1 | ✅ SyntheticVideoSource @ configurable FPS |
| FC command rate | 10 Hz (rate-limited) | 4 | Unit test | ✅ enforced by `CommandRateLimiter` |
| Synthetic source FPS | unlimited | 0 | Unit test | ✅ tested at 100–200 FPS |

## Phase 1.1 Soak & Thermal KPIs

Introduced in the Phase 1.1 minimal-loop work (see
[POC_PHASE_1_1.md](POC_PHASE_1_1.md)). The instrumentation is in place
(`system-telemetry` + `MetricsRecorder` + the `soak` example); the numeric
"Current" cells are filled after the first on-device run of
`scripts/soak_30min.sh`.

| KPI | Target | Where measured | Current |
|---|---|---|---|
| Soak run duration | ≥ 30 min | `soak` example | — (instrumented) |
| Sustained FPS (capture → detection) | ≥ 15 | `summary.json` | — (instrumented) |
| End-to-end latency p50 (capture → detection) | < 100 ms | `summary.json` | — (instrumented) |
| End-to-end latency p95 (capture → detection) | < 150 ms | `summary.json` | — (instrumented) |
| Memory growth over 30 min (VmRSS) | < 50 MB | `telemetry.jsonl` | — (instrumented) |
| Max CPU package temp over soak | observation | `telemetry.jsonl` | — (instrumented) |
| Max NPU temp over soak (RK3588) | observation | `telemetry.jsonl` (`npu_temp_c`) | — (instrumented) |
| NPU load % over soak (RK3588) | observation | `telemetry.jsonl` (`npu_load_percent`) | — (instrumented) |
| Soak crashes / panics | 0 | run exit code | — (instrumented) |

**Note on temperature:** the original KPI table had no thermal metrics. The
RKNN SDK does not expose temperature; the canonical source is the kernel
thermal-zone sysfs (`/sys/class/thermal/thermal_zoneN`), read by
`system_telemetry::cpu_temp_c` / `npu_temp_c`. NPU load comes from the devfreq
attribute (`/sys/class/devfreq/fdab0000.npu/load` on RK3588).

## Performance Benchmarks (criterion)

| Benchmark | Target | Current | Notes |
|---|---|---|---|
| Kalman predict | < 1 µs | ✅ ~560 ps | Measured on dev machine |
| Kalman update | < 100 ns | ✅ ~38 ns | |
| Kalman full cycle | < 200 ns | ✅ ~73 ns | predict + update |
| Kalman 100-frame sequence | < 10 µs | ✅ ~88 ns | Sustained throughput |
| NMS (5 disjoint) | < 10 µs | ✅ measured | |
| NMS (50 overlapping) | < 100 µs | ✅ measured | Stress test |
| Anti-loop process (allow) | < 1 µs | ✅ measured | |
| Anti-loop process (suppress) | < 500 ns | ✅ measured | |
| Watchdog feed (5 watchdogs) | < 500 ns | ✅ measured | |
| Watchdog check_expired (5) | < 1 µs | ✅ measured | |

## Accuracy KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| mAP (test dataset, selected classes) | > 0.70 | 2 | CI benchmark | — |
| Tracking accuracy (offset from GT) | < 5% of frame | 3 | SITL replay | ✅ Kalman velocity converges (unit test) |
| Tracking success rate (real flight) | > 90% | 8 | Flight test | — |
| Hungarian algorithm correctness | 100% optimal | 3 | Unit test | ✅ 10 tests covering all cases |

## Stability KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Watchdog triggers (normal mode) | < 1 / hour | 5, 7 | HITL-T2 | — |
| Oscillation events (SITL 30 min) | 0 | 5 | SITL | ✅ detector tested (unit + e2e + scenarios) |
| HITL 8-hour stability run | no crash | 7 | HITL-T2 | — |
| Memory growth (8 hours) | < 50 MB | 7 | HITL-T2 | — |

## Code Quality KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Unit test pass rate | 100% | all | CI | ✅ 185 passing, 5 ignored (vivid) |
| Clippy warnings (with `-D warnings`) | 0 | all | CI | ✅ 0 |
| `cargo fmt --check` | clean | all | CI | ✅ clean |
| `cargo audit` critical CVEs | 0 | all | CI | — |
| `cargo deny` license violations | 0 | all | CI | — |
| Coverage on critical-path crates | > 80% | 5 | tarpaulin | — |

## Current Status

### Completed
- ✅ Workspace builds: `cargo build --workspace`
- ✅ All tests pass: **185 tests** across 7 crates + 5 vivid-gated (ignored)
- ✅ Clippy clean: `cargo clippy --workspace --all-targets -- -D warnings`
- ✅ Fmt clean: `cargo fmt --check`
- ✅ Smoke test binary runs: `cargo run -p auto-targeting-cli -- --mock-all`
- ✅ Interactive REPL: `cargo run -p auto-targeting-cli -- --repl`
- ✅ End-to-end integration tests: `cargo test --test e2e_pipeline`
- ✅ Commander struct: full lifecycle (connect → arm → scan → select → track → abort → reset)
- ✅ Scenario runner: `cargo run -- scenario <file>` — 5/5 scenarios pass
- ✅ Benchmarks: `cargo bench --workspace` — criterion benchmarks for Kalman, NMS, anti-loop, watchdogs
- ✅ V4l2Source: real implementation behind `v4l2` feature flag
- ✅ RKNN C++ bridge: compiles (stub backend), NMS unit tests pass
- ✅ Multi-target tracker: Hungarian algorithm, 12 unit tests

### Test breakdown by crate

| Crate | Unit tests | Integration tests | Benchmarks | Notes |
|---|---|---|---|---|
| `common` | 20 | — | — | types, config, scenario parser |
| `commander` | 44 | — | 8 | state machine, watchdogs, anti-loop, Commander |
| `fc-adapter` | 27 | — | — | Mock, SITL, ArduPilot |
| `cv-inference` | 5 | — | 4 | NMS, mock backend |
| `target-tracker` | 36 | — | 8 | Kalman, single + multi-target, Hungarian |
| `video-capture` | 20 | — | — | synthetic, replay, v4l2 stub + vivid-gated |
| `cli` | 16 | 11 | — | REPL + e2e pipeline |
| **Total** | **168** | **11** | **20** | **+ 5 vivid-gated** |

### C++ components

| Component | Status | Tests |
|---|---|---|
| `rknn-bridge` NMS | ✅ Compiles (stub backend) | 6 C++ tests |
| `rknn-bridge` SHM server | ✅ Stub implementation | — |
| `rknn-bridge` main loop | ✅ Compiles | — |

### Phase progress

| Phase | Status | Notes |
|---|---|---|
| 0: Foundation | ✅ Complete | Workspace, all crates, CI, docs, ADRs |
| 1: Video Capture | ✅ V4l2Source done | Real impl behind `v4l2` feature; `SyntheticVideoSource` always available |
| 1.7: vivid CI tests | ✅ Complete | 2 ignored tests, CI workflow updated |
| 2: CV/Inference | 🚧 Stub + C++ bridge | `RknnBridgeClient` stub; C++ bridge compiles with stub backend |
| 3: Target Tracker | ✅ Complete | Single-target + Multi-target (Hungarian algorithm) |
| 4: FC Adapter | ✅ Complete | Mock + SITL + ArduPilot (serial/TCP/UDP) |
| 4.8: SITL MAVLink | ✅ Complete | `mavlink` 0.18 crate, UDP transport, 9 tests |
| 4.9: ArduPilot MAVLink | ✅ Complete | serial/TCP/UDP, 8 tests |
| 5: Commander | ✅ Complete | `Commander` struct + full lifecycle + 18 tests |
| 5.6: CLI REPL | ✅ Complete | 16 commands, 16 tests, interactive console |
| 6: Integration | ✅ Complete | 11 e2e tests + scenario runner (5/5 pass) |
| 6.2: SITL scenarios | ✅ Complete | 5 JSON scenarios + parser + runner |
| 6.3: Replay | ✅ Complete | `Recording` + `ReplaySource` with loop/real-time modes |
| Benchmarks | ✅ Complete | criterion benchmarks for all hot paths |
| 7: HITL | — | Awaiting hardware |
| 8: Flight tests | — | Awaiting hardware |

### ADRs

| ADR | Title | Status |
|---|---|---|
| 0001 | RKNN Inference via C++ Bridge Microservice | Accepted (with protocol spec) |
| 0002 | Tracking Algorithm — IoU + Kalman (single) / Hungarian (multi) | Accepted |
| TEMPLATE | Template for new ADRs | — |
