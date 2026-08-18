# Changelog

All notable changes to the **Auto-Targeting System** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(with pre-release phase tags, e.g. `0.1.0-phase-1.1`, while the FC integration
is incomplete).

---

## [Unreleased]

### Added
- **`video-recorder` crate (TG26-125)**: first real consumer of the SHM
  frame ring — ffmpeg subprocess (rawvideo pipe -> libx264/yuv420p MP4),
  burned-in OSD (ISO timestamp, frame id, geometry via new
  `cv_visualizer::draw_osd`), sequential/latest read modes with
  catch-up jumps. Guard discipline: the FrameGuard is dropped before any
  heavy work, so a blocked encoder pipe can never freeze a ring slot.
  Hardware-validated on RK3588: parallel consumer VERIFIED=237/TORN=0
  during recording; MP4 h264 640x480 353 frames 11.77s (ffprobe).
  New pub `video_capture::nv12_to_rgb24` (integer BT.601).
- **Performance audit & optimization pass** (docs/PERF_AUDIT_2026-08.md,
  A/B on RK3588): rgb24_to_nv12 ×9.1, yuyv_to_rgb24 ×7.9, yuyv_to_nv12
  ×2.4, letterbox ×11.3, rgb_to_nchw ×1.6, postprocess ×1.26 — total CPU
  stages of the RGB path ×2.9 (9.87 → 3.44 ms/frame @640x480). Integer
  BT.601 fixed-point (+/-1 vs f32 reference, consistency test added),
  chunk iterators, transposed class scan, letterbox LUT, branch-free
  base64, per-frame alloc trims. Criterion baseline benches for
  convert/yolov8 hot paths.
- **`shmem-buffer` crate (TG26-160)**: SPMC ring buffer for video frames in
  shared memory — memfd + linkat(/dev/shm) + mmap on Linux, in-process arena
  anywhere else; NV12 (default) / RGB24 storage; lock-free single-word
  CAS protocol per slot; `FrameGuard` RAII (no overwrite while held);
  drop-new policy on full buffer (never blocks the producer); stale-slot
  reaper for crashed consumers. 15 unit + 7 acceptance tests, criterion
  bench, producer/consumer/reaper examples. ADR D-013,
  docs/DEV_NOTES/shmem_ring_buffer.md (incl. incident post-mortem).
- **Alternative camera validated: Sony PlayStation Eye** (OV534+OV7721,
  non-UVC, YUYV-only) — out-of-tree `gspca_ov534` kernel module built and
  installed on the stand; formats up to 640×480@60 and 320×240@187 confirmed;
  live demo end-to-end (84 frames, 80 906 detections, `processed.mp4`).
  See `auto-targeting/docs/CAMERA_PS_EYE_TEST.md`.
- `--format mjpeg|yuyv` and `--backend v4l|direct` flags in `camera_latency`
  and `live_camera_demo` examples; new `v4l2-direct-cam` feature of
  `cv-inference`.
- **`V4l2DirectSource`** — direct V4L2 capture via raw `libc` ioctl
  (`v4l2-direct` feature), bypassing the `v4l` crate abstraction that was the
  capture bottleneck. (`feat(video-capture): direct V4L2 capture via libc ioctl`)
- `direct_capture_bench` example for A/B comparison of the two capture backends.
- `camera_latency` benchmark example with a `--pipeline` flag for sequential vs.
  pipelined capture/decode comparison.
- `live_camera_demo` example — the full camera → NPU → detections → annotated
  JPEG/MP4 pipeline, end-to-end on RK3588.
- Hardware test results recorded: camera latency benchmarks (H2) and the live
  demo run (H3). See
  [`auto-targeting/docs/HARDWARE_TEST_RESULTS.md`](auto-targeting/docs/HARDWARE_TEST_RESULTS.md) §2.7a and §8.
- Repository health docs: top-level `README.md`, `CONTRIBUTING.md`,
  `CODE_OF_CONDUCT.md`, `SECURITY.md`, `SUPPORT.md`, `.editorconfig`, dual
  `LICENSE-MIT` / `LICENSE-APACHE`, GitHub issue templates, and a docs CI
  workflow (markdownlint + link checker).

### Changed
- **Drop-old capture policy** in `V4l2Source`: `tx.try_send` replaces
  `tx.blocking_send`, so the capture thread is never blocked by a slow consumer.
  (`perf(video-capture): drop-old capture policy`)
- `SyntheticVideoSource` channel depth fixed from `max_frames.max(1)` (= 1 for
  infinite sources) to `fps.clamp(3, 30)`, removing a back-pressure deadlock.
- Rewrote [`auto-targeting/docs/PROJECT_REPORT.md`](auto-targeting/docs/PROJECT_REPORT.md)
  with the validated hardware results (was a pre-hardware draft).

### Performance
- V4L2 capture throughput raised from **21 FPS** (`v4l` crate) to **32 FPS**
  (direct ioctl) on Orange Pi 5 + Arducam OV9782.
- Camera pipeline characterized end-to-end: capture p50 = 23 ms, MJPG decode =
  9 ms, NPU inference = 29 ms (sequential total 61 ms; pipeline target ~29 ms).

### Fixed
- `v4l2_direct.rs`: undefined `data_len` in the dequeue path (build error when
  `v4l2` + `v4l2-direct` features combine) — use `bytesused` clamped to the
  mapped buffer length.
- `v4l2_direct.rs`: `VIDIOC_S_PARM timeperframe` offsets were 4 bytes early —
  the driver read a garbage interval and silently kept the camera at its
  default 30 fps regardless of the requested rate.
- `convert.rs`: `yuyv_to_rgb24`/`yuyv_to_nv12` read out of bounds on the last
  pixel pair of a frame (panic on the first real PS Eye frame). Now processed
  pairwise per [Y0,U,Y1,V]; regression tests added.
- `live_camera_demo`: the capture pump loop broke on channel **Full** (not
  just Closed), killing capture after exactly 5 frames whenever inference
  lagged the camera — this was the long-standing "v4l crate early-terminate"
  mystery from the Arducam run. Now breaks only on Closed.
- `v4l2_buffer` struct layout: kernel `timeval` is 12 bytes (not 16 like
  glibc), which shifted `m.offset`/`length`. Switched to raw `[u8; 88]` byte
  buffers with offsets verified via C `offsetof()`.
- `V4l2Format` union 8-byte alignment padding (kernel `v4l2_window` has a
  pointer field).
- `live_camera_demo` feature-gate conflict (`cfg(unix)` + `v4l2-cam` propagation).

---

## [0.1.0-phase-1.1] — 2026-08-10

**Phase 1.1 — minimal CV loop (camera → model → detections), hardware-validated
on RK3588.** End-to-end detections through `V4L2 → JPEG-decode → letterbox →
rknn-bridge → NPU zero-copy → YOLOv8 postprocess`, with NPU inference at
~29 ms/frame.

### Added
- **Rust workspace (10 crates)**: `common`, `video-capture`, `cv-inference`,
  `yolov8`, `cv-visualizer`, `system-telemetry`, `target-tracker`,
  `fc-adapter`, `commander`, `cli` — **294 unit tests**.
- **C++ `rknn-bridge` microservice**: Unix-socket server, JSON protocol with
  big-endian length-prefix, **zero-copy NPU IO** via `rknn_set_io_mem` +
  `rknn_create_mem`, output attr `RKNN_TENSOR_FLOAT32`, input
  `RKNN_TENSOR_NHWC`, `rknn_set_core_mask(NPU_CORE_0)`. **6 C++ NMS tests**
  pass on the NPU.
- `yolov8` crate — pure-Rust letterbox + postprocess (NMS, confidence filter).
- ONNX Runtime backend (`cpu-onnx` feature) for x86 development.
- `cv-visualizer` — headless annotation (bboxes + labels → JPEG + JSONL).
- `system-telemetry` — RSS, CPU/NPU temperature, latency p50/p95.
- Interactive REPL CLI + TOML config (`figment`, `AT_` env overrides).
- **Anti-loop protection**: state machine, watchdogs, deadband, rate limiter,
  oscillation detector.
- `MockFcAdapter` (in-memory) + `SittlMavlinkAdapter` (UDP MAVLink).
- [`auto-targeting/docs/SDD-SPEC.md`](auto-targeting/docs/SDD-SPEC.md) —
  919-line, 15-section specification; ADRs D-001..D-011; machine-readable
  progress tracker; `ai-context/` for agent handoff.
- CI (`ci.yml`) + Nightly (`nightly.yml`: full ArduPilot SITL + coverage) at
  repo root with `working-directory: auto-targeting`.
- ArduPilot SITL `docker-compose` + replay scenarios; systemd units +
  healthcheck script.

### Performance (measured on Orange Pi 5, RK3588)
- NPU inference latency: **27–29 ms** (KPI target < 60 ms ✅).
- Sustained NPU throughput: **~34 FPS** (KPI target ≥ 15 ✅).
- `rknn-bridge` RSS idle: **5.7 MB** (KPI target < 50 MB ✅).
- Temperatures under load: CPU 45.3 °C, NPU 44.4 °C (KPI targets 70 / 85 °C ✅).

### Fixed
- Endianness mismatch between C++ (native `uint32`) and Rust (`to_be_bytes`):
  canonical big-endian length-prefix via `htonl`/`ntohl`.
- `RKNN_TENSOR_FORMAT_RGB` undeclared in RKNN SDK 2.x → renamed to
  `RKNN_TENSOR_NHWC` (ADR D-007).
- `rknn_outputs_get` returning `size = 0` for float16 models → switched to
  zero-copy `rknn_set_io_mem` with `output_attr_.type = FLOAT32`.
- Missing sigmoid on class scores (RKNN export emits raw logits, unlike ONNX).
- `extract_frame_data_b64` was an empty-buffer stub → caused a segfault;
  implemented base64 decode.
- NPU core binding via `rknn_set_core_mask` after `rknn_init`.

### Known limitations
- `cpu-onnx` does not build on the device (prebuilt `ort` needs GCC13; the
  device ships GCC12). ONNX inference is x86-dev-only; on-device path is RKNN.
- The `v4l` Rust crate caps capture at ~21 FPS (5× slower than `v4l2-ctl`);
  the direct-ioctl `V4l2DirectSource` (in [Unreleased]) supersedes it.

---

[Unreleased]: https://github.com/EgorLikhachev/Autotargeting/compare/v0.1.0-phase-1.1...main
[0.1.0-phase-1.1]: https://github.com/EgorLikhachev/Autotargeting/releases/tag/v0.1.0-phase-1.1
