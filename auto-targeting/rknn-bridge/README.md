# rknn-bridge

C++ microservice for NPU-based inference on the Orange Pi 5 (RK3588S).

## Status

- ✅ Compiles without NPU (stub backend returns fake detections)
- ✅ Compiles with RKNN SDK (real backend, when `librknnrt.so` is available)
- ✅ Unit tests for NMS (6 tests)
- ✅ Unix socket server with length-prefixed JSON protocol
- 🚧 Shared memory (memfd + SCM_RIGHTS) for frame data — stub implementation
- 🚧 Real RKNN output parsing (YOLOv8 output tensor layout)

## Architecture

```
┌─────────────────────┐    Unix socket      ┌──────────────────┐
│  cv-inference       │ ──────────────────► │  rknn-bridge     │
│  (Rust)             │    JSON envelope    │  (C++)           │
│                     │ ◄────────────────── │  librknnrt.so    │
│  bridge_client.rs   │   JSON response     │                  │
└─────────────────────┘                     └──────────────────┘
                                            Frame data via SHM (TODO)
```

See `docs/ADR/0001-rknn-cpp-bridge.md` for the full protocol specification.

## Building

### Without NPU (stub backend)

```bash
cd rknn-bridge
mkdir build && cd build
cmake ..
make
```

This produces a `rknn-bridge` binary that uses a stub backend (returns fake
detections). Useful for development on x86 machines.

### With RKNN SDK (real NPU)

```bash
# Download the RKNPU2 SDK
git clone https://github.com/airockchip/rknn-toolkit2 /opt/rknn-toolkit2

cd rknn-bridge
mkdir build && cd build
cmake -DRKNN_SDK_PATH=/opt/rknn-toolkit2 ..
make
```

This produces a `rknn-bridge` binary that uses the real RKNN backend.

### With tests

```bash
cmake -DBUILD_TESTS=ON ..
make
ctest
```

## Running

```bash
# Start the bridge (listens on Unix socket)
./rknn-bridge --socket /tmp/rknn-bridge.sock --model /path/to/model.rknn

# The Rust orchestrator (cv-inference crate) connects to this socket.
```

## Protocol

Messages are length-prefixed JSON (4-byte big-endian length + JSON payload).

See `include/protocol.h` for the message type definitions.

### Message types

| Type | Direction | Description |
|---|---|---|
| `init` | Rust → Bridge | Load model, configure thresholds |
| `init_ack` | Bridge → Rust | Confirm model loaded (or error) |
| `infer` | Rust → Bridge | Run inference on a frame (via SHM) |
| `infer_ack` | Bridge → Rust | Return detections + latency |
| `health` | Rust → Bridge | Health check (NPU utilization, etc.) |
| `health_ack` | Bridge → Rust | Health status |
| `shutdown` | Rust → Bridge | Graceful shutdown |
| `shutdown_ack` | Bridge → Rust | Confirm shutdown |

## Files

| File | Purpose |
|---|---|
| `CMakeLists.txt` | Build configuration |
| `include/protocol.h` | IPC protocol definitions (shared with Rust) |
| `include/rknn_model.h` | InferenceBackend abstract interface |
| `include/nms.h` | Non-Maximum Suppression |
| `include/shm_server.h` | Unix socket server with SHM support |
| `src/rknn_model.cpp` | StubBackend + RknnBackend (conditional on HAVE_RKNN) |
| `src/nms.cpp` | NMS implementation |
| `src/shm_server.cpp` | Unix socket server implementation |
| `src/bridge_main.cpp` | Main IPC loop + JSON (de)serialization |
| `src/main.cpp` | Entry point (arg parsing, calls run_bridge) |
| `tests/test_nms.cpp` | NMS unit tests |

## Deployment

On Orange Pi 5, the bridge runs as a systemd service:

```bash
sudo cp rknn-bridge /opt/auto-targeting/bin/
sudo cp deploy/systemd/rknn-bridge.service /etc/systemd/system/
sudo systemctl enable --now rknn-bridge
```

See `deploy/systemd/rknn-bridge.service` for the unit file.

## Limitations

- **Stub backend only returns fake detections.** Real inference requires
  the RKNN SDK and proper YOLOv8 output parsing (TODO).
- **Frame data via SHM is a stub.** The `receive_frame()` method returns
  false. Real implementation needs `recvmsg()` with `SCM_RIGHTS` for fd
  passing.
- **JSON parsing is hand-rolled.** Switch to `nlohmann/json` if the protocol
  grows.
