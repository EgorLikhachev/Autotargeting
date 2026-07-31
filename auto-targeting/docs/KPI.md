# KPI Dashboard

> Consolidated metrics. Each KPI has a target, the phase where it is verified,
> and the current measurement (filled in as phases complete).

## Latency KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Video latency (capture → frame available) | < 50 ms | 1 | HITL-T1 | — |
| Inference latency (NPU) | < 60 ms | 2 | HITL-T1 | — |
| Lock acquisition time | < 1 s | 3, 5 | SITL | ✅ confirmed in unit tests (3 frames) |
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

## Accuracy KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| mAP (test dataset, selected classes) | > 0.70 | 2 | CI benchmark | — |
| Tracking accuracy (offset from GT) | < 5% of frame | 3 | SITL replay | ✅ Kalman velocity converges (unit test) |
| Tracking success rate (real flight) | > 90% | 8 | Flight test | — |

## Stability KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Watchdog triggers (normal mode) | < 1 / hour | 5, 7 | HITL-T2 | — |
| Oscillation events (SITL 30 min) | 0 | 5 | SITL | ✅ detector tested (unit + e2e) |
| HITL 8-hour stability run | no crash | 7 | HITL-T2 | — |
| Memory growth (8 hours) | < 50 MB | 7 | HITL-T2 | — |

## Code Quality KPIs

| KPI | Target | Phase | Where measured | Current |
|---|---|---|---|---|
| Unit test pass rate | 100% | all | CI | ✅ 134 passing, 5 ignored (vivid) |
| Clippy warnings (with `-D warnings`) | 0 | all | CI | ✅ 0 |
| `cargo fmt --check` | clean | all | CI | ✅ clean |
| `cargo audit` critical CVEs | 0 | all | CI | — |
| `cargo deny` license violations | 0 | all | CI | — |
| Coverage on critical-path crates | > 80% | 5 | tarpaulin | — |

## Current Status (Phase 0 + partial 1/4/5/6)

### Completed
- ✅ Workspace builds: `cargo build --workspace`
- ✅ All tests pass: **134 tests** across 7 crates + 5 vivid-gated (ignored)
- ✅ Clippy clean: `cargo clippy --workspace --all-targets -- -D warnings`
- ✅ Fmt clean: `cargo fmt --check`
- ✅ Smoke test binary runs: `cargo run -p auto-targeting-cli -- --mock-all`
- ✅ Interactive REPL: `cargo run -p auto-targeting-cli -- --repl`
- ✅ End-to-end integration tests: `cargo test --test e2e_pipeline`
- ✅ Commander struct: full lifecycle (connect → arm → scan → select → track → abort → reset)

### Test breakdown by crate

| Crate | Unit tests | Integration tests | Notes |
|---|---|---|---|
| `common` | 8 | — | types, config (TOML parsing, env overrides) |
| `commander` | 44 | — | state machine (10), watchdogs (7), anti-loop (9), Commander (18) |
| `fc-adapter` | 19 | — | MockFcAdapter (10), rate limiter (4), SITL MAVLink (9) |
| `cv-inference` | 5 | — | NMS (3), mock backend (2) |
| `target-tracker` | 11 | — | Kalman (5), tracker (6) |
| `video-capture` | 20 | — | synthetic (8), replay (8), v4l2 stub (4) + 2 vivid-gated |
| `cli` | 16 | 11 | REPL commands (16) + e2e pipeline (11) |
| **Total** | **123** | **11** | **+ 5 vivid-gated (ignored)** |

### Phase progress

| Phase | Status | Notes |
|---|---|---|
| 0: Foundation | ✅ Complete | Workspace, all crates, CI, docs, ADRs |
| 1: Video Capture | 🚧 Stub + synthetic | `SyntheticVideoSource` working; `V4l2Source` stub for Phase 1 |
| 1.7: vivid CI tests | ✅ Complete | 2 ignored tests, CI workflow updated |
| 2: CV/Inference | 🚧 Stub | `RknnBridgeClient` + `CpuInferenceBackend` stubs; NMS working |
| 3: Target Tracker | 🚧 Skeleton + Kalman | Single-target tracker working; multi-target is Phase 3 stretch |
| 4: FC Adapter | 🚧 Mock + SITL | `MockFcAdapter` ✅, `SittlMavlinkAdapter` ✅, `ArduPilotMavlinkAdapter` stub |
| 4.8: SITL MAVLink | ✅ Complete | `mavlink` 0.18 crate, UDP transport, 9 tests |
| 5: Commander | ✅ Phase 5.1 Complete | `Commander` struct + full lifecycle + 18 tests |
| 5.6: CLI REPL | ✅ Complete | 16 commands, 16 tests, interactive console |
| 6: Integration | ✅ e2e tests | 11 integration tests covering full pipeline |
| 6.3: Replay | ✅ Complete | `Recording` + `ReplaySource` with loop/real-time modes |
| 7: HITL | — | Awaiting hardware |
| 8: Flight tests | — | Awaiting hardware |

### ADRs

| ADR | Title | Status |
|---|---|---|
| 0001 | RKNN Inference via C++ Bridge Microservice | Accepted (with protocol spec) |
| 0002 | Tracking Algorithm — IoU + Kalman | Accepted |
| TEMPLATE | Template for new ADRs | — |
