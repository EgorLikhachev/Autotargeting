# Changelog

All notable changes to the **Auto-Targeting System** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(with pre-release phase tags, e.g. `0.1.0-phase-1.1`, while the FC integration
is incomplete).

---

## [Unreleased]

### Added
- **Stage 2 roadmap** (docs/STAGE2_ROADMAP.md): the complete plan for the
  stand-to-aircraft transition — Phase 1.2 (dataset protocol for the four
  stand classes + fine-tune pipeline with KPI gate mAP>=0.85), Phase 7
  HITL bench build in 9 verified steps, FC selection matrix (Matek
  H743-Wing recommended over Pixhawk 6C / SpeedyBee), wiring to the pin
  (CP2102 USB-TTL, TX/RX cross, common GND), power (dedicated 5V/4A BEC,
  ~3A OPi5 peak), cooling bill of materials (heatsink+fan, <=80C gate),
  RC/GCS recommendation (RadioMaster ELRS + Mission Planner), and a
  split checklist: user purchases/actions (~$158 bench BOM) vs software
  work (labels-from-config, train/eval scripts, calibration dataset for
  int8, HITL guides, flight-readiness autocheck).

### Changed
- **Dependency & toolchain refresh (2026-09-01)**: lockfile updated (77
  patch/minor bumps incl. tokio/serde/clap/futures trees); major bumps —
  thiserror 1→2 (drop-in), axum 0.7→0.8 (health endpoint), sd-notify
  0.4→0.5; **MSRV 1.75 → 1.85** (rust-version + clippy msrv + docs) —
  edition-2024-capable floor; deferred majors documented (toml 1.x via
  figment, criterion 0.8, reqwest 0.13, rustyline 18, base64 0.23 —
  isolated/transitive, risk > value this round). En route: e2e test
  global-TargetId flake fixed (id compared to acquire() result, not
  absolute 1); cargo fmt over workspace (newline-style drift).
  Full workspace: 16 lib suites green, integration green, clippy clean
  (lib+tests scope), fmt clean.

### Added
- **Stage 8: systemd composition + 30-min soak (closes run_full)** — 8
  services (bus anchor/config-svc, rknn-bridge, camera, detector, tracker,
  fc-bridge, commander-bus, recorder; shared autotarget.env,
  Restart=always, new commander_bus service bin). Soak on RK3588: 30/30
  snapshots all-active, zero restarts, RSS +5.4MB/30min (~7MB/h, 7x under
  the 8h KPI), detector stable 11-12 FPS, recorder 238MB artifact. First
  sustained NPU thermal measurement: 83-87C — heatsink/airflow flagged
  for flight tests. En-route fixes: camera --seconds 0 = infinite +
  stale-segment self-heal; bus-mon/repl-bus connect mode (anchor is
  config-svc).
- **Stage 7 safety fixes**: watchdog Abort hook in the commander bus loop
  (Abort actions now trigger RTL+reset; the loop feeds its own
  CommandLoop/VideoLoop watchdogs); KNOWN_ISSUES #3 closed — crude
  offset->east/down replaced with the real CameraToAngular transform
  (pixels -> FOV angle -> NED yaw with drone attitude). SITL: telemetry
  in commander window 245 (was 0), 136 corrections.
- **M5: operator console on the bus** — cli::bus_console + subcommands:
  bus-mon (at/** monitor), repl-bus (FC commands via the bus + live
  tracks/telemetry/statuses), config-svc/config-get (pub/sub-ack config
  protocol; zenoh-query unreliable in scouting-off peer topology —
  documented). [bus] config section (AT_BUS__ENDPOINT). 2/2 bus
  integration tests + 20 lib.
- **M6: SHM frame path to rknn-bridge (D-016)** — named /dev/shm segment
  (double-buffered) replaces the base64 round-trip: init passes the segment
  path/size, infer sends only a buffer index (~1.6MB off the wire + no
  encode/decode). Transport ceiling eliminated — infer p50 29.5 ms equals
  pure NPU time; detector FPS 27.2 (was 9.9; at conf 0.45, 12.5 — remainder
  is the over-detect JSON response, a Phase 1.2 model issue). Base64 kept as
  automatic fallback. Fixed en route: latent letterbox-contract bug (C++
  copied init-dims bytes from the 640x640 letterboxed frame, cutting the
  image bottom — affected both paths since TG26-35) and issue #12 (bridge
  hung forever after client disconnect: close+re-accept, SO_RCVTIMEO,
  length sanitize). New C++<->Rust round-trip integration test (green on
  RK3588) closes the long-standing SDD §15 test gap.
- **Commander on the bus (M4)**: commander::bus_runner — closed control
  loop: at/tracks + at/telemetry subscriptions feed the existing Commander
  (state machine, watchdogs, anti-loop, rate limiter — safety logic
  unchanged); first track becomes active target; bbox-center offset ->
  corrections via the owned FlightControllerAdapter; status at
  at/status/commander. Closed-loop integration test on a real zenoh bus
  (mock FC + fc-bridge telemetry): corrections flow for offset targets,
  deadband suppresses centered ones. 115 unit + 3 bus tests green;
  commander now builds clippy-clean on non-unix hosts (sd-notify gated).
- **`fc-bridge` crate (M3)**: FC ↔ bus bridge over the
  FlightControllerAdapter trait — telemetry to at/telemetry at configured
  Hz (GPS/mode included), FC edge events (link/armed/mode) to
  at/fc_events with dedup, commands from at/commands (set_roi,
  set_pos_ned, set_mode, arm/disarm) dispatched to any adapter (mock |
  sitl | ardupilot), status at/status/fc. Hardware-run on RK3588 (mock):
  9.9 Hz telemetry + events observed via bus_dump; tests 3/3 on x86+ARM.
- **`tracker-crate` (M2, bus migration)**: tracker component — first bus
  CONSUMER: subscribes at/detections, runs the existing Kalman+Hungarian
  MultiTargetTracker, publishes TrackMsg per active track to at/tracks +
  at/status/tracker. Live contour on RK3588 (camera→detector(NPU)→tracker,
  observed via bus_dump); integration tests 2/2 on x86 and ARM (moving
  target → one stable track; two targets → two tracks).
- **Bus migration M0+M1** (BUS_MIGRATION_PLAN.md): M0 contracts —
  CommandMsg/TrackMsg/FcEvent, TelemetrySample GPS/battery/mode extensions
  (serde-default legacy-compatible), CONTRACT_VERSION, commands publisher
  with CongestionControl::Block; M1 — bus_dump observer (at/**) and
  component statuses on the stand: camera_publisher --bus →
  at/status/camera (fps_actual ~32), video-recorder --bus →
  at/status/recorder (315 frames/run), both visible to bus_dump on RK3588.
- **`detector` crate (TG26-35, ADR D-015)**: independent detection component —
  SHM ring in (guard discipline), existing inference backends (NPU bridge
  with the C++ unprojection contract, cpu-onnx, mock), detections on the
  event bus (`at/detections`, contract extended with frame_w/frame_h,
  backward compatible) + status topic (fps/infer p50-p95/e2e). Hardware
  contour on RK3588: 9.9 FPS, infer p50 95.8 ms, e2e ~240 ms, 293/293
  published, 0 errors; bus delivery test green on x86 and ARM.
- **Zenoh bus in production use**: detector (TG26-35) publishes
  at/detections + at/status/detector on the RK3588 contour; full component
  migration plan committed (docs/BUS_MIGRATION_PLAN.md, M0-M6, ~44-59h).
- **`event-bus` crate (D-014)**: typed event/data bus on Zenoh 1.10 —
  brokerless peer-to-peer (fixed topology, scouting off), typed
  pub/sub over serde_json, project topics (at/detections, at/telemetry,
  at/commands, at/config, at/status). Research compared 8 real
  implementations (docs/BUS_SELECTION.md); prototype validated on x86 and
  RK3588: one-way latency p50 588-814 us on ARM (vs 1 ms target),
  UDS-baseline comparison, 26 zenoh crates / 10.7 MB binary / 0 system
  deps. ADR D-014; component migration deliberately out of scope.
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
