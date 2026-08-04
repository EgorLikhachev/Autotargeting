# Auto-Targeting System

Companion computer for autonomous target tracking on a fixed-wing UAV.

## Status

🚧 **Phase 0 complete + partial 1/4/5/6.** See [`docs/KPI.md`](docs/KPI.md) for details.

- ✅ Cargo workspace with 7 crates, 105 unit tests passing
- ✅ Anti-loop protection: state machine, watchdogs, deadband, rate limiter, oscillation detector
- ✅ `MockFcAdapter` (in-memory) + `SittlMavlinkAdapter` (UDP MAVLink) — both working
- ✅ Interactive REPL: `cargo run -p auto-targeting-cli -- --repl`
- ✅ Replay infrastructure for regression tests
- 🚧 `V4l2Source` and `ArduPilotMavlinkAdapter` are stubs (need real hardware)

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for high-level design,
[`docs/HYPOTHESES.md`](docs/HYPOTHESES.md) for the hypothesis log, and
the top-level Roadmap document for the phased development plan.

## Hardware

- **Companion computer:** Orange Pi 5 (RK3588S, 6 TOPS NPU)
- **Camera:** Arducam UC-852 (USB UVC)
- **Flight controller:** SpeedyBee F405 running ArduPilot (reference platform)
- **Comms:** UART/USB MAVLink v2

## Build

```bash
# Host build (x86_64, for dev/tests)
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check

# Cross-compile for Orange Pi 5 (aarch64)
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

## Run

```bash
# Interactive REPL (operator console) — fully functional with mock FC
cargo run -p auto-targeting-cli -- --repl

# Smoke test with all mocks
cargo run -p auto-targeting-cli -- --mock-all

# Health check (for systemd / healthcheck scripts)
cargo run -p auto-targeting-cli -- --health-check

# With config file (production — Phase 5)
cargo run -p auto-targeting-cli -- --config config.toml
```

## Phase 1.1 — minimal CV loop (camera → model → detections)

The Phase 1.1 minimal contour runs the real model end-to-end on x86 (ONNX
Runtime) and, with hardware, on RK3588 (RKNN). See
[`docs/POC_PHASE_1_1.md`](docs/POC_PHASE_1_1.md) for the full write-up.

```bash
# 1) Fetch the baseline COCO YOLOv8n model (one-time, ~13 MB)
./scripts/download_models.sh

# 2) Single-image inference smoke (closes "запустить готовую модель")
cargo run -p cv-inference --example onnx_infer --features cpu-onnx -- \
    models/yolov8n.onnx path/to/image.jpg

# 3) 30-minute continuous soak test of the full loop
#    (synthetic source → model → annotate → metrics → telemetry)
./scripts/soak_30min.sh
#    → output/soak/summary.json   (FPS, p50/p95 latency)
#    → output/soak/telemetry.jsonl (RSS, CPU/NPU temperature)
#    → output/soak/frames/...      (annotated JPEGs)

# 4) Mux the annotated frames into a processed demo video
./scripts/make_video.sh output/soak 15   # → output/soak/processed.mp4
```

The CPU path requires the `cpu-onnx` feature (ONNX Runtime, auto-downloaded).
On RK3588, convert the model first:

```bash
python scripts/convert_rknn.py --onnx models/yolov8n.onnx \
    --out models/yolov8n_int8.rknn --platform rk3588
```

New Phase 1.1 crates:

- `yolov8` — backend-agnostic letterbox + output parser (shared by CPU & NPU)
- `cv-inference` (feature `cpu-onnx`) — real ONNX Runtime backend
- `cv-visualizer` — headless bbox/label annotation + JPEG/JSONL writer
- `system-telemetry` — VmRSS, CPU/NPU temperature, FPS/latency recorder

## REPL commands

```
help                         — show available commands
status                       — system state, FC, watchdogs
arm / disarm                 — arm or disarm the drone
set-mode <guided|rtl|...>   — change FC flight mode
scan                         — start scanning for targets
select-target <id>          — select target, transition to TRACKING
abort                        — ABORT (force transition + RTL)
reset                        — return to IDLE (after ABORT + disarm)
watchdogs                    — show watchdog statuses
anti-loop                    — show anti-loop guard stats
feed-watchdog <name>         — manually feed a watchdog
simulate-heartbeat-loss      — test: simulate FC heartbeat loss
simulate-attitude <r p y>    — test: inject attitude update
quit                         — exit
```

## Testing

```bash
# All unit tests
cargo test --workspace

# Vivid-gated tests (requires `sudo modprobe vivid`)
sudo modprobe vivid
cargo test -p video-capture -- --include-ignored vivid

# CI-equivalent check
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Repository layout

```
auto-targeting/
├── crates/
│   ├── common/           # shared types, errors, TOML config
│   ├── video-capture/    # V4L2 + SyntheticVideoSource + ReplaySource
│   ├── cv-inference/     # InferenceBackend trait + Mock + NMS + RKNN stub
│   ├── target-tracker/   # KalmanFilter2D + single-target tracker
│   ├── fc-adapter/       # FlightControllerAdapter trait + Mock + SITL MAVLink
│   ├── commander/        # state machine + watchdogs + anti-loop guard
│   └── cli/              # auto-targeting binary + interactive REPL
├── docs/
│   ├── ARCHITECTURE.md
│   ├── HYPOTHESES.md
│   ├── KPI.md
│   ├── SAFETY.md
│   └── ADR/
│       ├── 0001-rknn-cpp-bridge.md
│       ├── 0002-tracking-algorithm.md
│       └── TEMPLATE.md
├── deploy/
│   ├── systemd/
│   │   ├── auto-targeting.service
│   │   └── rknn-bridge.service
│   └── scripts/
│       └── healthcheck.sh
├── sim/
│   ├── sitl/             # docker-compose.yml for ArduPilot SITL
│   └── scenarios/        # replay scenarios (Phase 6)
├── .github/workflows/
│   ├── ci.yml            # PR checks + cross-compile
│   └── nightly.yml       # full SITL + benchmarks
├── Cargo.toml            # workspace
├── config.example.toml   # config template
├── rust-toolchain.toml
├── rustfmt.toml
├── clippy.toml
└── deny.toml
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for full module descriptions.
