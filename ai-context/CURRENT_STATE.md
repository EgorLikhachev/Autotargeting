# Current State — готовность по модулям

**Дата:** 2026-08-10 · **Ветка:** `main` @ `v0.1.0-phase-1.1`

## Готовность (✅ = работает, 🟡 = stub/частично, 🔴 = не начато)

### Phase 1.1 (минимальный контур CV) — ✅ ЗАКРЫТА

| Компонент | Статус | Детали |
|---|---|---|
| Единый модуль изображений (`VideoSource`) | ✅ | SyntheticVideoSource + ReplaySource + V4l2Source (feature `v4l2`) |
| Инференс ONNX Runtime (x86 dev) | ✅ | `cv-inference` feature `cpu-onnx` (ort 2.0-rc.13) |
| Инференс RKNN NPU (RK3588) | ✅ | `rknn-bridge` C++, zero-copy + sigmoid, 32ms latency |
| Парсер YOLOv8 (letterbox + postprocess + NMS) | ✅ | `yolov8` крейт, зеркало в C++ |
| End-to-end детекции на NPU | ✅ | 1342 person detections на bus.jpg, conf=0.50 |
| Headless визуализатор | ✅ | `cv-visualizer` (bbox/labels/JPEG/JSONL) |
| Телеметрия | ✅ | `system-telemetry` (RSS, CPU/NPU temp, FPS/latency) |
| Soak-тест 30 мин | ✅ | `examples/soak.rs` + `scripts/soak_30min.sh` |
| Конвертация ONNX→RKNN | ✅ | `scripts/convert_rknn.py` (rknn-toolkit2 2.3.0) |

### Реальные метрики с железа (Orange Pi 5)

| Метрика | Значение | KPI |
|---|---|---|
| NPU inference latency | 27–29 ms | < 60 ms ✅ |
| Sustained FPS (client RT) | 17.1 | ≥ 15 ✅ |
| Bridge VmRSS (idle) | 5.7 MB | < 50 MB ✅ |
| NPU temp (idle) | 43.4 °C | observation |
| init latency (валидная модель) | 32–39 ms | — |

### Остальное (по фазам ROADMAP)

| Компонент | Статус | Фаза |
|---|---|---|
| Трекер целей (Kalman + Hungarian) | ✅ | Phase 3 |
| State machine (9 состояний) + anti-loop (7 слоёв) | ✅ | Phase 5 |
| FC-адаптеры (Mock/SITL/ArduPilot MAVLink) | ✅ | Phase 4 |
| CLI + REPL (15 команд) | ✅ | Phase 5 |
| Конфигурация (TOML + env override) | ✅ | Phase 0 |
| CI/CD (6 jobs + nightly) | ✅ | Phase 0 |
| Тесты: 356 unit + 5 criterion + 5 SITL-сценариев | ✅ | — |
| **`run_full()`** (связать видео+инференс+FC в runtime) | 🟡 stub | Phase 5/6 |
| **Свой датасет + fine-tune** | 🔴 | Phase 1.2 |
| **Реальная камера USB/MIPI** | 🟡 V4L2 реализован, не тестировался на стенде | Phase 1 |
| **Замкнутая петля CV→автопилот** | 🔴 главный риск | Phase 6+ |
| HITL / Flight tests | 🔴 | Phase 7–8 |

## 5 известных TODO (из аудита, P1/P2)

1. **Endianness length-prefix** — ✅ fixed (D-002)
2. **SCM_RIGHTS не реализован** — 🟡 inline-base64 работает, zero-copy SHM — TODO P1
3. **Crude coordinate transform в commander** — 🟡 TODO P1
4. **Упрощённый Kalman (fixed-gain)** — 🟡 TODO P2
5. **`select_target` без lock_confirmation** — 🟡 TODO P2

Дополнительно из тестирования на железе:
6. **Rust `yolov8::postprocess` sigmoid sync** — TODO P2 (CPU-путь ждёт post-sigmoid, NPU-путь уже починен)
7. **NMS tuning** — 1342 → ~10 детекций (TODO P2)

См. [`KNOWN_ISSUES.md`](KNOWN_ISSUES.md) для деталей.
