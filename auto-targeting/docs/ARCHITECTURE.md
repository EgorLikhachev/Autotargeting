# Architecture

> **Status:** Living document. Updated as ADRs are accepted.
> **Source of truth:** the code. This document explains the *why*; the code is the *what*.

This document is the developer-facing companion to the high-level Roadmap
(`../download/AUTO_TARGETING_ROADMAP.md`). It records the architectural
decisions as they are realized in code, with references to the ADRs and
hypotheses that justify them.

## Module overview

```
┌─────────────────┐    Frame (NV12)    ┌──────────────────┐
│  video-capture  │ ─────────────────► │  cv-inference    │
│  (V4L2 + MJPEG) │   shared mem       │  (Rust + C++     │
└─────────────────┘                    │   RKNN bridge)   │
                                       └────────┬─────────┘
                                                │ Vec<Detection>
                                                ▼
┌─────────────────┐  TargetState      ┌──────────────────┐
│   commander     │ ◄───────────────  │ target-tracker   │
│ (state machine, │                   │ (Kalman + SORT)  │
│  watchdogs)     │                   └──────────────────┘
└────────┬────────┘
         │ Commands (ROI, position target)
         ▼
┌─────────────────┐   MAVLink     ┌──────────────────┐
│  fc-adapter     │ ────────────► │  ArduPilot FC    │
│ (HAL trait)     │ ◄──────────── │ (SpeedyBee F405) │
└─────────────────┘  Telemetry    └──────────────────┘
```

| Crate | Role | Status |
|---|---|---|
| `common` | Shared types, errors, config | ✅ Phase 0 |
| `video-capture` | V4L2 capture, MJPEG decode | 🚧 skeleton |
| `cv-inference` | Rust orchestrator for RKNN bridge | 🚧 skeleton |
| `target-tracker` | Kalman filter + multi-target tracker | 🚧 skeleton |
| `fc-adapter` | `FlightControllerAdapter` trait + impls | ✅ Mock impl |
| `commander` | State machine + watchdogs + anti-loop | ✅ Phase 0 |
| `cli` | Binary entry point + operator CLI | ✅ Phase 0 (mock mode) |

## FC abstraction (HAL)

The `FlightControllerAdapter` trait (`crates/fc-adapter/src/traits.rs`) is the
single abstraction that decouples the system from a specific FC. The commander
works with `Box<dyn FlightControllerAdapter>` and never sees MAVLink, UART, or
any specific FC implementation.

Implementations:

| Implementation | Transport | Status |
|---|---|---|
| `ArduPilotMavlinkAdapter` | MAVLink v2 over UART/USB | 🚧 Phase 4 |
| `SittlMavlinkAdapter` | MAVLink v2 over UDP | 🚧 Phase 4 |
| `MockFcAdapter` | In-memory | ✅ Working |

See ADR-0001 (pending) for the C++ RKNN bridge decision, and HYPOTHESES.md
H-002 for the MAVLink 10 Hz streaming assumption.

## Anti-loop protection

Six layers, see `crates/commander/src/`:

| Layer | Where | Status |
|---|---|---|
| 1. Per-loop watchdog timers | `watchdogs.rs` | ✅ Working |
| 2. State machine with deterministic transitions | `state_machine.rs` | ✅ Working |
| 3. Deadband + hysteresis + bounding limits | `anti_loop.rs` | ✅ Working |
| 4. Rate limiter | `crates/fc-adapter/src/rate_limiter.rs` | ✅ Working |
| 5. Oscillation detector | `anti_loop.rs` | ✅ Working |
| 6. Safety pilot RC override | ArduPilot config | 🚧 Phase 7 |
| 7. systemd `WatchdogSec` | `deploy/systemd/auto-targeting.service` | 🚧 Phase 0.9 |

## Configuration

TOML-based, with environment variable overrides (prefix `AT_`, sections
separated by `__`). See `config.example.toml` for a full template.

```bash
# Override video device via env var
AT_VIDEO__DEVICE=/dev/video1 ./auto-targeting --config config.toml
```

## Building

```bash
# Dev (host)
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check

# Cross-compile for Orange Pi 5 (aarch64)
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

## Running

```bash
# Phase 0 smoke test (mock everything, no hardware)
cargo run -p auto-targeting-cli -- --mock-all

# Health check (for systemd / healthcheck scripts)
cargo run -p auto-targeting-cli -- --health-check
```
