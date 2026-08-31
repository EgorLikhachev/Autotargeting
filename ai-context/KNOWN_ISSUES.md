# Known Issues — TODO и средовые ограничения

**Дата актуализации:** 2026-08-14

## Баги из аудита (приоритеты P0/P1/P2)

| # | Расхождение | Приоритет | Статус | Где |
|---|---|---|---|---|
| 1 | **Endianness length-prefix** (C++ native ↔ Rust big-endian) | P0 | ✅ fixed (D-002) | `shm_server.cpp` (htonl/ntohl) |
| 2 | **Кадры в bridge шли base64** | P1 | ✅ **Fixed** (D-016/M6: именованный SHM-сегмент, base64 — fallback) | `shm_server.cpp`, `bridge_client.rs` |
| 3 | **Crude coordinate transform** | P1 | ✅ **Fixed** (этап 7: CameraToAngular с attitude, пиксели→FOV-угол→NED yaw) | `commander.rs` |
| 4 | **Упрощённый Kalman** — fixed-gain вместо 4×4 covariance | P2 | Open | `kalman.rs:79` |
| 5 | **`select_target` без confirmation** — сразу Tracking без `lock_confirmation_frames` | P2 | Open | `commander.rs:254` |

## Из тестирования на железе

| # | Проблема | Приоритет | Статус |
|---|---|---|---|
| 6 | **Rust yolov8::postprocess нет sigmoid** — RKNN-export даёт raw логиты, ONNX-export встраивает sigmoid. C++ bridge уже починен (sigmoid для class scores), Rust-парсер — нет. Влияет только на CPU-путь (x86), NPU-путь работает. | P2 | Open |
| 7 | **NMS tuning** — избыточные детекции (5171 за 15с live-demo), нужно ~10 на кадр. Threshold=0.45 не полностью подавляет overlapping мелкие boxes. | P2 | Open |
| 8 | **`live_camera_demo` берёт мало кадров** (5 за 15с вместо ~450) — pump-цикл ломался на channel **Full**, а не только Closed | P1 → ✅ | **Fixed**: `pump_frames()` break только по Closed (`a15e427`); с PS Eye — 84 кадра/15с |
| 9 | **`v4l` crate давал 21 FPS vs 100 у v4l2-ctl** | P0 → ✅ | **Fixed**: `v4l2_direct.rs` (прямой libc ioctl) → 32 FPS |
| 10 | **SyntheticVideoSource channel bound=1** при infinite (back-pressure deadlock) | P1 → ✅ | **Fixed**: `(fps).clamp(3,30)` + `try_send` |
| 11 | **`v4l` crate ВИСНЕТ на gspca-драйвере** (PS Eye/ov534): start() ок, recv() ни кадра | P0 для gspca | Обход: `--backend direct` (примеры уже поддерживают); на UVC — работает, но медленно |
| 12 | **rknn-bridge без таймаутов** | P2 | ✅ **Fixed** (M6: re-accept после disconnect + SO_RCVTIMEO 30с + length-sanitize) |

## Средовые ограничения

- **cpu-onnx на RK3588**: prebuilt ONNX Runtime (ort.pyke.io) требует libstdc++ GCC13, Debian 12 bookworm поставляет GCC12. Решение: на RK3588 использовать NPU (RKNN), ONNX только для x86-разработки. (D-008)
- **Windows + MSVC**: статическая линковка ort-sys не работает (несовместимость C++ runtime). Все тип-чеки проходят, ломается только финальная линковка test/example-бинарников. На Linux/x86 всё собирается.
- **V4L2 / sd-notify / Unix-сокеты**: Linux-only. На Windows эти крейты (`video-capture` feature `v4l2`/`v4l2-direct`, `cli`, `commander`) не собираются. Это существующее ограничение проекта (целевая платформа — Linux/RK3588).
- **librknnrt.so**: проприетарный runtime Rockchip, НЕ redistributed в репо (не покрывается MIT/Apache). Берётся из релизов [rknn-toolkit2](https://github.com/airockchip/rknn-toolkit2).

## Test-gaps

- Нет round-trip теста C++↔Rust (нужен Unix-socket integration test).
- Soak с реальной камерой — live-demo отработал (5171 детекция), но длительный soak с `V4l2DirectSource` ещё не прогонялся (TODO #8).
- `#[ignore]`-тесты (vivid/v4l2-gated, integration_with_real_bridge) — запускаются флагом `-- --include-ignored`.

См. `auto-targeting/docs/sdd/decisions.md` D-001…D-011 для контекста решений.
