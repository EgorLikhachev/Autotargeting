# ADR-0001: RKNN Inference via C++ Bridge Microservice

- **Status:** Accepted
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

## RKNN Bridge Protocol Specification

### Transport

- **Control channel:** Unix domain socket at `/tmp/rknn-bridge.sock`
  (configurable via `[inference] bridge_socket`).
- **Frame data channel:** shared memory via `memfd_create()` with a name
  derived from the frame sequence number. The Rust side creates the memfd,
  writes the frame, sends the fd via SCM_RIGHTS over the Unix socket.
- **Response:** the bridge writes detections back to the Unix socket as a
  length-prefixed JSON message.

### Message types

#### 1. Init (Rust → Bridge)

Sent once at startup to load the model.

```json
{
  "type": "init",
  "model_path": "/opt/auto-targeting/models/yolov8n_int8.rknn",
  "input_width": 1280,
  "input_height": 720,
  "input_format": "nv12",
  "confidence_threshold": 0.45,
  "nms_threshold": 0.45
}
```

Response:
```json
{"type":"init_ack","ok":true,"output_classes":80,"backend":"rknn"}
```
or
```json
{"type":"init_ack","ok":false,"error":"model not found"}
```

#### 2. Infer (Rust → Bridge)

Sent for each frame. Frame data is passed via shared memory (the `memfd`
fd is sent as ancillary data via `SCM_RIGHTS`).

```json
{
  "type": "infer",
  "frame_seq": 12345,
  "shm_fd": "<passed via SCM_RIGHTS>",
  "shm_size": 1382400,
  "captured_at_ms": 1698230400000
}
```

Response (detections as JSON array):
```json
{
  "type": "infer_ack",
  "frame_seq": 12345,
  "latency_ms": 42,
  "detections": [
    {
      "bbox": {"x": 100, "y": 200, "width": 60, "height": 80},
      "class": "person",
      "class_id": 0,
      "confidence": 0.92
    }
  ]
}
```

#### 3. Health Check (Rust → Bridge)

Sent periodically (every 5 s) and on reconnect.

```json
{"type":"health"}
```

Response:
```json
{"type":"health_ack","ok":true,"npu_utilization":0.65,"model_loaded":true}
```

#### 4. Shutdown (Rust → Bridge)

Sent on graceful shutdown.

```json
{"type":"shutdown"}
```

### Error handling

- If the bridge doesn't respond within `inference_loop_wdt_ms` (200 ms), the
  Rust side considers the inference failed and the watchdog fires.
- On bridge crash, the Rust side attempts reconnect every 1 s for 30 s,
  then gives up and switches to `CpuInferenceBackend` if
  `allow_cpu_fallback = true`.

### Performance budget

| Operation | Budget | Notes |
|---|---|---|
| Frame copy to SHM | < 2 ms | Use `memcpy` on mmap'd memfd |
| Unix socket send | < 1 ms | Small control message |
| RKNN inference | 30–50 ms | NPU INT8 YOLOv8n on 720p |
| NMS (in C++) | < 5 ms | Greedy NMS, ~10 detections |
| Response parse | < 1 ms | JSON, ~1 KB typical |
| **Total** | **< 60 ms** | Meets KPI |

### Why not Cap'n Proto / FlatBuffers?

JSON is sufficient for the response (small array of detections). The frame
data goes through SHM, not the socket, so serialization overhead is minimal.
If profiling shows JSON parsing as a bottleneck, we can switch to Cap'n Proto
without changing the SHM layer.

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
- Deployment: both binaries run as separate systemd units
  (`auto-targeting.service` and `rknn-bridge.service`).
- The `rknn-bridge.service` is a dependency of `auto-targeting.service`
  (`After=rknn-bridge.service` in the systemd unit).
