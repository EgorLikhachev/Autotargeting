# Заметки по разработке: компонент детектора (TG26-35)

**Дата:** 2026-08-18 · **Крейт:** `crates/detector` · **Решение:** ADR D-015

## 1. Архитектура

```
SHM ring (NV12, TG26-160) ─▶ FrameConsumer ─▶ копия кадра ─▶ [guard DROP]
                                                        └▶ препроцессинг*
                                                           └▶ InferenceBackend
                                                              (bridge NPU |
                                                               cpu-onnx | mock)
                                                                 └▶ DetectionsFrame
                                                                    └▶ шина at/detections
                                                                       + at/status/detector
```

\* препроцессинг зависит от бэкенда:
- **bridge (NPU)**: NV12→RGB24 → `yolov8::letterbox` 640×640; в `init`
  передаём `input_width/height = dims кольца`, `input_format="rgb24"` —
  C++-сторона делает letterbox-unprojection в эти размеры (контракт
  rknn-bridge, см. AUDIT/исследование задачи);
- **cpu-onnx / mock**: кадр кольца как есть (бэкенд конвертирует сам).

Guard-дисциплина: слот держится только на время копии — инференс (до ~96 мс
на base64-пути) никогда не замораживает кольцо.

## 2. Контракт события (ADR D-015)

`event_bus::DetectionsFrame`:
- `frame_seq` — id кадра кольца;
- `captured_at` — метка ЗАХВАТА кадра (из ts_ns слота);
- `detections[].bbox` — пиксели исходного кадра, origin левый-верх;
- `detections[].class/class_id/confidence/frame_seq`;
- `frame_w`, `frame_h` — размеры кадра (serde default — старые payload
  совместимы).

Статус `at/status/detector`: fps, infer p50/p95, e2e p50, processed/
published, jumps, infer_errors, detections_total, frame dims.

## 3. Результаты на стенде (RK3588, Vitade MJPG → NPU)

| Метрика | Значение |
|---|---|
| FPS детектора | **9.9** |
| infer p50 | 95.8 мс |
| e2e p50 (ts→publish) | ~240 мс |
| published / processed | 293/293, 0 ошибок |
| jumps | 58 (камера 30 FPS ≫ детектор 10 FPS) |

Тест шины-доставки на ARM — зелёный (2.42 с).

## 4. Ограничения

1. **Потолок ~10 FPS** — base64 round-trip NPU-пути (~96 мс); устранение —
   SCM_RIGHTS/SHM-мост (SDD §15 #2), отдельная задача.
2. **e2e растёт при перепроизводстве**: детектор медленнее камеры → кадры
   стареют в кольце; прыжки TooFarBehind→latest сглаживают (58 за 30 с).
   Для трекера предпочтителен режим «свежий кадр» — уже поддерживается
   (`latest`), выбор за потребителем.
3. **Over-detect int8-модели** (334K детекций за 30 с) — известная проблема
   dummy-калибровки (KNOWN_ISSUES №7/8); не влияет на контур, влияет на
   полезность события.
4. **Порядок завершения**: закрыть шину ДО drop bridge-клиента (Drop шлёт
   shutdown → rknn-bridge завершается).
5. bridge-бэкенд unix-only; детектор на Windows — mock/cpu-onnx.

## 5. Использование

```bash
# Стенд (подняты camera_publisher и rknn-bridge):
detector --segment autotarget.frames --backend bridge \
    --model ~/auto-targeting/auto-targeting/models/yolov8n_int8.rknn
# Тесты (везде): cargo test -p detector
```
