# Key Files — map проекта

## Структура (пути от git-root)

```
auto-targeting/                  # корень репо (GitHub front-door)
├── README.md                    # канонический README (бейджи, quickstart, конфиг)
├── LICENSE-MIT / LICENSE-APACHE # dual license (как в Cargo.toml)
├── CONTRIBUTING.md              # branching + Conventional Commits + PR-чеклист
├── CHANGELOG.md                 # Keep a Changelog
├── SECURITY.md / SUPPORT.md / CODE_OF_CONDUCT.md
├── .github/
│   ├── workflows/               # ci.yml, nightly.yml, docs.yml (markdownlint+lychee)
│   ├── ISSUE_TEMPLATE/          # bug_report.md, feature_request.md
│   └── PULL_REQUEST_TEMPLATE.md
├── ai-context/                  # эта папка (контекст для агентов)
└── auto-targeting/              # Rust workspace (вложенный — CI завязан на это)
    ├── Cargo.toml               # workspace manifest
    ├── crates/
    │   ├── common/              # типы, config, scenario, errors
    │   ├── video-capture/       # VideoSource + V4L2 + V4l2Direct + Synthetic + Replay
    │   ├── yolov8/              # letterbox + postprocess (чистая логика)
    │   ├── cv-inference/        # InferenceBackend + ONNX + RKNN client
    │   ├── cv-visualizer/       # bbox/labels → JPEG/JSONL
    │   ├── system-telemetry/    # RSS/temp/FPS/latency
    │   ├── target-tracker/      # Kalman + Hungarian multi-target
    │   ├── fc-adapter/          # MAVLink (Mock/SITL/ArduPilot)
    │   ├── shmem-buffer/        # TG26-160: SPMC кольцо в SHM (POSIX shm/mmap)
    │   ├── video-recorder/      # TG26-125: рекордер-потребитель (ffmpeg+OSD)
    │   ├── event-bus/           # D-014: шина событий на Zenoh (peer-to-peer)
    │   ├── detector/            # TG26-35: детектор ring→NPU→шина (ADR D-015)
    │   ├── commander/           # StateMachine + anti-loop + PID + safety
    │   └── cli/                 # бинарь auto-targeting (REPL/scenario/health)
    ├── rknn-bridge/             # C++ микросервис NPU (CMake)
    ├── docs/                    # SDD-SPEC, KPI, SAFETY, ADR, PROJECT_REPORT, HARDWARE_TEST_RESULTS
    ├── scripts/                 # download_models, soak_30min, make_video, convert_rknn, run_hardware_tests
    ├── sim/scenarios/           # SITL JSON-сценарии
    └── config.example.toml      # шаблон конфига (6 секций)
```

> **Важно:** workspace вложен в `auto-targeting/auto-targeting/`. Это намеренно —
> CI использует `working-directory: auto-targeting`. Все `cargo`-команды —
> изнутри `auto-targeting/`.

## Точки входа

| Что | Где | Как запустить (из `auto-targeting/`) |
|---|---|---|
| CLI (REPL) | `cli/src/main.rs` | `cargo run -p auto-targeting-cli -- --repl` |
| Single-image inference | `cv-inference/examples/onnx_infer.rs` | `cargo run -p cv-inference --example onnx_infer --features cpu-onnx -- model.onnx img.jpg` |
| **Live camera demo** | `cv-inference/examples/live_camera_demo.rs` | `cargo run -p cv-inference --example live_camera_demo --features "cpu-onnx,v4l2-cam" -- --device /dev/video0` |
| Camera latency bench | `video-capture/examples/camera_latency.rs` | `cargo run -p video-capture --example camera_latency --features v4l2 -- --pipeline` |
| Direct capture bench | `video-capture/examples/direct_capture_bench.rs` | `cargo run -p video-capture --example direct_capture_bench --features v4l2-direct` |
| Soak 30 min | `cv-inference/examples/soak.rs` | `./scripts/soak_30min.sh` |
| rknn-bridge (C++) | `rknn-bridge/src/bridge_main.cpp` | `cd rknn-bridge/build && ./rknn-bridge` |
| **SHM producer** | `shmem-buffer/examples/shmem_producer.rs` | `cargo run --release -p shmem-buffer --example shmem_producer -- --name demo.frames` |
| **SHM consumer** | `shmem-buffer/examples/shmem_consumer.rs` | `... --example shmem_consumer -- --name demo.frames --mode next` |
| **Видеорекордер** | `video-recorder/src/main.rs` | `video-recorder --name autotarget.frames --out rec.mp4 --osd --font ...` |
| SITL scenarios | `cli/src/scenario_runner.rs` | `cargo run -- scenario --all sim/scenarios/` |

## Ключевые файлы (для быстрой навигации)

| Файл | Что |
|---|---|
| `common/src/types.rs` | ВСЕ доменные типы (Frame, Detection, SystemState, BoundingBox, ...) |
| `common/src/config.rs` | AppConfig + figment (TOML + env override `AT_`) |
| `cv-inference/src/backend.rs` | trait InferenceBackend + InferenceError |
| `cv-inference/src/cpu_onnx.rs` | реальный ONNX Runtime backend (feature cpu-onnx) |
| `cv-inference/src/bridge_client.rs` | RknnBridgeClient (Unix-socket IPC, unix-only) |
| `yolov8/src/lib.rs` | letterbox + postprocess + COCO_LABELS |
| `video-capture/src/v4l2_real.rs` | V4l2Source через `v4l` crate (drop-old policy) |
| `video-capture/src/v4l2_direct.rs` | **V4l2DirectSource — прямой libc ioctl, 32 FPS** |
| `commander/src/state_machine.rs` | StateMachine + таблица переходов |
| `commander/src/anti_loop.rs` | AntiLoopGuard + oscillation detector |
| `fc-adapter/src/traits.rs` | trait FlightControllerAdapter |
| `cli/src/repl.rs` | 15 операторских команд |
| `rknn-bridge/src/rknn_model.cpp` | RknnBackend (zero-copy NPU + sigmoid + YOLOv8 парсер) |
| `rknn-bridge/src/bridge_main.cpp` | Unix-socket сервер + JSON/base64 протокол |

## Конфигурация

`auto-targeting/auto-targeting/config.example.toml` — 6 секций: `[video]`,
`[inference]`, `[tracker]`, `[fc]`, `[commander]`, log. Override через env с
префиксом `AT_` (секции через `__`):

```bash
AT_VIDEO__DEVICE=/dev/video1 AT_INFERENCE__CONFIDENCE_THRESHOLD=0.5 \
    cargo run -p auto-targeting-cli -- --repl
```
