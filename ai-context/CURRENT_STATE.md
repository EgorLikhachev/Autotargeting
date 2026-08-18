# Current State — готовность по модулям

**Дата:** 2026-08-14 · **Ветка:** `main` @ `33028e6` (после repo-formatting)

## Готовность (✅ = работает, 🟡 = stub/частично, 🔴 = не начато)

### Phase 1.1 (минимальный контур CV) — ✅ ЗАКРЫТА + hardware-validated

| Компонент | Статус | Детали |
|---|---|---|
| Единый модуль изображений (`VideoSource`) | ✅ | Synthetic + Replay + V4l2Source (`v4l2`) + **V4l2DirectSource (`v4l2-direct`, прямой ioctl, 32 FPS)** |
| Инференс ONNX Runtime (x86 dev) | ✅ | `cv-inference` feature `cpu-onnx` (ort 2.0-rc.13) |
| Инференс RKNN NPU (RK3588) | ✅ | `rknn-bridge` C++, zero-copy + sigmoid, 27–29 ms |
| Парсер YOLOv8 (letterbox + postprocess + NMS) | ✅ | `yolov8` крейт, зеркало в C++ |
| End-to-end детекции на NPU | ✅ | person/bus на bus.jpg через C++ bridge |
| **Live camera demo (камера → NPU → видео)** | ✅ | `examples/live_camera_demo.rs`, 5171 детекция, аннотированный MP4 |
| Headless визуализатор | ✅ | `cv-visualizer` (bbox/labels/JPEG/JSONL) |
| Телеметрия | ✅ | `system-telemetry` (RSS, CPU/NPU temp, FPS/latency) |
| Soak-тест 30 мин | ✅ | `examples/soak.rs` + `scripts/soak_30min.sh` |
| Конвертация ONNX→RKNN | ✅ | `scripts/convert_rknn.py` (rknn-toolkit2 2.3.0) |

### Реальные метрики с железа (Orange Pi 5)

| Метрика | Значение | KPI |
|---|---|---|
| NPU inference latency | **27–29 ms** | < 60 ms ✅ |
| Sustained FPS (NPU only) | **~34 FPS** | ≥ 15 ✅ |
| Bridge VmRSS (idle) | 5.7 MB | < 50 MB ✅ |
| CPU temp (под нагрузкой) | 45.3 °C | < 70 °C ✅ |
| **NPU temp (под нагрузкой)** | **44.4 °C** | < 85 °C ✅ |
| init latency (валидная модель) | 32–39 ms | — |
| V4L2 capture throughput (`v4l2-direct`) | **32 FPS** (vs 21 у `v4l` crate) | — |

Полные таблицы — `auto-targeting/docs/HARDWARE_TEST_RESULTS.md` (§2 метрики, §8 live demo).

### Остальное (по фазам ROADMAP)

| Компонент | Статус | Фаза |
|---|---|---|
| Трекер целей (Kalman + Hungarian) | 🟡 scaffold | Phase 1.2 / 3 |
| State machine + anti-loop (7 слоёв) | ✅ | Phase 5 |
| FC-адаптеры (Mock / SITL) | ✅ | Phase 4 |
| FC-адаптер ArduPilot MAVLink (real FC) | 🟡 stub | Phase 4 |
| CLI + REPL (15 команд) | ✅ | Phase 5 |
| Конфигурация (TOML + `AT_` env override) | ✅ | Phase 0 |
| CI/CD (CI + Nightly + docs workflow) | ✅ | Phase 0 |
| **Репозиторий оформлен (README/LICENSE/CONTRIBUTING/CHANGELOG/CoC/SECURITY/SUPPORT/issue-templates)** | ✅ | — |
| **SHM ring buffer (TG26-160, крейт `shmem-buffer`)** | ✅ 24/24 теста, 7/7 критериев, ADR D-013 | Phase 6 фундамент |
| **Видеорекордер (TG26-125, крейт `video-recorder`)** | ✅ HW-validated: MP4+OSD, параллельный потребитель не заблокирован (TORN=0) | первый SHM-потребитель |
| Тесты: **294 unit (lib, aarch64)** + 6 C++ NMS + criterion + SITL-сценарии | ✅ | — |
| **`run_full()`** (связать видео+инференс+FC в runtime) | 🟡 stub | Phase 5/6 |
| **Свой датасет + fine-tune** | 🔴 | Phase 1.2 |
| **Замкнутая петля CV→автопилот** | 🔴 главный риск | Phase 6+ |
| HITL / Flight tests | 🔴 | Phase 7–8 |

## Известные TODO (из аудита + тестирования на железе)

| # | TODO | Приоритет | Статус |
|---|---|---|---|
| 1 | Endianness length-prefix (C++↔Rust) | P0 | ✅ fixed (D-002) |
| 2 | SCM_RIGHTS / zero-copy SHM для кадра | P1 | 🟡 inline-base64 работает |
| 3 | Crude coordinate transform в commander | P1 | Open |
| 4 | Упрощённый Kalman (fixed-gain) | P2 | Open |
| 5 | `select_target` без lock_confirmation | P2 | Open |
| 6 | Rust yolov8::postprocess нет sigmoid (только CPU-путь) | P2 | Open (NPU-путь уже починен) |
| 7 | NMS tuning — избыточные детекции (5171 за 15с) | P2 | Open |
| 8 | Подключить `V4l2DirectSource` в `live_camera_demo` (5→~450 кадров за 15с) | P1 | Open (модуль готов) |

См. [`KNOWN_ISSUES.md`](KNOWN_ISSUES.md) для деталей.
