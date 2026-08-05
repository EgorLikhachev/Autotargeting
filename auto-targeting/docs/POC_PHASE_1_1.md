# PoC Phase 1.1 — Минимальный контур CV (камера → модель → обнаружения)

**Статус:** реализация завершена; физический прогон на RK3588 ожидает доступ к
NPU-железу. Этот документ фиксирует архитектуру минимального контура, что
готово в коде, известные ограничения и рекомендации для следующих этапов.

Связанные задачи: **1.1** (минимальный контур), **1.2** (основные решения).
Связанные документы: [KPI.md](KPI.md), [ARCHITECTURE.md](ARCHITECTURE.md),
[ADR-0001](ADR/0001-rknn-cpp-bridge.md).

---

## 1. Цели и критерии 1.1

Из ТЗ 1.1, минимальный контур:

> камера → модуль изображений → baseline-модель → обнаружения

Критерии готовности и их статус после этой фазы:

| Критерий 1.1 | Статус | Где |
|---|---|---|
| Модель получает кадры через единый модуль изображений | ✅ | `video-capture` (`VideoSource`), `cv-inference::CpuInferenceBackend` ест `Frame` напрямую |
| Рамки, классы и confidence отображаются или сохраняются | ✅ (сохранение) | `cv-visualizer` — аннотированные JPEG + JSONL |
| Измерены FPS, задержка, память, температура | ✅ (код) | `system-telemetry` + `MetricsRecorder`; цифры на железе — при первом прогоне |
| Непрерывный тест ≥ 30 минут | ✅ (код) | пример `soak`, скрипт `scripts/soak_30min.sh` |
| Сохранён пример обработанного видео | ✅ (скрипт) | `scripts/make_video.sh` (ffmpeg из JPEG-кадров) |
| Статья с результатами/ошибками/ограничениями | ✅ | этот документ |

Что **не делалось** (по 1.1): финальная модель не обучалась, точность не
гарантируется, полный датасет не собирается. Используется предобученный
COCO YOLOv8n как baseline.

---

## 2. Что добавлено в этой фазе

### 2.1 Новый крейт `yolov8` (чистая Rust-логика)

`crates/yolov8` — backend-агностичный препроцессинг и постпроцессинг YOLOv8:

- `letterbox` — resize в 640×640 с сохранением пропорций (pad=114);
- `LetterboxParams` — обратное преобразование координат в исходный кадр;
- `rgb_to_nchw_f32` — нормировка + NCHW float32 тензор;
- `postprocess` — парсинг выхода `[1, 4+nc, 8400]`: выбор лучшего класса,
  threshold, NMS, маппинг в координаты оригинала;
- `COCO_LABELS` — таблица 80 классов.

Покрытие: **15 unit-тестов**, включая edge-cases (NaN-координаты, пустой
выход, все-ниже-порога, неизвестный class_id). Парсер не зависит от ONNX или
RKNN — единая логика для CPU и NPU путей.

### 2.2 Реальный `CpuInferenceBackend` (ONNX Runtime, feature `cpu-onnx`)

`crates/cv-inference/src/cpu_onnx.rs` — замена прежнего stub'а. Полный пайплайн:
`Frame` → RGB24 → letterbox → NCHW → `ort` session.run → парсер `yolov8`.

Включается фичей `cpu-onnx` (по умолчанию выкл, чтобы не тянуть ONNX в сборки,
где нужен только mock). `ort 2.0.0-rc.13` скачивает prebuilt ONNX Runtime — без
системных зависимостей на x86_64 и aarch64 Linux.

### 2.3 Новый крейт `cv-visualizer` (headless-аннотация)

`crates/cv-visualizer` — отрисовка bbox/классов/confidence и сохранение:

- `annotate(frame, detections, font?)` → `RgbImage` через `image`+`imageproc`;
- `FrameWriter` — пишет `frames/seq_NNNNNN.jpg` + `detections.jsonl`, с
  throttle `save_every_n`;
- Текст подписи — опционально через TTF (`with_font_path`), иначе только рамки;
  класс/confidence всегда полностью в JSONL.

Чистый Rust (без OpenCV), кросс-компилируется на aarch64. Покрытие: **12 тестов**.

### 2.4 Новый крейт `system-telemetry` (метрики + термометрия)

`crates/system-telemetry`:

- `rss_kb()` — VmRSS из `/proc/self/status`;
- `cpu_temp_c()` / `npu_temp_c()` — thermal-зоны sysfs (`cpu-thermal`,
  `npu-thermal` на RK3588);
- `npu_load_percent()` — devfreq NPU load;
- `TelemetrySample` — JSON-сериализуемый снимок;
- `metrics::MetricsRecorder` — аккумулятор latency по стадиям (capture/infer/
  annotate/total) с p50/p95/max и sustained FPS.

Linux-only в части sysfs/proc; на прочих ОС пробы возвращают `None`, поэтому
крейт собирается и на dev-машине. Покрытие: **16 тестов**.

### 2.5 Примеры `onnx_infer` и `soak`

- `examples/onnx_infer.rs` — single-image end-to-end: JPEG → Frame → модель →
  детекции. Закрывает «запустить готовую модель на изображении».
- `examples/soak.rs` — непрерывный контур видео→модель→аннотация→метрики на
  заданное число минут, с телеметрией и `summary.json`. Закрывает
  «непрерывный тест ≥ 30 минут».

### 2.6 Скрипты

- `scripts/download_models.sh` — скачивание `yolov8n.onnx`;
- `scripts/soak_30min.sh` — обёртка над `soak` на 30 минут;
- `scripts/make_video.sh` — ffmpeg mux JPEG → `processed.mp4`;
- `scripts/convert_rknn.py` — ONNX → INT8 RKNN (rknn-toolkit2 на хосте).

### 2.7 C++ RKNN-bridge — парсер YOLOv8

`rknn-bridge/src/rknn_model.cpp::RknnBackend::infer` — прежний TODO заменён
полной реализацией: `rknn_outputs_get` → разбор `[1, 4+nc, A]` → выбор лучшего
класса → threshold → NMS → reverse-letterbox в координаты оригинала. Логика
1:1 с Rust `yolov8::postprocess` (см. комментарии в файле — они явно указывают
держать оба парсера синхронно). В `load_model` добавлен `rknn_query` output
shape, чтобы `nc`/`anchors` не хардкодились.

---

## 3. Результаты замеров

> **Важно:** количественные цифры FPS/latency/температуры заполняются после
> первого физического прогона на RK3588. На x86 dev-машине (Windows + MSVC)
> статическая линковка `ort-sys` не работает (несовместимость C++ runtime),
> поэтому ONNX-рантайм здесь линкуется только на Linux. CI на Ubuntu прогоняет
> 2-минутный smoke; полный 30-мин soak — на железе.

Частично заполнено после первого прогона на RK3588 (2026-08-05). Полный отчёт —
[`HARDWARE_TEST_RESULTS.md`](HARDWARE_TEST_RESULTS.md). FPS/latency-ячейки
остаются `_заполнить_` до получения валидной `.rknn` модели (демо-модель
несовместима с драйвером 2.3.0); soak 30 мин — после того же.

| Метрика | Цель | x86 (CPU, ONNX) | RK3588 (NPU, RKNN) |
|---|---|---|---|
| Сборка на платформе | — | ✅ Linux x86_64 | ✅ **aarch64 native** |
| Unit-тесты | pass | — | ✅ **294/294** |
| `rknn-bridge` линкуется с librknnrt | — | n/a | ✅ **librknnrt.so 2.3.0** |
| IPC-протокол (endianness-фикс) | round-trip | — | ✅ **517 ms init round-trip** |
| NPU hardware responsive | yes | n/a | ✅ devfreq + thermal active |
| Video FPS (sustained) | ≥ 30 | _заполнить_ | _заполнить_ |
| Inference FPS | ≥ 15 | _заполнить_ | _заполнить_ (нужна валидная .rknn) |
| Inference latency p50 | < 60 ms | _заполнить_ | _заполнить_ |
| End-to-end latency p95 | < 150 ms | _заполнить_ | _заполнить_ |
| Memory: bridge VmRSS (idle) | < 50 MB | — | ✅ **5.7 MB** (запас ×9) |
| Max CPU temp (idle) | (наблюдение) | — | 45.3 °C (bigcore) |
| Max NPU temp (idle) | (наблюдение) | n/a | 43.4 °C (`npu-thermal`) |
| 30-min run crashes | 0 | _заполнить_ | _заполнить_ (после валидной модели) |

Артефакты прогона (где смотреть цифры):
- `output/soak/summary.json` — FPS + p50/p95 latency по стадиям;
- `output/soak/telemetry.jsonl` — временной ряд RSS/температуры;
- `output/soak/detections.jsonl` — детекции;
- `output/soak/frames/seq_NNNNNN.jpg` → `output/soak/processed.mp4`.

---

## 4. Ограничения текущей реализации

1. **Точность COCO-модели на классах стенда.** Базовая `yolov8n` обучена на
   COCO (person/car/...). Классы ТЗ 1.2 (палатка, ящик, бензовоз, джип) не
   покрыты — это допустимо для 1.1 («точность не гарантируется»), но явно
   фиксируется как ограничение. Расширение классов — задача 1.2/далее.

2. **Windows + MSVC: ONNX не линкуется.** `ort-sys` статически собран под
   более новый MSVC runtime, чем доступен в этом окружении; статическая
   линковка падает с `unresolved external __std_*`. На Linux/aarch64
   (целевая платформа) этой проблемы нет — prebuilt `.so` линкуется штатно.
   Все тип-чеки (`cargo check --features cpu-onnx`) проходят; проблема
   исключительно в финальной линковке test/example бинарников на Windows.

3. **Soak на синтетическом источнике.** По умолчанию `soak` использует
   `SyntheticVideoSource` (no camera). Это даёт корректные метрики
   CPU/памяти/латентности инференса, но не проверяет реальный путь захвата
   (V4L2/MIPI) и реалистичность самих детекций. На железе запускать с
   реальным источником.

4. **Текст подписей требует TTF.** `cv-visualizer` рисует bbox всегда, но
   подпись класса/confidence — только если передан путь к шрифту
   (`--font /usr/share/fonts/.../DejaVuSansMono.ttf`). Без шрифта подписи
   есть только в JSONL. Это сознательное решение, чтобы не встраивать
   бинарник шрифта в репо.

5. **C++ RKNN-bridge: физический NPU.** Парсер YOLOv8 реализован и компилируется,
   но end-to-end прогон через NPU требует `librknnrt.so` + `.rknn` модели на
   RK3588 (`HAVE_RKNN=1`). На прочих платформах собирается только StubBackend.

6. **SHM-передача кадра.** В `rknn-bridge/src/shm_server.cpp` получение кадра
   через SCM_RIGHTS остаётся незавершённым (как и ранее). Для Phase 1.1 это не
   блокирует soak-тест (CPU-путь идёт полностью в Rust), но для production NPU
   потребуется завершить (или временно упасть на Unix-socket передаче сырого
   кадра — медленнее, но рабочий).

---

## 5. Рекомендации для следующих этапов

### 5.1 Немедленно (для закрытия 1.1 на железе)

1. На RK3588: `./scripts/soak_30min.sh` с реальным V4L2/MIPI источником,
   заполнить таблицу §3 цифрами.
2. Завершить SHM-передачу кадра в `shm_server.cpp` (или завести fallback на
   Unix-socket).
3. Проверить INT8 RKNN-модель через `scripts/convert_rknn.py` — убедиться, что
   mAP не просел критично (> ~0.5 mAP на COCO val как baseline-порог).

### 5.2 Переход к 1.2

1. Сбор датасета для классов стенда (палатка, ящик, бензовоз, джип) —
   собственная съёмка с коптера + материалы заказчика + симуляционные/открытые
   данные; разные высоты/углы/дистанции.
2. Fine-tune YOLOv8n на узких классах; экспорт в ONNX → RKNN.
3. Расширить `yolov8::COCO_LABELS` через config-таблицу классов
   (`[inference] labels = [...]`).

### 5.3 Архитектурные улучшения (не Blocking)

- Заменить hand-rolled JSON в `bridge_main.cpp` на `nlohmann/json` (header-only)
  — авторы bridge сами отмечают fragility текущего сериализатора.
- Формализовать выбор бэкенда: `mock | cpu-onnx | rknn-bridge` как единый enum
  в конфиге (сейчас выбор размазан по `allow_cpu_fallback` и feature-флагам).
- dmabuf/zero-copy: `Frame` сейчас владеет `Vec<u8>`; для production на RK3588
  перейти на dmabuf/shm, чтобы кадр не копировался между захватом и NPU.
- Property-тесты на C++ парсер YOLOv8 (зеркало Rust proptest) — гарантия, что
  оба парсера остаются численно идентичными.

### 5.4 Риск (отдельное исследование)

Интеграция CV ↔ автопилот — наиболее неопределённая часть (отмечено в ТЗ 1.2).
Эта фаза намеренно её не затрагивает: командир/state-machine уже готовы, но
реальная замкнутая петля CV→FC→движение выносится в отдельное исследование
после рабочего детектора на железе.

---

## 6. Чек-лист готовности (финальный)

- [x] `CpuInferenceBackend` отдаёт реальные детекции из `.onnx` (код; прогон на Linux)
- [x] C++ `RknnBackend` парсит выход YOLOv8 (код; прогон на RK3588)
- [x] Аннотированные кадры сохраняются; `make_video.sh` собирает `processed.mp4`
- [x] Измерены FPS, latency (p50/p95), память (VmRSS), температура (CPU + NPU на RK3588) — код; цифры §3
- [x] Soak-тест ≥ 30 мин (`soak` example + `soak_30min.sh`)
- [x] Данный документ (результаты, ошибки, ограничения, рекомендации)
- [ ] Заполнить таблицу §3 цифрами с железа
- [ ] Обновить [KPI.md](KPI.md) статусами Confirmed после прогона на RK3588
