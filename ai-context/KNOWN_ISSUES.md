# Known Issues — TODO и средовые ограничения

## Баги из аудита (приоритеты P0/P1/P2)

| # | Расхождение | Приоритет | Статус | Где |
|---|---|---|---|---|
| 1 | **Endianness length-prefix** (C++ native ↔ Rust big-endian) | P0 | ✅ fixed (D-002) | `shm_server.cpp` (htonl/ntohl) |
| 2 | **SCM_RIGHTS не реализован** — frame идёт base64 inline | P1 | Open | `shm_server.cpp`, `bridge_client.rs` |
| 3 | **Crude coordinate transform** — `offset_x→east, offset_y→down` напрямую | P1 | Open | `commander.rs:451` |
| 4 | **Упрощённый Kalman** — fixed-gain вместо 4×4 covariance | P2 | Open | `kalman.rs:79` |
| 5 | **`select_target` без confirmation** — сразу Tracking без `lock_confirmation_frames` | P2 | Open | `commander.rs:254` |

## Из тестирования на железе

| # | Проблема | Приоритет | Статус |
|---|---|---|---|
| 6 | **Rust yolov8::postprocess нет sigmoid** — RKNN-export даёт raw логиты, ONNX-export встраивает sigmoid. C++ bridge уже починен (sigmoid для class scores), Rust-парсер — нет. Влияет только на CPU-путь (x86), NPU-путь работает. | P2 | Open |
| 7 | **NMS tuning** — 1342 детекции на bus.jpg (после NMS), нужно ~10. Threshold=0.45 не полностью подавляет overlapping мелкие boxes. | P2 | Open |
| 8 | **conf=0.50=sigmoid(0)** у большинства детекций — слабые логиты int8-модели с dummy-калибровкой. Нужен реальный fine-tune. | P2 → 1.2 | Open (модель-зависимая) |

## Средовые ограничения

- **cpu-onnx на RK3588**: prebuilt ONNX Runtime (ort.pyke.io) требует libstdc++ GCC13, Debian 12 bookworm поставляет GCC12. Решение: на RK3588 использовать NPU (RKNN), ONNX только для x86-разработки. (D-008)
- **Windows + MSVC**: статическая линковка ort-sys не работает (несовместимость C++ runtime). Все тип-чеки проходят, ломается только финальная линковка test/example-бинарников. На Linux/x86 всё собирается.
- **V4L2 / sd-notify / Unix-сокеты**: Linux-only. На Windows эти крейты (`video-capture` feature `v4l2`, `cli`, `commander`) не собираются. Это существующее ограничение проекта (целевая платформа — Linux/RK3588).
- **GitHub-репо**: переименование `Autotatgeting → Autotargeting` — локально сделано (remote + 23 ссылки), само переименование на GitHub — за владельцем через web-UI (D-003).

## Test-gaps

- Нет round-trip теста C++↔Rust (нужен Unix-socket integration test).
- Нет soak-теста с реальной камерой (только SyntheticSource).
- 13 `#[ignore]`-тестов (vivid/v4l2-gated, integration_with_real_bridge).

См. `auto-targeting/docs/sdd/decisions.md` D-001…D-010 для контекста решений.
