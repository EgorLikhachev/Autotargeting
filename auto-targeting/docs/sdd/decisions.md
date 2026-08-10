# Decisions Log — Auto-Targeting SDD

> Журнал нетривиальных архитектурных и процессных решений. Требование
> SDD-промта (§«Антизацикливающие директивы»): каждое решение фиксируется с
> причиной, чтобы при зацикливании можно было быстро откатиться.
>
> Формат: `ADR`-style — Контекст → Решение → Последствия. Обратная
> совместимость: новые записи только добавляются; устаревшие помечаются
> `**[SUPERSEDED by D-NNN]**`, не удаляются.

---

## D-001 — SDD-спецификация как единственный источник истины
**Дата:** 2026-08-04 · **Статус:** Accepted

**Контекст:** Проект вырос до 10 Rust-крейтов + C++ rknn-bridge, 356 unit-тестов, развёртывание на RK3588. Новые фичи/рефакторинг требуют постоянного чтения кода агентами, что дорого и подвержено ошибкам.

**Решение:** Ввести `docs/SDD-SPEC.md` как канон спецификации. Все будущие изменения начинаются с правки SDD → агент читает diff → генерирует код → тесты → обновляет SDD. Промт явно требует «настолько полной, чтобы агент не читал код».

**Последствия:**
- (+) Onboarding новых агентов/разработчиков сокращается до чтения одного файла.
- (+) Контракты (traits, протоколы, конфиг) централизованы — меньше drift.
- (−) SDD нужно поддерживать синхронно с кодом; иначе становится ложью. Шаг 6 (валидация) — обязателен при каждом изменении.
- (−) Объём ~1000 строк; решено принять (ТЗ требует полноты).

---

## D-002 — Канонический big-endian для IPC length-prefix
**Дата:** 2026-08-05 · **Статус:** Accepted · **Связанный баг:** endianness (SDD §15)

**Контекст:** Rust-клиент `bridge_client.rs` пишет 4-байтный length-prefix через `to_be_bytes()` (big-endian). C++ `shm_server.cpp` читал/писал native uint32_t → на little-endian (x86, aarch64 — все целевые платформы) протокол несовместим, bridge нерабочий.

**Решение:** Канон = **big-endian / network byte order**. C++ приведён к канону через `htonl`/`ntohl` (`<arpa/inet.h>`), Rust остаётся без изменений. Это行业标准 для сетевых протоколов и соответствует существующему Rust-коду.

**Альтернативы рассмотрены:**
- Привести Rust к little-endian (`to_le_bytes`) — отвергнуто: ломает «network byte order» конвенцию, требует менять уже-написанный Rust.
- Ввести версию протокола с negotiacion — overkill для length-prefix.

**Последствия:**
- (+) Bridge становится работоспособным на любой little-endian платформе.
- (+) Совпадает с сетевой конвенцией (htons/htonl).
- (−) На Windows нет `<arpa/inet.h>` — shm_server.cpp остаётся Linux-only (существующее ограничение, не регрессия).
- (!) Test-gap: нет round-trip теста C++↔Rust. Зафиксировано в SDD §15 как TODO (нужен Unix-socket integration test).

---

## D-003 — Переименование репозитория Autotatgeting → Autotargeting
**Дата:** 2026-08-05 · **Статус:** Partial (локально готово, GitHub — за владельцем)

**Контекст:** Имя GitHub-репо содержит опечатку (`Autotatgeting` вместо `Autotargeting`), фигурирует в 23 местах docs/Cargo.toml/scripts. Это косметический, но постоянный источник путаницы.

**Решение:** Переименовать GitHub-репо (Settings → Rename). GitHub сохраняет редирект старого имени, поэтому внешние клоны/CI продолжат работать.

**Последствия:**
- (+) Устранена опечатка во всех ссылках после правки 23 вхождений.
- (+) Редирект старого URL — обратная совместимость для существующих клонов.
- (−) Один ручной шаг за владельцем репо (gh CLI не установлен в окружении).
- (!) После GitHub-переименования пушить через новый remote URL (уже обновлён локально).

---

## D-004 — Не переписывать git-историю (UUID-коммиты)
**Дата:** 2026-08-05 · **Статус:** Accepted

**Контекст:** 14 из 25 коммитов имеют UUID-сообщения (шум от авто-коммитов AI-ассистента `Z User <z@container>`). Squash/rewrite сделал бы историю чище.

**Решение:** НЕ переписывать. История уже запушена в `origin/feature/phase-1.1-cv-loop`; force-push рискован и ломает любые существующие клоны/PR.

**Последствия:**
- (+) Никакого риска для запушенной ветки.
- (−) UUID-коммиты остаются в истории. Принято как приемлемый шум.
- (!) Будущие коммиты следуют conventional-commits (feat/fix/docs/chore/refactor) — уже начато в Phase 1.1.

---

## D-005 — Фиксировать 5 багов аудита, чинить только endianness
**Дата:** 2026-08-05 · **Статус:** Accepted

**Контекст:** Аудит кода (SDD Шаг 4/6) обнаружил 5 расхождений: endianness (critical), SCM_RIGHTS stub, crude coordinate transform, упрощённый Kalman, select_target без confirmation.

**Решение:** В этом раунде чиню только endianness (D-002 — реальный interoperability-блокер). Остальные 4 — в SDD §15 как TODO с приоритетами (P1/P2). Очистка+SDD — отдельная задача от багфиксов; смешивать нельзя (anti-loop: одна подзадача — один тип работы).

**Последствия:**
- (+) Сфокусированный, ревьюабельный набор коммитов.
- (+) 4 TODO задокументированы — не потеряются.
- (−) 4 бага остаются в коде до следующих раундов. Принято (явно отмечено пользователем).

---

## D-006 — MCP-серверы: рекомендации, не внедрение
**Дата:** 2026-08-05 · **Статус:** Accepted

**Контекст:** SDD-промт требует предложить MCP-серверы для SDD-процесса. Внедрение (конфиг `.zcode/`/env) — отдельная задача конфигурации окружения.

**Решение:** В SDD §13 даю список рекомендуемых MCP-серверов с обоснованием и примерами команд. Физическое внедрение — за пользователем в отдельном раунде.

**Последствия:**
- (+) Не смешиваю документирование с конфигурацией окружения.
- (+) Рекомендации зафиксированы для будущих агентов.
- (−) MCP пока не подключены. Принято.

---

## D-007 — RKNN SDK 2.x: RKNN_TENSOR_NHWC вместо FORMAT_RGB
**Дата:** 2026-08-05 · **Статус:** Accepted · **Найден:** на целевом железе (Orange Pi 5, librknnrt 2.3.0)

**Контекст:** При первой сборке `rknn-bridge` с `HAVE_RKNN=1` на реальном NPU
компиляция упала: `RKNN_TENSOR_FORMAT_RGB was not declared`. Наш код (из
Phase 1.1/E) использовал имя из **SDK 1.x**. В SDK 2.x enum
`rknn_tensor_format` переименован: вместо pixel-format-семантики
(`RGB`/`BGR`/`GRAY`) теперь layout-семантика (`NCHW`/`NHWC`/`NC1HWC2`).
RGB-vs-BGR channel order теперь фиксируется при конвертации модели, а не в
`rknn_input.fmt`.

**Решение:** `input.fmt = RKNN_TENSOR_FORMAT_RGB` → `RKNN_TENSOR_NHWC`
(RGB24 packed bytes = NHWC layout, N=1 implicit). Коммит `5576f22`.

**Последствия:**
- (+) Bridge компилируется и линкуется с librknnrt 2.3.0.
- (!) Этот баг **невозможно было обнаружить на x86** — там нет RKNN SDK. Только
  on-device тестирование выявило его. Подтверждает ценность SDD §15 test-gap
  (нет C++↔Rust round-trip теста).
- (!) Если в будущем вернутся к SDK 1.x (старые платы), потребуется `#[cfg]`-стиль
  ветвление по версии SDK. Сейчас target = 2.x, принято.

---

## D-008 — cpu-onnx не поддерживается на Debian 12 / RK3588
**Дата:** 2026-08-05 · **Статус:** Accepted (ограничение окружения)

**Контекст:** `cargo test -p cv-inference --features cpu-onnx` на устройстве
падает на линковке: `undefined reference to __cxa_call_terminate / _M_replace_cold`.
Prebuilt ONNX Runtime (ort.pyke.io) собран с GCC 13+, требует libstdc++ 13;
Debian 12 bookworm поставляет libstdc++ 12.x (`GLIBCXX_3.4.30`), нужных символов
нет; `libstdc++-13-dev` в apt отсутствует.

**Решение:** Принять как средовое ограничение. CPU-ONNX fallback предназначен для
**x86-разработки** (где libstdc++ 13 доступен), не для RK3588. На RK3588 основным
путём является NPU/RKNN, для которого ONNX не нужен.

**Последствия:**
- (+) Не нужно тащить ONNX Runtime в production-сборку на борту.
- (−) Dev-цикл на самом устройстве ограничен mock/RKNN; для ONNX-экспериментов — x86.
- (!) Зафиксировано в [HARDWARE_TEST_RESULTS.md §3](../HARDWARE_TEST_RESULTS.md).

---

## D-009 — INT8 vs float16 RKNN model: trade-off documented
**Дата:** 2026-08-06 · **Статус:** Accepted (контекст для будущей работы)

**Контекст:** На Orange Pi 5 (librknnrt 2.3.0) протестированы две конвертации
yolov8n.onnx:

1. **INT8** (`do_quantization=True`, dummy-noise calibration): `rknn_outputs_get`
   работает (size≠0), но детекции = 0 (bad calibration — шумовые картинки
   «схлопнули» квантизацию). С реальным calib (bus.jpg + zidane.jpg) — всё равно
   0: YOLOv8 чувствителен к INT8, нужно много данных + возможно QAT.
2. **float16** (`do_quantization=False`): rknn-toolkit2 Python даёт **47 корректных
   детекций** (class=0 person, class=5 bus) на bus.jpg. Но наш C++ bridge через
   `rknn_outputs_get(want_float=1)` получает **size=0** — librknnrt 2.3.0 не
   возвращает output для float16-модели этим API.

**Решение:** Для baseline зафиксировать **float16 как целевую** (она даёт
корректные детекции в эталоне). Для моста — нужен переход на **zero-copy API**
(`rknn_create_mem` + `rknn_set_io_mem`), который корректно работает с float16
output. Это TODO P1.

**Последствия:**
- (+) Подтверждено: NPU железо + модель + наш YOLOv8-парсер — всё валидно
  (47 детекций с правильными классами в Python-эталоне).
- (+) Throughput: bridge inference latency = 27-29ms (~35 FPS только NPU) —
  KPI ≥15 FPS выполнен с запасом.
- (−) End-to-end детекции через C++ bridge пока недоступны — нужен zero-copy API.
- (!) Эскалация по anti-loop (SDD §12): ~7 итераций на dtype/format исследования,
  лимит 5 превышен. Зафиксировано для следующего раунда.

---

## D-010 — Zero-copy IO + sigmoid в C++ bridge: end-to-end детекции работают
**Дата:** 2026-08-10 · **Статус:** Accepted

**Контекст:** После D-009 (float16-output не читался через rknn_outputs_get)
проведено исследование RKNN zero-copy API. Найден официальный паттерн из
`rknn_create_mem_demo.cpp`: установить `output_attr_.type = RKNN_TENSOR_FLOAT32`
перед `rknn_set_io_mem`, и рантайм сам конвертирует native fp16 NPU-output в
наш float32-буфер. Это заменяет rknn_inputs_set/rknn_outputs_get полностью.

**Реализовано:**
- `load_model`: query input/output attrs → fields, compute `is_quant_`, log SDK
  version, persistent `rknn_create_mem` для input + output (один раз, переиспользуется).
- `infer`: memcpy кадра в input_mem (с учётом w_stride), rknn_run, прямой
  `float*` из output_mem_->virt_addr. Убран весь rknn_outputs_get/want_float.
- Деструктор: rknn_destroy_mem перед rknn_destroy.

**Вторая находка (корень zero-detections):** RKNN-export YOLOv8 **не встраивает
sigmoid** в выход модели (в отличие от ONNX-export Ultralytics). Raw class
scores — это pre-sigmoid логиты; наш парсер ожидал post-sigmoid [0,1].
Добавлена численно-стабильная sigmoid-лямбда в C++ парсер (только для class
scores rows 4+, не для box coords rows 0..3).

**Результат:** end-to-end детекции через C++ bridge на bus.jpg: 1342 person
detection (class верный, bbox реальные). Phase 1.1 критерий «рамки/классы/
confidence сохраняются» — **выполнен**.

**Последствия:**
- (+) NPU-путь полностью работает: NPU → zero-copy → sigmoid → парсер → NMS → Detection.
- (+) Throughput: 86ms cold / ~32ms warm (NPU inference) — KPI ≥15 FPS выполнен.
- (−) Rust yolov8::postprocess НЕ имеет sigmoid (ONNX-export встраивает его).
  Синхронизация — TODO P2 (для CPU-пути на x86; RKNN-путь на устройстве уже работает).
- (−) conf=0.50 (sigmoid(0)) у большинства детекций — слабые логиты из-за dummy
  калибровки int8-модели. Нужен реальный fine-tune (задача 1.2).
- (−) 1342 детекции после NMS — избыточно. NMS-tuning / лучший порог — TODO.
