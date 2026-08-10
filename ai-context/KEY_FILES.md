# Key Files — map проекта

## Структура

```
auto-targeting/                  # корень репо
├── .github/workflows/           # CI (ci.yml, nightly.yml)
├── ai-context/                  # эта папка (контекст для агентов)
├── auto-targeting/              # Rust workspace (вложенный)
│   ├── Cargo.toml               # workspace manifest
│   ├── crates/
│   │   ├── common/              # типы, config, scenario, errors
│   │   ├── video-capture/       # VideoSource + V4L2 + Synthetic + Replay
│   │   ├── yolov8/              # letterbox + postprocess (чистая логика)
│   │   ├── cv-inference/        # InferenceBackend + ONNX + RKNN client
│   │   ├── cv-visualizer/       # bbox/labels → JPEG/JSONL
│   │   ├── system-telemetry/    # RSS/temp/FPS/latency
│   │   ├── target-tracker/      # Kalman + Hungarian multi-target
│   │   ├── fc-adapter/          # MAVLink (Mock/SITL/ArduPilot)
│   │   ├── commander/           # StateMachine + anti-loop + PID + safety
│   │   └── cli/                 # бинарь auto-targeting (REPL/scenario/health)
│   ├── rknn-bridge/             # C++ микросервис NPU (CMake)
│   ├── docs/                    # SDD-SPEC, KPI, SAFETY, ADR, HARDWARE_TEST_RESULTS
│   ├── scripts/                 # download_models, soak_30min, make_video, convert_rknn
│   ├── sim/scenarios/           # 5 SITL JSON-сценариев
│   └── config.example.toml      # шаблон конфига
└── .gitignore
```

## Точки входа

| Что | Где | Как запустить |
|---|---|---|
| CLI (REPL) | `cli/src/main.rs` | `cargo run -p auto-targeting-cli -- --repl` |
| Single-image inference | `cv-inference/examples/onnx_infer.rs` | `cargo run -p cv-inference --example onnx_infer --features cpu-onnx -- model.onnx img.jpg` |
| Soak 30 min | `cv-inference/examples/soak.rs` | `./scripts/soak_30min.sh` |
| rknn-bridge (C++) | `rknn-bridge/src/main.cpp` | `cd rknn-bridge/build && ./rknn-bridge` |
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
| `commander/src/state_machine.rs` | StateMachine + таблица переходов |
| `commander/src/anti_loop.rs` | AntiLoopGuard + oscillation detector |
| `fc-adapter/src/traits.rs` | trait FlightControllerAdapter |
| `cli/src/repl.rs` | 15 операторских команд |
| `rknn-bridge/src/rknn_model.cpp` | RknnBackend (zero-copy NPU + sigmoid + YOLOv8 парсер) |

## Конфигурация

`auto-targeting/auto-targeting/config.example.toml` — 6 секций: `[video]`, `[inference]`, `[tracker]`, `[fc]`, `[commander]`, log. Override через env с префиксом `AT_`:
```bash
AT_VIDEO__DEVICE=/dev/video1 AT_INFERENCE__CONFIDENCE_THRESHOLD=0.5 auto-targeting
```
