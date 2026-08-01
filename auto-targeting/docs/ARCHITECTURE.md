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
│  (V4L2 + MJPEG  │   shared mem       │  (Rust + C++     │
│   or synthetic) │                    │   RKNN bridge)   │
└─────────────────┘                    └────────┬─────────┘
                                                │ Vec<Detection>
                                                ▼
┌─────────────────┐  TargetState      ┌──────────────────┐
│   commander     │ ◄───────────────  │ target-tracker   │
│ (state machine, │                   │ (Kalman + IoU    │
│  watchdogs)     │                   │  matching)       │
└────────┬────────┘                   └──────────────────┘
         │ Commands (ROI, position target)
         ▼
┌─────────────────┐   MAVLink     ┌──────────────────┐
│  fc-adapter     │ ────────────► │  ArduPilot FC    │
│ (HAL trait)     │ ◄──────────── │ (SpeedyBee F405  │
└─────────────────┘  Telemetry    │  or SITL)        │
                                  └──────────────────┘
```

| Crate | Role | Status |
|---|---|---|
| `common` | Shared types, errors, config | ✅ Complete |
| `video-capture` | V4L2 + synthetic + replay sources | ✅ Synthetic + Replay; V4L2 stub |
| `cv-inference` | Rust orchestrator for RKNN bridge | 🚧 Stub + Mock + NMS |
| `target-tracker` | Kalman filter + IoU tracker | ✅ Single-target working |
| `fc-adapter` | `FlightControllerAdapter` trait + impls | ✅ Mock + SITL; ArduPilot stub |
| `commander` | State machine + watchdogs + anti-loop | ✅ Phase 0 complete |
| `cli` | Binary + interactive REPL | ✅ REPL working |

## FC abstraction (HAL)

The `FlightControllerAdapter` trait (`crates/fc-adapter/src/traits.rs`) is the
single abstraction that decouples the system from a specific FC. The commander
works with `Box<dyn FlightControllerAdapter>` and never sees MAVLink, UART, or
any specific FC implementation.

Implementations:

| Implementation | Transport | Status |
|---|---|---|
| `ArduPilotMavlinkAdapter` | MAVLink v2 over UART/USB | 🚧 Phase 4 (stub falls back to Mock) |
| `SittlMavlinkAdapter` | MAVLink v2 over UDP | ✅ Working (9 tests) |
| `MockFcAdapter` | In-memory | ✅ Working (10 tests) |

Factory function: `fc_adapter::build_adapter(&config.fc)` picks the right
adapter based on `config.fc.adapter` string (`"mock"`, `"sitl-mavlink"`,
`"ardupilot-mavlink"`).

See ADR-0001 for the C++ RKNN bridge decision, and HYPOTHESES.md H-002 for
the MAVLink 10 Hz streaming assumption.

## Anti-loop protection

Seven layers, see `crates/commander/src/` and `deploy/systemd/`:

| Layer | Where | Status |
|---|---|---|
| 1. Per-loop watchdog timers | `watchdogs.rs` | ✅ Working (5 watchdogs) |
| 2. State machine with deterministic transitions | `state_machine.rs` | ✅ Working (9 states) |
| 3. Deadband + hysteresis + bounding limits | `anti_loop.rs` | ✅ Working |
| 4. Rate limiter | `crates/fc-adapter/src/rate_limiter.rs` | ✅ Working (10 Hz) |
| 5. Oscillation detector | `anti_loop.rs` | ✅ Working |
| 6. Safety pilot RC override | ArduPilot config | 🚧 Phase 7 |
| 7. systemd `WatchdogSec` | `deploy/systemd/auto-targeting.service` | ✅ Configured (10 s) |

## CLI modes

```bash
# Interactive REPL (operator console) — Phase 5.6 ✅
cargo run -p auto-targeting-cli -- --repl

# Smoke test with all mocks — Phase 0 ✅
cargo run -p auto-targeting-cli -- --mock-all

# Health check (for systemd) — Phase 0 ✅
cargo run -p auto-targeting-cli -- --health-check

# Full production mode — Phase 5 🚧
cargo run -p auto-targeting-cli -- --config /etc/auto-targeting/config.toml
```

### REPL commands

| Command | Description |
|---|---|
| `help` | List all commands |
| `status` | Show state machine, FC, watchdogs |
| `arm` / `disarm` | Arm/disarm the drone |
| `set-mode <mode>` | Change FC mode (guided, rtl, loiter, manual, auto, stabilize) |
| `scan` | Start scanning for targets |
| `select-target <id>` | Select a target, transition to TRACKING |
| `abort` | ABORT (force transition + RTL) |
| `reset` | Return to IDLE (after ABORT + disarm) |
| `watchdogs` | Show watchdog statuses |
| `anti-loop` | Show anti-loop guard stats |
| `feed-watchdog <name>` | Manually feed a watchdog |
| `simulate-heartbeat-loss` | Test: simulate FC heartbeat loss |
| `simulate-attitude <r p y>` | Test: inject attitude update |
| `quit` | Exit |

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

## Testing

```bash
# All tests
cargo test --workspace

# Vivid-gated tests (requires `sudo modprobe vivid`)
sudo modprobe vivid
cargo test -p video-capture -- --include-ignored vivid

# REPL smoke test
echo -e "help\nstatus\narm\nabort\nquit" | cargo run -p auto-targeting-cli -- --repl
```

## ADRs (Architecture Decision Records)

| ADR | Title | Status |
|---|---|---|
| 0001 | RKNN Inference via C++ Bridge Microservice | Accepted |
| 0002 | Tracking Algorithm — IoU + Kalman | Accepted |
| TEMPLATE | Template for new ADRs | — |

See `docs/ADR/` for full text.

## Hypotheses

See `docs/HYPOTHESES.md` for the full log. Critical hypotheses that block
Flight Readiness:

- H-001: Mature Rust bindings for RKNPU2 SDK (CRITICAL)
- H-002: ArduPilot handles 10 Hz MAVLink streaming (CRITICAL)
- H-003: Arducam UC-852 V4L2 + MJPEG support (HIGH)
- H-004: `mavlink` Rust crate stability (HIGH) — partially confirmed in Phase 4.8
