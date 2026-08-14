# Auto-Targeting — Rust workspace

> This is the **workspace developer README** (inside the `auto-targeting/` folder).
> For the project overview, badges, quickstart, and configuration, see the
> **[top-level README](../README.md)**. This file covers build/test/run details
> and the crate layout for people working inside the workspace.

Rust workspace + C++ `rknn-bridge` microservice for the Auto-Targeting System
(visual target tracking on a fixed-wing UAV, RK3588 NPU).

## Status

🟢 **Phase 1.1 (minimal CV loop) — validated on real hardware.**

- ✅ Cargo workspace with **10 crates**, **294 unit tests** passing
- ✅ C++ `rknn-bridge`: **6 NMS tests**, linked to `librknnrt.so` 2.3.0
- ✅ End-to-end detections on RK3588 NPU (zero-copy, **~29 ms** inference, **~34 FPS**)
- ✅ Live camera demo → annotated video (`examples/live_camera_demo.rs`)
- ✅ Anti-loop protection: state machine, watchdogs, deadband, rate limiter, oscillation detector
- ✅ `MockFcAdapter` (in-memory) + `SittlMavlinkAdapter` (UDP MAVLink) — both working
- 🚧 `ArduPilotMavlinkAdapter` is still a stub (real FC integration = Phase 2)

See [`docs/PROJECT_REPORT.md`](docs/PROJECT_REPORT.md) for the full results
report and [`docs/HARDWARE_TEST_RESULTS.md`](docs/HARDWARE_TEST_RESULTS.md) for
raw numbers. For the high-level design see
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md); for the hypothesis log see
[`docs/HYPOTHESES.md`](docs/HYPOTHESES.md).

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

The toolchain is pinned via [`rust-toolchain.toml`](rust-toolchain.toml)
(stable, MSRV 1.75). Style rules live in
[`rustfmt.toml`](rustfmt.toml) and [`clippy.toml`](clippy.toml).

## Run

```bash
# Interactive REPL (operator console) — fully functional with the mock FC
cargo run -p auto-targeting-cli -- --repl

# Smoke test the whole pipeline with all mocks
cargo run -p auto-targeting-cli -- --mock-all

# Health check (used by systemd / healthcheck scripts)
cargo run -p auto-targeting-cli -- --health-check

# With a config file (production / on-device)
cargo run -p auto-targeting-cli -- --config config.toml
```

See [`config.example.toml`](config.example.toml) for the full config template
and the `AT_` env-var override scheme (e.g. `AT_VIDEO__DEVICE=/dev/video1`).

## Phase 1.1 — minimal CV loop (camera → model → detections)

```bash
# 1) Fetch the baseline COCO YOLOv8n model (one-time, ~13 MB)
./scripts/download_models.sh

# 2) Single-image inference smoke (x86, ONNX Runtime)
cargo run -p cv-inference --example onnx_infer --features cpu-onnx -- \
    models/yolov8n.onnx path/to/image.jpg

# 3) On-device live demo: camera → NPU → annotated video (Unix + bridge running)
cargo build --release -p cv-inference --examples --features "cpu-onnx,v4l2-cam"
./target/release/examples/live_camera_demo \
    --device /dev/video0 --duration 15 \
    --output output/live --model yolov8n_int8.rknn

# 4) Convert the model for the NPU (on an x86 host with rknn-toolkit2)
python scripts/convert_rknn.py --onnx models/yolov8n.onnx \
    --out models/yolov8n_int8.rknn --platform rk3588
```

Phase 1.1 crates and examples:

- `yolov8` — backend-agnostic letterbox + output parser (shared by CPU & NPU)
- `cv-inference` (feature `cpu-onnx`) — real ONNX Runtime backend + `rknn-bridge` client
- `cv-visualizer` — headless bbox/label annotation + JPEG/JSONL writer
- `system-telemetry` — VmRSS, CPU/NPU temperature, FPS/latency recorder
- `examples/onnx_infer.rs`, `examples/soak.rs`, `examples/live_camera_demo.rs`
- `video-capture` `examples/camera_latency.rs` (with `--pipeline` A/B mode),
  `examples/direct_capture_bench.rs`

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
# All unit tests (mock backends — no hardware required)
cargo test --workspace

# Feature-gated real-inference tests (ONNX Runtime on x86)
cargo test -p cv-inference --features cpu-onnx

# Vivid-gated tests (requires `sudo modprobe vivid`)
sudo modprobe vivid
cargo test -p video-capture -- --include-ignored vivid

# C++ rknn-bridge (Unix only)
cd rknn-bridge && cmake -B build -DBUILD_TESTS=ON && cmake --build build -j
./build/test_nms        # 6/6

# CI-equivalent
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo test --workspace
```

## Workspace layout

```
auto-targeting/
├── crates/
│   ├── common/           # shared types, errors, TOML config
│   ├── video-capture/    # V4L2 (v4l + direct ioctl) + synthetic + replay
│   ├── cv-inference/     # InferenceBackend trait + ONNX + rknn-bridge client
│   ├── yolov8/           # pure-Rust letterbox + postprocess (NMS)
│   ├── cv-visualizer/    # headless annotation (boxes + labels → JPEG/JSONL)
│   ├── system-telemetry/ # RSS, CPU/NPU temp, latency p50/p95
│   ├── target-tracker/   # KalmanFilter2D + single-target tracker (Phase 1.2)
│   ├── fc-adapter/       # FlightControllerAdapter trait + Mock + SITL MAVLink
│   ├── commander/        # state machine + watchdogs + anti-loop guard
│   └── cli/              # auto-targeting binary + interactive REPL
├── rknn-bridge/          # C++ NPU microservice (zero-copy rknn_set_io_mem)
├── docs/                 # SDD-SPEC, ARCHITECTURE, KPI, SAFETY, ADR, reports
├── scripts/              # model conversion, device setup, hardware tests
├── sim/                  # ArduPilot SITL docker + replay scenarios
├── deploy/               # systemd units + healthcheck
├── Cargo.toml            # workspace manifest
├── config.example.toml   # config template
├── rust-toolchain.toml
├── rustfmt.toml
├── clippy.toml
└── deny.toml
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for full module descriptions
and [`docs/SDD-SPEC.md`](docs/SDD-SPEC.md) for the complete specification.
