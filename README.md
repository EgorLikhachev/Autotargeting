# Auto-Targeting System

[![CI](https://github.com/EgorLikhachev/Autotargeting/actions/workflows/ci.yml/badge.svg)](https://github.com/EgorLikhachev/Autotargeting/actions/workflows/ci.yml)
[![Nightly](https://github.com/EgorLikhachev/Autotargeting/actions/workflows/nightly.yml/badge.svg)](https://github.com/EgorLikhachev/Autotargeting/actions/workflows/nightly.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-stable%20%3E%3D%201.85-orange.svg)](https://www.rust-lang.org)
[![Version](https://img.shields.io/badge/version-0.1.0--phase--1.1-green.svg)](CHANGELOG.md)

**Auto-Targeting System** is a companion computer for autonomous visual target
tracking on a fixed-wing UAV. It captures live video, runs YOLOv8n object
detection on the **RK3588 NPU** (Rockchip, 6 TOPS), tracks a selected target,
and commands the flight controller over MAVLink — all behind anti-loop
safety guards (state machine, watchdogs, deadband, oscillation detector).

Phase 1.1 — the minimal CV loop (camera → model → detections) — is
**validated on real hardware**: end-to-end detections on an Orange Pi 5 with a
USB camera, NPU inference at ~29 ms/frame (~34 FPS).

> **⚠️ Safety-critical.** This software is intended to command a flying vehicle.
> Never fly with a configuration that has not passed the
> [Flight Readiness Criteria](auto-targeting/docs/SAFETY.md). The default FC
> adapter is `mock` precisely so the system cannot move a real vehicle out of
> the box.

---

## Table of contents

- [Overview](#overview)
- [Hardware](#hardware)
- [Prerequisites](#prerequisites)
- [Installation](#installation)
- [Configuration](#configuration)
- [Usage](#usage)
- [Testing](#testing)
- [Cross-compiling for Orange Pi 5](#cross-compiling-for-orange-pi-5)
- [Project structure](#project-structure)
- [Contributing](#contributing)
- [License](#license)
- [Acknowledgements](#acknowledgements)

---

## Overview

The system is a **Rust workspace (10 crates)** plus a small **C++ NPU
microservice (`rknn-bridge`)**. Rust handles capture, post-processing, tracking,
safety, and the operator CLI; C++ talks to the proprietary RKNN driver through
a Unix socket with a JSON protocol.

```
USB/MIPI camera ─▶ video-capture ─▶ cv-inference ─▶ rknn-bridge (C++) ─▶ RK3588 NPU
                       │                  ▲
                       ▼                  │
                 target-tracker ──▶ commander (FSM + safety) ──▶ fc-adapter (MAVLink)
```

| What | Status |
|---|---|
| Cargo workspace, 10 crates, **294 unit tests** | ✅ |
| C++ `rknn-bridge`, **6 NMS tests**, linked to `librknnrt.so` 2.3.0 | ✅ |
| End-to-end detections on RK3588 NPU (zero-copy, ~29 ms) | ✅ |
| Live camera demo → annotated video | ✅ |
| Anti-loop protection (state machine, watchdogs, oscillation detector) | ✅ |
| Mock + SITL MAVLink adapters | ✅ |
| Real FC integration, multi-target tracking (Phase 2) | 🚧 |

See [`auto-targeting/docs/PROJECT_REPORT.md`](auto-targeting/docs/PROJECT_REPORT.md)
for the full results report and [`auto-targeting/docs/HARDWARE_TEST_RESULTS.md`](auto-targeting/docs/HARDWARE_TEST_RESULTS.md)
for raw numbers.

---

## Hardware

- **Companion computer:** Orange Pi 5 (RK3588, 6 TOPS NPU)
- **Camera:** Arducam OV9782 USB (global-shutter UVC, MJPG)
- **Flight controller:** SpeedyBee F405 running ArduPilot (reference platform)
- **Comms:** UART/USB MAVLink v2

---

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| **Rust** | stable, **≥ 1.85** (MSRV) | set via `rust-toolchain.toml` |
| **Git** | any recent | |
| **CMake** | ≥ 3.16 | only for `rknn-bridge` |
| **C++ compiler** | GCC ≥ 10 / Clang ≥ 12 | only for `rknn-bridge` |
| **Linux** | any modern | `v4l2-cam`/`v4l2-direct` and the bridge are Unix-only |
| **ONNX Runtime** | fetched automatically | `cpu-onnx` feature pulls prebuilt binaries |
| **RKNN SDK** | 2.3.0 | on-device only; `librknnrt.so` + `rknn_api.h` |

On Debian/Ubuntu:

```bash
sudo apt update
sudo apt install -y build-essential cmake pkg-config libssl-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

---

## Installation

```bash
# 1. Clone
git clone https://github.com/EgorLikhachev/Autotargeting.git
cd Autotargeting

# 2. The Rust workspace lives one level in (auto-targeting/).
cd auto-targeting

# 3. Build (the toolchain is pinned via rust-toolchain.toml)
cargo build --workspace
```

> **Repository layout note:** the Rust workspace is nested under
> `auto-targeting/` (see [Project structure](#project-structure)). This is
> intentional — CI and paths assume it. Run all `cargo` commands from inside
> `auto-targeting/`.

Models are not bundled. Download YOLOv8n weights:

```bash
./scripts/download_models.sh        # fetches yolov8n.onnx + yolov8n.pt
```

---

## Configuration

The system reads a TOML config file. A fully commented template lives at
[`auto-targeting/config.example.toml`](auto-targeting/config.example.toml).
Copy and edit it:

```bash
cp auto-targeting/config.example.toml auto-targeting/config.toml
# edit auto-targeting/config.toml
```

### Environment variables

Any config key can be overridden via environment variables using the `AT_`
prefix and `__` as the section separator (powered by `figment`). For example,
to override `[video] device` and `[fc] adapter`:

| Variable | Maps to | Default |
|---|---|---|
| `AT_VIDEO__DEVICE` | `video.device` | `/dev/video0` |
| `AT_VIDEO__FPS` | `video.fps` | `30` |
| `AT_INFERENCE__MODEL_PATH` | `inference.model_path` | `/opt/auto-targeting/models/yolov8n_int8.rknn` |
| `AT_INFERENCE__CONFIDENCE_THRESHOLD` | `inference.confidence_threshold` | `0.45` |
| `AT_FC__ADAPTER` | `fc.adapter` | `mock` (`mock` \| `sitl-mavlink` \| `ardupilot-mavlink`) |
| `AT_FC__ENDPOINT` | `fc.endpoint` | `127.0.0.1:14550` |
| `RUST_LOG` | tracing filter | `info,auto_targeting=debug` |

> **Safety default:** `fc.adapter = "mock"`. The system will **not** command a
> real flight controller until you explicitly set it to `sitl-mavlink` or
> `ardupilot-mavlink` and pass the Flight Readiness Criteria.

---

## Usage

All commands run from `auto-targeting/`.

```bash
# Interactive operator REPL — fully functional with the mock FC (no hardware needed)
cargo run -p auto-targeting-cli -- --repl

# Smoke test the whole pipeline with all mocks
cargo run -p auto-targeting-cli -- --mock-all

# Health check (used by systemd / healthcheck scripts)
cargo run -p auto-targeting-cli -- --health-check

# Run with a config file (production / on-device)
cargo run -p auto-targeting-cli -- --config config.toml
```

### Phase 1.1 — minimal CV loop (camera → model → detections)

```bash
# x86 dev: ONNX Runtime inference on a single image or video
cargo run -p cv-inference --example onnx_infer --features cpu-onnx -- path/to/image.jpg

# On-device live demo: camera → NPU → annotated video (Unix + rknn-bridge running)
cargo build --release -p cv-inference --examples --features "cpu-onnx,v4l2-cam"
./target/release/examples/live_camera_demo \
  --device /dev/video0 --duration 15 \
  --output output/live --model yolov8n_int8.rknn
```

The hardware scripts in [`auto-targeting/scripts/`](auto-targeting/scripts/)
automate device setup, model conversion, and the full on-device test suite —
see [`auto-targeting/docs/HARDWARE_TEST_RESULTS.md`](auto-targeting/docs/HARDWARE_TEST_RESULTS.md).

---

## Testing

```bash
cd auto-targeting

# All unit tests (mock backends only — no hardware required)
cargo test --workspace

# Lint (must be clean; CI enforces -D warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Formatting check
cargo fmt --check

# License/dependency policy
cargo deny check

# Feature-gated real-inference tests (ONNX Runtime on x86)
cargo test -p cv-inference --features cpu-onnx
```

### On-device (Orange Pi 5)

```bash
./scripts/run_hardware_tests.sh     # 294 unit tests + 6 C++ NMS tests + bridge smoke
./scripts/soak_30min.sh             # 30-min endurance run with telemetry
./scripts/verify_camera.sh          # capture/decode latency benchmark
```

---

## Cross-compiling for Orange Pi 5

```bash
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu -p auto-targeting-cli
```

The C++ `rknn-bridge` is built natively on the device (it links the
prebuilt `librknnrt.so` for aarch64). Use [`auto-targeting/scripts/setup_orange_pi.sh`](auto-targeting/scripts/setup_orange_pi.sh)
for a one-shot device setup, and [`auto-targeting/scripts/convert_rknn.py`](auto-targeting/scripts/convert_rknn.py)
to convert `yolov8n.onnx → yolov8n_int8.rknn`.

---

## Project structure

```
Autotargeting/                     # git root (GitHub front door)
├── README.md                      # you are here
├── LICENSE-MIT                    # dual license
├── LICENSE-APACHE
├── CONTRIBUTING.md
├── CHANGELOG.md
├── .github/                       # CI workflows + issue/PR templates
└── auto-targeting/                # Rust workspace + C++ bridge
    ├── crates/
    │   ├── common/                # shared types, errors, TOML config (+[bus])
    │   ├── video-capture/         # V4L2 (direct ioctl) + synthetic + replay + convert
    │   ├── yolov8/                # pure-Rust letterbox + postprocess (NMS)
    │   ├── cv-inference/          # InferenceBackend + ONNX + rknn-bridge client (SHM)
    │   ├── cv-visualizer/         # annotation + OSD (draw_osd)
    │   ├── system-telemetry/      # RSS, CPU/NPU temp, latency p50/p95
    │   ├── target-tracker/        # Kalman + Hungarian (library)
    │   ├── fc-adapter/            # FlightControllerAdapter: Mock/SITL/ArduPilot
    │   ├── shmem-buffer/          # TG26-160: SPMC frame ring in /dev/shm
    │   ├── event-bus/             # D-014: Zenoh typed pub/sub + bus_dump/track_gen
    │   ├── detector/              # TG26-35: ring → NPU → at/detections
    │   ├── tracker/               # M2: at/detections → at/tracks
    │   ├── fc-bridge/             # M3: FC ↔ bus (telemetry/commands/events)
    │   ├── video-recorder/        # TG26-125: MP4 + OSD, camera_publisher example
    │   ├── commander/             # state machine + watchdogs + anti-loop + bus_runner
    │   └── cli/                   # auto-targeting: REPL + bus-mon/repl-bus/config
    ├── rknn-bridge/               # C++ NPU microservice (zero-copy rknn_set_io_mem)
    ├── docs/                      # SDD-SPEC, ARCHITECTURE, KPI, SAFETY, ADR, reports
    ├── scripts/                   # model conversion, device setup, hardware tests
    ├── sim/                       # ArduPilot SITL docker + replay scenarios
    └── deploy/                    # systemd units + healthcheck
```

For the module-level design, see [`auto-targeting/docs/ARCHITECTURE.md`](auto-targeting/docs/ARCHITECTURE.md).
For the full spec, see [`auto-targeting/docs/SDD-SPEC.md`](auto-targeting/docs/SDD-SPEC.md).

---

## Contributing

Contributions are welcome — please read [`CONTRIBUTING.md`](CONTRIBUTING.md)
first. In short: branch off `main`, use [Conventional Commits](https://www.conventionalcommits.org/),
keep `cargo clippy`/`cargo fmt` clean, and follow the PR checklist.

Because this software can command a flying vehicle, changes to anything
safety-critical (`commander/`, `fc-adapter/`, `target-tracker/`, watchdog
config) require extra review — see [`auto-targeting/docs/SAFETY.md`](auto-targeting/docs/SAFETY.md).

---

## License

Dual-licensed under **MIT OR Apache-2.0**, at your option. This matches the
Rust ecosystem convention. See [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE).

> The prebuilt `librknnrt.so` (Rockchip RKNN runtime) used on-device is
> **not** covered by this license and is not redistributed in this repository.
> Obtain it from the [rknn-toolkit2](https://github.com/airockchip/rknn-toolkit2)
> releases under its own terms.

---

## Acknowledgements

- [Ultralytics YOLOv8](https://github.com/ultralytics/ultralytics) — detection model and export pipeline
- [Rockchip rknn-toolkit2](https://github.com/airockchip/rknn-toolkit2) — NPU runtime
- [Orange Pi 5](http://www.orangepi.org/) — reference hardware platform
- [ArduPilot](https://ardupilot.org/) — flight-controller firmware + SITL
