# Auto-Targeting System

Companion computer for autonomous target tracking on a fixed-wing UAV.

## Status

🚧 **Phase 0: Foundation & Scaffolding** — in progress.

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
# CLI smoke test (mock FC, no hardware required)
cargo run -p auto-targeting-cli -- --mock-fc --mock-video demo
```

## Repository layout

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §5 for the full tree.
