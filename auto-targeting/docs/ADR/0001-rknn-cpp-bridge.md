# ADR-0001: RKNN Inference via C++ Bridge Microservice

- **Status:** Proposed
- **Date:** 2026-08-01
- **Decision makers:** TBD
- **Related hypotheses:** H-001

## Context

The Orange Pi 5's RK3588S SoC includes a 6 TOPS NPU (Rockchip RKNPU2). To
achieve our inference latency KPI (< 60 ms for YOLOv8n on 720p), we MUST use
the NPU rather than CPU inference. ONNX Runtime on CPU would be 5–10× slower.

The RKNPU2 SDK is provided by Rockchip as a C/C++ library (`librknnrt.so`)
with Python bindings (`rknn-toolkit2`). There is no official Rust binding.

Hypothesis H-001 investigates whether mature third-party Rust bindings exist.
Per devops advice (advice #1), this is unlikely.

## Decision

Implement inference in a separate C++ microservice (`rknn-bridge/`). The
Rust orchestrator (`cv-inference` crate) communicates with the bridge over
a Unix domain socket, with frame data passed via shared memory (memfd) to
avoid copies.

```
┌─────────────────────┐    SHM (memfd)      ┌──────────────────┐
│  cv-inference       │ ──────────────────► │  rknn-bridge     │
│  (Rust)             │    Unix socket      │  (C++)           │
│                     │ ◄────────────────── │  librknnrt.so    │
│  bridge_client.rs   │   Vec<Detection>    │                  │
└─────────────────────┘                     └──────────────────┘
```

## Consequences

**Positive:**
- Use the official Rockchip SDK directly — no dependency on potentially
  unmaintained Rust bindings.
- C++ side can be updated independently of Rust releases.
- If Rust bindings mature later, we can swap the bridge client for a native
  Rust implementation without touching the rest of the system (the
  `InferenceBackend` trait abstracts this).

**Negative:**
- Two build systems (Cargo + CMake) — increases CI complexity.
- Two binaries to deploy — Ansible playbook must handle both.
- Cross-compilation gets harder (need both aarch64 Rust toolchain AND
  aarch64 C++ toolchain in the Docker image).
- IPC overhead (Unix socket + SHM) adds ~1–2 ms latency vs in-process call.

**Neutral:**
- The C++ microservice is small (~500 LOC) and does one thing.

## Alternatives considered

1. **Pure Rust with `rknn-rs` or similar crate.** Rejected pending H-001
   verification. If H-001 is CONFIRMED, this ADR will be superseded.
2. **Python inference service.** Rejected — Python startup time and GIL
   make it unsuitable for a 15+ FPS inference loop.
3. **CPU-only ONNX Runtime.** Rejected for production — too slow. Kept as
   a fallback for dev on x86 machines (`allow_cpu_fallback` config option).

## Implementation notes

- C++ bridge: `rknn-bridge/` directory, built with CMake.
- Rust client: `crates/cv-inference/src/backend.rs` → `RknnBridgeClient`.
- Protocol: length-prefixed JSON for control messages, raw SHM for frame
  data. (May switch to Cap'n Proto / FlatBuffers in Phase 2 if JSON parsing
  shows up in profiles.)
- Deployment: both binaries run as separate systemd units
  (`auto-targeting.service` and `rknn-bridge.service`).
