# Auto-Targeting System — Архитектура и Roadmap

> **Проект:** Companion Computer для автономного наведения дрона самолетного типа.
> **Вычислитель:** Orange Pi 5 (RK3588S, 6 TOPS NPU).
> **Камера:** Arducam UC-852 (USB UVC).
> **Язык:** Rust (вся логика), C++ (RKNN-bridge микросервис).
> **FC:** ArduPilot на SpeedyBee F405 (референс), архитектура agnostic к железу.
> **Документ:** Planning-mode output, готов для импорта в Jira/Linear как набор Epic'ов.
> **Версия:** 0.1 (draft) · **Дата:** 2026-08-01

---

## 0. Принципы и конвенции проекта

Прежде чем перейти к архитектуре, зафиксируем принципы, которые влияют на все нижеперечисленные решения. Эти принципы должны быть явно упомянуты в каждом Architecture Decision Record (ADR) и в каждой фазе Roadmap — иначе архитектура быстро превратится в «shared bag of hacks».

**P1. Fail-safe по умолчанию.** Любой модуль при потере входного потока (видео, detections, heartbeat FC) обязан перейти в деградированный режим, который не посылает управляющих команд. Лучше потерять цель, чем разбить дрон. Это означает: watchdog-таймеры обязательны на каждом цикле, а не опциональны.

**P2. Hardware-agnostic через trait boundaries.** Все аппаратно-зависимые компоненты (камера, FC, NPU) скрыты за Rust-trait'ами. Меняется железо — меняется только конкретная реализация trait'а, верхняя логика не трогается. Это позволяет быстро мигрировать с SpeedyBee на Pixhawk и с Orange Pi на Jetson.

**P3. Латентность важнее пропускной способности.** В системе наведения 30 FPS при 50 ms латентности лучше, чем 60 FPS при 150 ms. Все измерения производительности в первую очередь замеряют end-to-end latency, а не throughput.

**P4. Гипотезы фиксируются до кода.** Любое архитектурное предположение («MAVLink v2 держит 50 Hz стриминг поз», «NPU RK3588S тянет YOLOv8n INT8 на 30 FPS») сначала попадает в `HYPOTHESES.md`, затем тестируется, и только после подтверждения попадает в production-код. Это защищает от хрупких зависимостей, которые «вдруг» ломаются в полевых тестах.

**P5. Воспроизводимость тестов.** Все тесты (unit, integration, SITL, HITL) должны быть детерминированными. Это значит: фиксированные входные записи (replays), фиксированные seed'ы, mock-таймеры в unit-тестах, явное разделение «логики» и «I/O».

---

## 1. Высокоуровневая архитектура (High-Level Architecture)

### 1.1. Обзор модулей

Система состоит из шести модулей, каждый из которых — отдельный crate в Cargo workspace. Модули общаются через типизированные каналы (`tokio::sync::mpsc` для асинхронных потоков, `crossbeam` для синхронных hot-path) и через shared memory для тяжелых данных (видео-фреймы). Архитектура построена вокруг принципа single-direction data flow: видео → inference → tracker → commander → FC. Управляющие команды идут в обратном направлении по отдельному каналу, что исключает кольцевые зависимости на уровне данных.

**Модуль 1. `video-capture`.**
Отвечает за захват видео с Arducam UC-852 (USB UVC-совместимая камера). Использует V4L2 (Linux Video API) напрямую через `v4l2` crate или обертку над `libc`. Поддерживает MJPEG-декодирование (камера отдает MJPEG для экономии USB-пропускной способности) с использованием `rusty_ffmpeg` или собственной реализации на basis `jpeg-decoder`. Ключевая задача модуля — отдавать фреймы с измеримой латентностью, а не с максимальным FPS. Реализует backpressure: если downstream не успевает потреблять, старые фреймы дропаются, а не копятся в очереди. Опционально использует `dmabuf` для zero-copy передачи фрейма в инференс-модуль.

**Модуль 2. `cv-inference` (Rust-orchestrator + C++ RKNN-bridge).**
Rust-сторона оркеструет инференс, но сам инференс выполняется в отдельном C++ микросервисе через RKNN SDK (см. раздел «Управление гипотезами» — H-001). Связь между Rust-оркестратором и C++-мостом — через shared memory (memfd + ring buffer) или ZMQ over Unix socket. Rust-сторона: принимает фрейм, передает указатель на shared memory в C++, получает обратно массив bounding box'ов с confidence scores. Модель — YOLOv8n, конвертированная в RKNN формат и квантованная в INT8. Классы целей конфигурируются через TOML-файл (например, `person`, `vehicle`, `uas`).

**Модуль 3. `target-tracker`.**
Отвечает за удержание выбранной цели в кадре между детекциями. Внутри: Kalman filter для предсказания позиции цели, трекинг-алгоритм (DeepSORT для многоточного отслеживания или KCF/MOSSE как fallback при потере NPU). Получает detections от `cv-inference`, поддерживает state конкретной цели (позиция, скорость, размер bbox), выдает смещение цели относительно центра кадра (target_offset_x, target_offset_y). Также отвечает за detecцию потери цели (occlusion timeout, target drift beyond frame).

**Модуль 4. `fc-adapter` (HAL over MAVLink).**
Аппаратно-независимый слой над полетным контроллером. См. раздел 1.3 — это ключевая абстракция проекта.

**Модуль 5. `commander`.**
Высокоуровневая оркестрация: state machine режимов дрона (IDLE → ARMED → SCANNING → TARGET_SELECTED → TRACKING → LOST → RTH → ABORT), обработка команд оператора, применение anti-loop watchdog'ей, rate-limiting MAVLink-команд. Это «мозг» системы — единственный модуль, который имеет право инициировать MAVLink-команды изменения положения.

**Модуль 6. `common`.**
Общие типы: `Frame`, `Detection`, `TargetState`, `Attitude`, `RoiTarget`, `PositionTargetNED`. Также: error types (`thiserror`), конфигурация (`serde` + TOML), tracing-инфраструктура.

### 1.2. Data flow

```
   ┌─────────────────┐    Frame (NV12)    ┌──────────────────┐
   │  video-capture  │ ─────────────────► │  cv-inference    │
   │  (V4L2 + MJPEG) │   shared mem       │  (Rust + C++     │
   └─────────────────┘                    │   RKNN bridge)   │
                                          └────────┬─────────┘
                                                   │ Vec<Detection>
                                                   ▼
   ┌─────────────────┐  TargetState      ┌──────────────────┐
   │   commander     │ ◄───────────────  │ target-tracker   │
   │ (state machine, │                   │ (Kalman + SORT)  │
   │  watchdogs)     │                   └──────────────────┘
   └────────┬────────┘
            │ Commands (ROI, position target)
            ▼
   ┌─────────────────┐   MAVLink     ┌──────────────────┐
   │  fc-adapter     │ ────────────► │  ArduPilot FC    │
   │ (HAL trait)     │ ◄──────────── │ (SpeedyBee F405) │
   └─────────────────┘  Telemetry    └──────────────────┘
```

Поток данных строго однонаправленный сверху вниз. Команды от оператора (выбор цели, смена режима) приходят в `commander` через отдельный control-канал и не пересекаются с hot-path видеопотока.

### 1.3. Абстракция над полетным контроллером (HAL / Adapter pattern)

Чтобы система была agnostic к железу FC, вводится trait `FlightControllerAdapter` в crate `fc-adapter`. Любая реализация FC должна удовлетворять этому trait'у. Конкретная реализация выбирается в рантайме через конфигурацию (TOML-поле `fc.adapter = "ardupilot-mavlink"`).

```rust
// crates/fc-adapter/src/lib.rs (концептуально)

#[async_trait]
pub trait FlightControllerAdapter: Send {
    /// Установка ROI (Region of Interest) — куда смотрит камера/нос дрона.
    /// None — сброс ROI.
    async fn set_roi(&mut self, roi: Option<RoiTarget>) -> Result<(), FcError>;

    /// Стриминг целевой позиции в локальной NED-системе координат.
    /// Вызывается на 10 Hz, ArduPilot в режиме GUIDED держит позицию.
    async fn set_position_target_local_ned(
        &mut self,
        target: PositionTargetNED,
    ) -> Result<(), FcError>;

    /// Смена режима полета (GUIDED, LOITER, RTL, MANUAL).
    async fn set_mode(&mut self, mode: FlightMode) -> Result<(), FcError>;

    /// Текущее отношение (attitude) — roll/pitch/yaw + угловые скорости.
    /// Берется из кэша, обновляемого фоновым таском чтения MAVLink.
    fn attitude(&self) -> Attitude;

    /// Статус heartbeat — когда последний раз видели FC alive.
    fn heartbeat_status(&self) -> HeartbeatStatus;

    /// Arm/disarm.
    async fn arm(&mut self) -> Result<(), FcError>;
    async fn disarm(&mut self) -> Result<(), FcError>;

    /// Запрос текущей позиции в глобальных координатах (GPS).
    async fn global_position(&self) -> Option<GlobalPosition>;
}
```

**Конкретные реализации (планируемые):**

| Реализация | Транспорт | Назначение |
|---|---|---|
| `ArduPilotMavlinkAdapter` | MAVLink v2 over UART/USB | Production (SpeedyBee F405, любой Pixhawk) |
| `Px4MavlinkAdapter` | MAVLink v2 over UDP | Будущее (если мигрируем на PX4) |
| `SittlMavlinkAdapter` | MAVLink v2 over UDP | CI/CD и SITL-тесты |
| `MockFcAdapter` | In-memory | Unit-тесты, deterministic replays |

Ключевой момент: `commander` работает только с trait-объектом `Box<dyn FlightControllerAdapter>`, он ничего не знает о MAVLink, UART или конкретном FC. Это позволяет:
1. Тестировать всю логику commander'а с `MockFcAdapter` (детерминированно, без сети).
2. В SITL-тестах использовать `SittlMavlinkAdapter` (UDP, без реального железа).
3. В HITL/постановочных тестах использовать `ArduPilotMavlinkAdapter` (через UART к SpeedyBee F405).

Используется crate `mavlink` (https://crates.io/crates/mavlink) — он поддерживает MAVLink v1/v2, TCP/UDP/serial, асинхронный API через `tokio`. Согласно рекомендации девопса (см. совет #4), это проверенный выбор.

**Важно про PWM (ответ на совет #2 от девопса):** прямое управление серво через PWM с Orange Pi в архитектуре **запрещено**. Все управляющие команды идут исключительно через MAVLink (`MAV_CMD_DO_SET_ROI` для Gimbal/Camera ROI, `SET_POSITION_TARGET_LOCAL_NED` для наведения носа дрона на цель в режиме GUIDED). Это отвязывает систему от конкретного FC — любой ArduPilot-совместимый контроллер (SpeedyBee, Pixhawk, CubePilot) примет те же команды.

### 1.4. Механизм Anti-loop на уровне архитектуры

Это критическая подсистема. Большинство инцидентов с автономным наведением происходит не из-за ошибок в CV-модели, а из-за осцилляций в control loop. Архитектура закладывает пять уровней защиты.

**Уровень 1. Per-loop Watchdog-таймеры.**
Каждый цикл (video loop, inference loop, tracking loop, command loop) имеет свой watchdog. Если цикл не обновил свой «heartbeat» в течение заданного таймаута, `commander` переводится в деградированный режим. Конкретные значения (вынесены в конфиг):

| Watchdog | Таймаут | Действие при превышении |
|---|---|---|
| `video_loop_wdt` | 100 ms | Перейти в состояние `VIDEO_DEGRADED`, прекратить выдачу команд |
| `inference_loop_wdt` | 200 ms | Перейти в `INFERENCE_DEGRADED`, использовать tracker-only (без новых detections) |
| `tracking_loop_wdt` | 50 ms | Перейти в `TRACKING_DEGRADED`, заморозить последнюю команду |
| `command_loop_wdt` | 100 ms | Перейти в `ABORT`, инициировать RTH |
| `fc_heartbeat_wdt` | 1000 ms | Перейти в `ABORT`, инициировать RTH, логировать critical |

Реализация: `commander` держит `HashMap<WatchdogId, Instant>`, обновляемое каждым циклом через неблокирующий канал. Отдельный фоновый таск проверяет таймауты на 10 Hz.

**Уровень 2. State Machine с детерминированными переходами.**
Используется crate `statig` (или собственная реализация на enum'ах). Переходы между состояниями явно перечислены — никаких спонтанных прыжков. Например, из `TRACKING` нельзя напрямую попасть в `TRACKING` — это бессмысленно. Из `TRACKING` можно попасть только в: `LOST`, `TRACKING_DEGRADED`, `ABORT`, `TARGET_SELECTED` (если оператор выбрал новую цель). Не-явные переходы отклоняются с логированием.

```
State machine:
  IDLE ──arm──► ARMED ──scan──► SCANNING
                                   │
                                   ▼ (target selected)
                              TARGET_SELECTED
                                   │
                                   ▼ (lock acquired <1s)
                                TRACKING ◄───┐
                                   │         │
                                   ▼         │ (reacquired <2s)
                                  LOST ──────┘
                                   │
                                   ▼ (lost >2s)
                                  RTH
                                   │
                                   ▼ (operator override)
                                  IDLE / ABORT
```

**Уровень 3. Deadband и гистерезис.**
Самый частый баг (см. совет #3 от девопса): цель уходит из кадра → дрон вращается → камера сносит по yaw → цель снова появляется → дрон пытается выровняться → осцилляция. Решение:

- **Deadband в центре кадра:** если `|target_offset_x| < 5%` ширины кадра И `|target_offset_y| < 5%` высоты — команда на коррекцию **не посылается**. Это убирает micro-jitter.
- **Гистерезис на потерю цели:** detection пропал → не сразу `LOST`, а `TRACKING_DEGRADED` на 500 ms. Если за это время detection вернулся — остаемся в `TRACKING`. Это фильтрует flickering.
- **Гистерезис на восстановление:** из `LOST` в `TRACKING` переходим только если detection стабильный минимум 3 кадра подряд.
- **Bounding limits на команды:** max yaw rate 30°/s, max pitch rate 15°/s, max target offset 30% кадра. Если tracker выдает смещение больше 30% — это аномалия, команда клиппится и логируется warning.

**Уровень 4. Rate limiting и команда throttling.**
MAVLink-команды на FC посылаются с фиксированной частотой 10 Hz (не больше). Даже если `commander` хочет чаще — внутренний rate-limiter их дропает. Это предотвращает перегрузку FC и стабилизирует control loop. Реализация: токен-бакет в `fc-adapter`.

**Уровень 5. Oscillation Detector.**
Хранится ring buffer последних 30 команд (3 секунды при 10 Hz). Каждую секунду вычисляется «частота смены знака» по yaw-командам. Если sign-change rate > 0.5 (т.е. направление меняется чаще, чем раз в 2 команды) — это индикатор осцилляции. Действие: заморозить команды на 1 секунду, перевести `commander` в `TRACKING_DEGRADED`, логировать critical alert. Если за 5 секунд детектор сработал 3 раза — `ABORT` + RTH.

**Уровень 6. Safety Pilot Override (HARD).**
На уровне FC (ArduPilot) настраивается RC override: если safety pilot двигает стиком — авто-команды немедленно прекращаются, дрон переходит в ручной режим. Это последняя линия обороны, недоступная программному слою.

---

## 2. Декомпозированный Roadmap (Phased Approach)

Проект разбит на 9 фаз (0–8). Каждая фаза имеет четкий deliverable и KPI — фаза не считается завершенной, пока KPI не подтвержден. Оценки длительности — предварительные (T-shirt sizing: S=1 нед, M=2 нед, L=3–4 нед).

### Фаза 0: Foundation & Scaffolding (S, ~1 нед)

**Цель:** Создать скелет проекта, на котором можно вести разработку всех модулей параллельно.

**Задачи:**
- 0.1 Инициализировать Cargo workspace с крейтами `common`, `video-capture`, `cv-inference`, `target-tracker`, `fc-adapter`, `commander`, `cli`.
- 0.2 Настроить `rust-toolchain.toml` (stable, с компонентами `rustfmt`, `clippy`).
- 0.3 Настроить `deny.toml` (`cargo-deny` для проверки лицензий и уязвимостей).
- 0.4 Базовая CI: fmt check, clippy `-D warnings`, `cargo test`, `cargo deny check`.
- 0.5 Инициализировать `docs/` с шаблонами: `ARCHITECTURE.md`, `HYPOTHESES.md`, `KPI.md`, `SAFETY.md`.
- 0.6 Создать ADR-каталог `docs/ADR/` с шаблоном.
- 0.7 Настроить tracing-инфраструктуру (`tracing` + `tracing-subscriber`), structured logging в JSON.
- 0.8 Конфигурационная система: `serde` + TOML, hot-reload через `notify` crate (для dev-режима).
- 0.9 SITL Docker-образ: ArduPilot SITL + Gazebo Classic в `sim/sitl/`.

**Deliverable:** Пустой workspace, CI зеленая, SITL запускается локально одной командой (`docker compose up`).

**KPI фазы:**
- CI runtime < 5 минут.
- `cargo build --workspace` completes за < 30 секунд (empty crates).
- SITL запускается и отвечает на MAVLink heartbeat за < 30 секунд.
- `HYPOTHESES.md` содержит минимум 5 записей (базовые гипотезы).

### Фаза 1: Video Capture Pipeline (M, ~2–3 нед)

**Цель:** Получить стабильный видеопоток с Arducam UC-852 с измеримой латентностью.

**Задачи:**
- 1.1 Исследовать и зафиксировать гипотезы H-101..H-103 (см. HYPOTHESES.md): поддержка V4L2, оптимальный формат (MJPEG vs YUYV), возможность dmabuf.
- 1.2 Реализовать `video-capture` crate: V4L2 capture через `v4l2` crate.
- 1.3 Bring-up Arducam UC-852: проверить supported formats/resolutions (`v4l2-ctl --list-formats-ext`).
- 1.4 Реализовать MJPEG-декодер (через `rusty_ffmpeg` или `jpeg-decoder`).
- 1.5 Frame queue с backpressure: drop-old стратегия, не drop-new.
- 1.6 Измерительная инфраструктура: timestamp на каждом этапе (capture → decode → publish), метрики в tracing.
- 1.7 Тесты на synthetic V4L2 device (`vivid` kernel module) для CI.
- 1.8 Документация: `docs/video-pipeline.md` с замерами на реальном железе.

**Deliverable:** Бинарник `video-capture-test`, который выводит в stdout метрики латентности при работе с реальной камерой.

**KPI фазы:**
- End-to-end latency (capture → frame available to consumer) < 50 ms на 720p@30 FPS.
- Стабильные 30 FPS без дропов при длительности теста 5 минут.
- CPU utilization на Orange Pi 5 < 30%.
- Все unit-тесты на `vivid` проходят в CI.

### Фаза 2: CV/Inference (RKNN Bridge) (L, ~3–4 нед)

**Цель:** Запустить YOLOv8 inference на NPU RK3588S с целевой частотой.

**Задачи:**
- 2.1 Подтвердить/опровергнуть H-001 (см. HYPOTHESES.md): есть ли зрелые Rust-биндинги к RKNPU2 SDK? Если нет — принять ADR-0001 «RKNN C++ bridge микросервис».
- 2.2 Реализовать `rknn-bridge/` C++ микросервис: загружает RKNN-модель, принимает фреймы из shared memory, возвращает detections.
- 2.3 Конвертация модели: ONNX YOLOv8n → RKNN формат с INT8 квантованием (использовать `rknn-toolkit2` на x86 host).
- 2.4 Реализовать Rust-сторону `cv-inference`: shared memory протокол, клиент к C++-мосту.
- 2.5 Структура `Detection { bbox, class_id, confidence, timestamp }`.
- 2.6 NMS (non-maximum suppression) в Rust или в C++ — решить в ADR.
- 2.7 Бенчмарки: mAP на тестовом датасете (COCO subset), латентность на Orange Pi.
- 2.8 Fallback-режим: если RKNN недоступен, переключение на CPU-инференс (ONNX Runtime, медленно, но работает).
- 2.9 Replay-инфраструктура: запись фреймов на диск, воспроизведение для регрессионных тестов.

**Deliverable:** Бинарник `inference-test`, который читает видео-файл и выводит detections в JSON. C++-мост собирается и деплоится как systemd-юнит.

**KPI фазы:**
- Inference latency < 60 ms на 720p (на NPU).
- FPS ≥ 15 на Orange Pi 5.
- mAP > 0.70 на тестовом датасете (COCO val, выбранные классы).
- NPU utilization > 50% (контроль через `rknn_server` или sysfs).
- Падение качества не более 5% mAP по сравнению с FP16 ONNX-моделью.

### Фаза 3: Target Tracker (M, ~2–3 нед)

**Цель:** Удерживать выбранную цель в кадре между detections, переживать кратковременные occlusions.

**Задачи:**
- 3.1 Реализовать Kalman filter для оценки позиции и скорости цели (2D в координатах кадра).
- 3.2 Реализовать tracking-алгоритм: DeepSORT (primary) или KCF (fallback). Решение зафиксировать в ADR-0002.
- 3.3 State целевой_TRACK: `TargetState { id, bbox, velocity, confidence, last_seen }`.
- 3.4 Логика handoff: оператор выбирает detection → tracker захватывает (acquires lock). Замерять время от выбора до захвата.
- 3.5 Occlusion handling: при потере detection на N кадров — предсказывать позицию по Kalman. Через M кадров — `LOST`.
- 3.6 Anti-flicker: при возврате detection — проверять IoU с предсказанной позицией, иначе игнорировать (защита от ложных срабатываний).
- 3.7 Тесты на синтетических последовательностях (движущаяся точка с шумом, occlusions).
- 3.8 Тесты на реальных записях (если есть).

**Deliverable:** `tracker-test` бинарник, который читает detections из JSON и выводит `TargetState` в реальном времени.

**KPI фазы:**
- Tracking accuracy: среднее отклонение трека от ground-truth < 5% размера кадра.
- Время от выбора цели до захвата (lock acquisition) < 1 секунды.
- Recovery time после occlusion < 500 ms (если цель вернулась в кадр).
- Доля потерянных целей на тестовых последовательностях < 10%.

### Фаза 4: FC Adapter (MAVLink HAL) (M, ~2 нед)

**Цель:** Реализовать `ArduPilotMavlinkAdapter` и `SittlMavlinkAdapter`, доказать что команды проходят.

**Задачи:**
- 4.1 Подтвердить H-201 (см. HYPOTHESES.md): `mavlink` crate стабильна и поддерживает нужные сообщения.
- 4.2 Реализовать trait `FlightControllerAdapter` (раздел 1.3).
- 4.3 Реализовать `ArduPilotMavlinkAdapter`: MAVLink v2 over serial (`/dev/ttyUSB0` или `/dev/ttyACM0`).
- 4.4 Реализовать `SittlMavlinkAdapter`: MAVLink v2 over UDP (`127.0.0.1:14550`).
- 4.5 Фоновый таск чтения telemetry: heartbeat, attitude, global position. Кэш в `Arc<RwLock<...>>`.
- 4.6 Реализовать `set_position_target_local_ned` стриминг на 10 Hz.
- 4.7 Реализовать `set_roi` через `MAV_CMD_DO_SET_ROI`.
- 4.8 Реализовать `MockFcAdapter` для unit-тестов: in-memory, recording всех команд.
- 4.9 Integration тесты: запуск SITL в CI, отправка команд, проверка что ArduPilot их принял (через SITL telemetry).

**Deliverable:** `fc-adapter-test` бинарник, который коннектится к SITL и отправляет тестовые команды (set mode GUIDED, set ROI, stream position targets). В CI — green.

**KPI фазы:**
- Время от вызова trait-метода до отправки MAVLink-сообщения < 5 ms.
- Время от отправки команды до изменения attitude в SITL < 100 ms (end-to-end через симулятор).
- Heartbeat monitoring: detects FC loss за < 1 секунду.
- Все integration-тесты в CI проходят стабильно (10 запусков подряд без flakes).

### Фаза 5: Commander & State Machine (M, ~2–3 нед)

**Цель:** Собрать все модули в единую систему, реализовать anti-loop watchdog'и и state machine.

**Задачи:**
- 5.1 Реализовать state machine через `statig` crate (или собственная на enum'ах + ADR-0003).
- 5.2 Реализовать все watchdog'и (раздел 1.4, Уровень 1).
- 5.3 Реализовать deadband и гистерезис (Уровень 3).
- 5.4 Реализовать rate-limiter для команд (Уровень 4).
- 5.5 Реализовать oscillation detector (Уровень 5).
- 5.6 CLI для оператора: `cli` binary с командами `select-target <id>`, `set-mode <mode>`, `abort`.
- 5.7 Operator interface: gRPC или HTTP API для будущей интеграции с UI. На данном этапе — CLI достаточно.
- 5.8 Integration тесты: end-to-end SITL demo (синтетическая цель в Gazebo → tracking → команды на FC).
- 5.9 Stress-тесты: 30-минутный SITL-ран, замерять watchdog-триггеры.

**Deliverable:** `auto-targeting` бинарник, который запускается на Orange Pi, коннектится к камере и FC, и в SITL демонстрирует tracking синтетической цели.

**KPI фазы:**
- Ноль oscillation-detector срабатываний за 30 минут SITL.
- Watchdog-триггеры < 1 в час (в нормальном режиме).
- Время от выбора цели до первой команды на FC < 1 секунды.
- Все переходы state machine покрыты unit-тестами (100% coverage на transition logic).

### Фаза 6: Integration & SITL Validation (S, ~2 нед)

**Цель:** Доказать, что система работает end-to-end в симуляции стабильно.

**Задачи:**
- 6.1 Полный pipeline: video (или simulated video) → inference → tracker → commander → FC adapter → SITL.
- 6.2 Сценарии тестирования: статичная цель, движущаяся цель, occlusions, множественные цели, потеря цели.
- 6.3 Replay-инфраструктура: запись сессии, воспроизведение для регрессии.
- 6.4 Performance benchmarks в CI (nightly): latency, FPS, NPU utilization.
- 6.5 Документация: `docs/sitl-test-report.md` с результатами всех сценариев.
- 6.6 Code review всего проекта, рефакторинг hot-path.

**Deliverable:** SITL demo записан на видео, все тесты green, KPI-документ заполнен.

**KPI фазы:**
- End-to-end latency (capture → command sent) < 150 ms.
- Все KPI фаз 1–5 подтверждены в integration-сценариях.
- СITL-сценарии проходят в 95% запусков (допускается 5% flakes на симулятор).

### Фаза 7: HITL — Hardware-In-The-Loop (M, ~2–3 нед)

**Цель:** Проверить систему на реальном железе (Orange Pi + SpeedyBee) без полетов.

**Задачи:** см. раздел 4.

**Deliverable:** HITL-стенд собран, протокол испытаний заполнен, все критерии Flight Readiness пройдены.

**KPI фазы:**
- 8-часовой stability-ран без crash.
- Все watchdog'и протестированы (искусственно вызваны и восстановлены).
- Латентность на реальном железе подтверждена (те же KPI, что в фазе 6).

### Фаза 8: Flight Tests (L, ~3 нед)

**Цель:** Подтвердить работу системы в реальных полетах.

**Задачи:**
- 8.1 Ground tests: запуск системы на земле, проверка video/inference/tracking без полета.
- 8.2 Tethered flights: дрон на тросе, тестирование tracking в ограниченном пространстве.
- 8.3 Free flights with safety pilot: первый реальный полет, safety pilot на RC, ready к override.
- 8.4 Сбор метрик, анализ, фиксация гипотез в HYPOTHESES.md.
- 8.5 Flight test report: что работало, что нет, какие доработки нужны.

**Deliverable:** Flight test report, обновленные гипотезы, plan для итераций.

**KPI фазы:**
- Tracking success rate > 90% в реальных полетах (success = удержание цели в кадре > 10 секунд).
- Ноль oscillation-induced инцидентов (нет ситуаций, требующих safety pilot override из-за осцилляции).
- Все критические гипотезы подтверждены или опровергнуты с зафиксированными результатами.

---

## 3. CI/CD Pipeline для Rust & Edge-устройств

### 3.1. Общая структура пайплайна

CI/CD строится на GitHub Actions (или GitLab CI — выбор в ADR-0004). Пайплайн разделен на три типа: **PR-check** (быстрый, на каждый pull request), **Main-build** (на merge в main), **Nightly** (полные тесты, раз в сутки). Это разделение — стандартный паттерн для Rust-проектов, который балансирует скорость фидбека и глубину тестирования.

**PR-check** (target: < 5 минут):
- `cargo fmt --check` — проверка форматирования.
- `cargo clippy --workspace --all-targets -- -D warnings` — строгий линтер.
- `cargo test --workspace` — unit-тесты.
- `cargo deny check` — лицензии и уязвимости зависимостей.
- `cargo audit` — отдельная проверка CVE.
- Build check на x86_64 (host).

**Main-build** (target: < 15 минут):
- Всё из PR-check.
- Cross-compile под `aarch64-unknown-linux-gnu` (см. 3.2).
- Build C++ `rknn-bridge` под aarch64 (в Docker-контейнере с aarch64-тулчейном).
- SITL integration tests (короткий набор, smoke-тесты).
- Загрузка артефактов (Rust binary + C++ bridge binary + config templates) в GitHub Releases / artifact storage.

**Nightly** (target: < 1 час):
- Всё из Main-build.
- Полные SITL-сценарии (все тест-кейсы из `sim/scenarios/`).
- Performance benchmarks: латентность, FPS, NPU utilization — с трендом в GitHub Pages или отдельном dashboard.
- Длительные stress-тесты (30-минутный SITL-ран).
- `cargo miri` на safe-Rust крейтах (по возможности).

### 3.2. Кросс-компиляция под aarch64 Linux

Целевая платформа — Orange Pi 5 работает под Linux (вероятно Ubuntu 22.04 aarch64 или Armbian). Бинарники собираются на x86_64 dev-машине/CI-runner и деплоятся на Pi.

**Подход 1: `cross` tool.** Crate `cross` (https://github.com/cross-rs/cross) использует Docker-образы с преднастроенными тулчейнами. Для стандартных целей (включая `aarch64-unknown-linux-gnu`) работает из коробки. Минус: образы не всегда содержат нужные системные библиотеки (например, `librknnrt.so` для RKNN).

**Подход 2: Custom Docker image.** Собственный Dockerfile в `deploy/Dockerfile.cross` с `aarch64-linux-gnu-gcc`, нужными библиотеками (включая RKNPU2 SDK), и переменными окружения для `PKG_CONFIG` / `CMAKE_TOOLCHAIN_FILE`. Используется в CI:

```dockerfile
# deploy/Dockerfile.cross (концептуально)
FROM ubuntu:22.04
RUN apt-get update && apt-get install -y \
    gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
    cmake pkg-config git curl \
    libc6-dev-arm64-cross
# Установка Rust toolchain с aarch64 target
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add aarch64-unknown-linux-gnu
# Установка RKNPU2 SDK для кросс-линковки
COPY rknn-sdk /opt/rknn-sdk
ENV RKNN_SDK_PATH=/opt/rknn-sdk
# Конфигурация cargo для кросс-компиляции
COPY cargo-config.toml /root/.cargo/config.toml
```

В `.cargo/config.toml`:
```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
ar = "aarch64-linux-gnu-ar"
```

**Статическая vs динамическая линковка:** предпочитаем динамическую линковку с системными библиотеками (glibc) — это упрощает обновления и совместимо с библиотеками RKNPU2, которые сами по себе динамические. Мускул-статика здесь не подходит, потому что у RKNN SDK нет статических версий.

**Альтернатива для dev:** нативная сборка прямо на Orange Pi 5 (через SSH). Медленно (Pi 5 мощная, но не как x86 workstation), но удобно для итеративной разработки. Используется только для ad-hoc тестов, не для production-сборок.

### 3.3. Доставка бинарников на Orange Pi

**Подход: Ansible + SCP + systemd.**

Ansible playbook `deploy/ansible/deploy.yml`:
1. **Provisioning** (один раз): установка системных зависимостей (`apt install libusb-1.0-0 v4l-utils`), копирование `librknnrt.so` в `/usr/local/lib/`, запуск `ldconfig`.
2. **Deploy** (на каждый релиз): SCP бинарников в `/opt/auto-targeting/`, копирование config templates, перезапуск systemd-юнита.
3. **Health check**: HTTP endpoint `:8080/health` возвращает статус системы. Ansible ждет 5 секунд после рестарта, проверяет endpoint, откатывается на предыдущую версию при failure.

Systemd-юнит `auto-targeting.service`:
```ini
[Unit]
Description=Auto-Targeting Companion Computer
After=network.target
Wants=network.target

[Service]
Type=notify
WorkingDirectory=/opt/auto-targeting
ExecStart=/opt/auto-targeting/bin/auto-targeting --config /etc/auto-targeting/config.toml
Restart=on-failure
RestartSec=2
WatchdogSec=10
NotifyAccess=main

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/auto-targeting

# Лимиты ресурсов
LimitNOFILE=4096
MemoryMax=2G

[Install]
WantedBy=multi-user.target
```

Ключевые моменты:
- `Type=notify` + `WatchdogSec=10` — systemd сам становится watchdog'ом. Приложение должно дергать `sd_notify(WATCHDOG=1)` каждые < 10 секунд. Если не дернет — systemd убивает и рестартит. Это еще один уровень защиты (Уровень 7, на уровне ОС).
- `Restart=on-failure` + `RestartSec=2` — авто-восстановление после crash.
- Hardening: `ProtectSystem=strict` + `ReadWritePaths` — приложение может писать только в свою директорию.
- `MemoryMax=2G` — лимит памяти, защита от memory leak'ов (RK3588S имеет 4–16 GB в зависимости от конфигурации).

**Откат (rollback):** Ansible хранит предыдущую версию бинарников в `/opt/auto-targeting/previous/`. При failure health-check'а — symbolic link откатывается, сервис перезапускается с предыдущей версией, алерт в лог.

### 3.4. Интеграция тестов с SITL

SITL (Software In The Loop) — это симулятор ArduPilot, который запускает тот же firmware, что и на реальном FC, но в user-space на Linux, с симулированной физикой через Gazebo или внутренний физический движок ArduPilot.

**Docker-окружение для SITL:**

`sim/sitl/docker-compose.yml`:
```yaml
version: "3.8"
services:
  sitl:
    image: ardupilot/ardupilot-sitl:latest
    command: ["--model", "plane", "--instance", "1"]
    ports:
      - "5760:5760"   # MAVLink TCP
      - "5762:5762"   # MAVLink TCP #2
      - "5763:5763"   # MAVLink UDP
    volumes:
      - ./scenarios:/scenarios
  gazebo:
    image: gazebo/gazebo:classic
    ports:
      - "11345:11345"
    depends_on: [sitl]
```

**Integration-тесты в Rust** (`crates/cli/tests/sitl_integration.rs`):
1. Запускают SITL в Docker (через `testcontainers` crate).
2. Подключаются к SITL через `SittlMavlinkAdapter` (UDP).
3. Загружают синтетический видеопоток (replay-файл).
4. Запускают pipeline.
5. Проверяют: tracking-команды идут на FC, FC их принимает, attitude меняется в ожидаемом направлении.
6. Закрывают SITL.

**Сценарии** (`sim/scenarios/`):
- `scenario_static_target.json` — цель стоит на месте, дрон должен навестись.
- `scenario_moving_target.json` — цель движется по известной траектории.
- `scenario_occlusion.json` — цель пропадает на 3 секунды, возвращается.
- `scenario_multiple_targets.json` — 3 цели, оператор выбирает одну.
- `scenario_oscillation_test.json` — цель движется так, чтобы провоцировать осцилляции (резкие смены направления). Проверяет oscillation detector.

Каждый сценарий — replay-файл (последовательность фреймов в MJPEG) + JSON с expected events (например, "на 5-й секунде должен перейти в TRACKING").

---

## 4. HITL — Hardware-In-The-Loop Испытания

### 4.1. Зачем нужен HITL, если есть SITL?

SITL тестирует софт-стек, но не ловит проблемы реального железа: задержки UART, баги в конкретной версии ArduPilot firmware, особенности SpeedyBee F405 (например, USB-CDC vs UART throughput), температурные троттлинги Orange Pi, поведение реальной камеры. HITL — обязательный этап между SITL и реальными полетами.

### 4.2. Схема стенда (три уровня)

**HITL-T1 (Soft) — Orange Pi + SITL на PC:**
```
┌──────────────┐                  ┌──────────────────┐
│  PC (x86)    │  UDP MAVLink     │  Orange Pi 5     │
│  - SITL      │ ◄──────────────► │  - auto-targeting│
│  - Gazebo    │  Synthetic video │  - RKNN bridge   │
│              │ ────────────────►│                  │
└──────────────┘  (over Ethernet) └──────────────────┘
```
Тестируется: Rust-стек на реальном Orange Pi, латентность NPU inference, общая стабильность.
Не тестируется: реальный FC, реальные датчики.

**HITL-T2 (Hard) — Orange Pi + реальный FC + SITL:**
```
┌──────────────┐   USB        ┌─────────────────┐  UART/USB  ┌──────────────────┐
│  PC (x86)    │ ◄──────────► │ SpeedyBee F405  │ ◄────────► │  Orange Pi 5     │
│  - SITL      │   MAVLink    │  (ArduPilot)    │  MAVLink   │  - auto-targeting│
│  - Gazebo    │              │                 │            │  - RKNN bridge   │
└──────────────┘              └─────────────────┘            └──────────────────┘
```
FC работает как MAVLink-роутер: получает команды от Orange Pi по UART, передает в SITL на PC по USB. Сверху — telemetry обратно. Это тестирует реальный FC firmware, реальные MAVLink-сообщения, реальную UART-латентность.
Реальная камера Arducam подключена к Orange Pi (или синтетическое видео инжектится через `vivid` модуль).

**HITL-T3 (Full) — Orange Pi + реальный FC + реальные серво/моторы (без пропеллеров):**
```
┌──────────────────┐  UART/USB  ┌─────────────────┐  PWM  ┌──────────────┐
│  Orange Pi 5     │ ◄────────► │ SpeedyBee F405  │ ────► │ Серво/ESC    │
│  - auto-targeting│            │  (ArduPilot)    │       │ (без пропов) │
│  - RKNN bridge   │            │                 │       └──────────────┘
│  - камера        │            │  - GPS: mock    │
└──────────────────┘            │  - IMU: real    │
                                └─────────────────┘
```
FC работает автономно (не требует SITL): ArduPilot крутится на firmware, IMU реальные, GPS mock (или статичная позиция), моторы и серво реальные но без пропеллеров. Это проверяет: реальный PWM output, реальные реакции серв на команды, поведение FC в реальных режимах (GUIDED, LOITER).

### 4.3. Что тестируется на каждом уровне

| Аспект | SITL | HITL-T1 | HITL-T2 | HITL-T3 |
|---|---|---|---|---|
| Rust-логика | ✓ | ✓ | ✓ | ✓ |
| NPU инференс | — (mock) | ✓ | ✓ | ✓ |
| Латентность Orange Pi | — | ✓ | ✓ | ✓ |
| Реальная камера | — | optional | ✓ | ✓ |
| Реальный FC firmware | — | — | ✓ | ✓ |
| MAVLink over UART | — | — | ✓ | ✓ |
| PWM output | — | — | — | ✓ |
| Физика полета | симуляция | симуляция | симуляция | — (только地面) |

### 4.4. Flight Readiness Criteria (критерии перехода к полетам)

Чтобы перейти от HITL к реальным полетам, **все** следующие условия должны быть выполнены. Это gate — если хотя бы один пункт не пройден, полеты не начинаются.

**A. Software quality:**
- A1. Все unit-тесты проходят в CI (100% green).
- A2. Все SITL integration-тесты проходят в 95% запусков (10 rDS подряд).
- A3. Code coverage на critical-path модулях (`commander`, `fc-adapter`, `target-tracker`) > 80%.
- A4. `cargo clippy -D warnings` без исключений в critical-path.
- A5. `cargo audit` без критических CVE.

**B. Performance:**
- B1. End-to-end latency (capture → command sent) < 150 ms на реальном железе (HITL-T2).
- B2. Video latency < 50 ms.
- B3. Inference latency < 60 ms.
- B4. Lock acquisition time < 1 секунды.

**C. Stability:**
- C1. 8-часовой HITL-T2 stability-ран без crash (могут быть watchdog-триггеры, но с восстановлением).
- C2. Watchdog-триггеры < 1 в час в нормальном режиме.
- C3. Memory leak: < 50 MB growth за 8 часов.

**D. Safety:**
- D1. Каждый watchdog искусственно вызван и подтверждено восстановление.
- D2. Oscillation detector: протестирован с synthetic oscillation pattern, корректно замораживает команды.
- D3. RC override: подтверждено что при движении стиком safety pilot'а авто-команды немедленно прекращаются (< 200 ms).
- D4. RTH activation: при `ABORT` дрон переходит в RTL режим < 1 секунды.
- D5. Disarm command: командой `disarm` моторы останавливаются < 500 ms.

**E. Documentation:**
- E1. `HYPOTHESES.md` ревью: все критические гипотезы (помеченные как `CRITICAL`) подтверждены или есть mitigation plan.
- E2. `SAFETY.md` описывает процедуры emergency процедуры (что делать при watchdog, при loss of GPS, при loss of video).
- E3. Flight test plan написан и ревьюнут.
- E4. Pre-flight checklist готов.

**F. Hardware:**
- F1. SpeedyBee F405 прошит последним stable ArduPilot Plane firmware.
- F2. Orange Pi 5 настроен: user `autotarget`, ssh-доступ, systemd-юниты установлены и autostart.
- F3. Камера Arducam UC-852 протестирована на вибростенде (имитация полетной вибрации).
- F4. Батарея и BEC обеспечивают стабильное питание Orange Pi + FC при полном нагрузке (NPU + видео).

### 4.5. Тестовые сценарии HITL

Каждый сценарий имеет: имя, цель, шаги, expected behavior, pass criteria. Сценарии описаны в `sim/hitl/scenarios.md`.

**Сценарий HITL-001: Baseline stability.**
- Цель: 8-часовой рун без crash.
- Шаги: запустить систему, выбрать цель, удерживать 8 часов.
- Pass: нет crash, watchdog-триггеры < 8, memory growth < 50 MB.

**Сценарий HITL-002: Watchdog validation.**
- Цель: проверить все watchdog'и.
- Шаги: поочередно искусственно «убивать» video, inference, tracking, FC heartbeat. Восстанавливать.
- Pass: каждый watchdog срабатывает < заданного таймаута, система восстанавливается после устранения причины.

**Сценарий HITL-003: Oscillation injection.**
- Цель: проверить oscillation detector.
- Шаги: инжектить synthetic detections, которые провоцируют осцилляции (резкие смены позиции цели).
- Pass: detector срабатывает, команды замораживаются, через 1 секунду система возвращается в норму.

**Сценарий HITL-004: FC loss simulation.**
- Цель: проверить поведение при потере FC.
- Шаги: физически отключить UART-кабель от FC. Подождать 5 секунд. Подключить обратно.
- Pass: watchdog срабатывает за < 1 сек, переход в `ABORT`, RTH команда (после восстановления), корректное восстановление после reconnect.

**Сценарий HITL-005: Vibration test.**
- Цель: проверить работу камеры и CV при вибрации.
- Шаги: вибростенд имитирует полетную вибрацию (5–200 Hz). Запустить pipeline.
- Pass: tracking удерживает цель > 70% времени, latency не деградирует более чем на 30%.

---

## 5. Структура репозитория и файл гипотез

### 5.1. Структура Cargo workspace

```
auto-targeting/
├── Cargo.toml                          # [workspace] declaration
├── Cargo.lock
├── rust-toolchain.toml                 # fixed toolchain version
├── deny.toml                           # cargo-deny config
├── rustfmt.toml
├── clippy.toml
├── README.md
├── LICENSE
│
├── docs/
│   ├── ARCHITECTURE.md                 # high-level architecture
│   ├── HYPOTHESES.md                   # hypothesis log (см. 5.2)
│   ├── KPI.md                          # consolidated KPI dashboard
│   ├── SAFETY.md                       # safety procedures, emergency protocols
│   ├── VIDEO_PIPELINE.md               # latency measurements, camera tuning
│   ├── SITL_TEST_REPORT.md             # latest SITL results
│   ├── HITL_TEST_REPORT.md             # latest HITL results
│   ├── FLIGHT_TEST_REPORT.md           # latest flight test results
│   └── ADR/                            # Architecture Decision Records
│       ├── 0001-rknn-cpp-bridge.md
│       ├── 0002-tracking-algorithm-choice.md
│       ├── 0003-state-machine-library.md
│       ├── 0004-ci-platform-choice.md
│       └── TEMPLATE.md
│
├── crates/
│   ├── common/                         # shared types, errors, config
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types.rs                # Frame, Detection, TargetState, ...
│   │       ├── errors.rs               # thiserror definitions
│   │       └── config.rs               # serde + TOML
│   │
│   ├── video-capture/                  # V4L2 capture, MJPEG decode
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── v4l2_device.rs
│   │       ├── mjpeg_decoder.rs
│   │       └── frame_queue.rs
│   │
│   ├── cv-inference/                   # Rust orchestrator for RKNN bridge
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── bridge_client.rs        # IPC to C++ microservice
│   │       ├── shm_protocol.rs         # shared memory protocol
│   │       └── detection.rs
│   │
│   ├── target-tracker/                 # tracking algorithms
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── kalman.rs
│   │       ├── sort.rs                 # DeepSORT or fallback
│   │       └── target_state.rs
│   │
│   ├── fc-adapter/                     # MAVLink HAL trait + implementations
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── traits.rs               # FlightControllerAdapter trait
│   │       ├── ardupilot_mavlink.rs    # ArduPilotMavlinkAdapter
│   │       ├── sitl_mavlink.rs         # SittlMavlinkAdapter
│   │       ├── mock.rs                 # MockFcAdapter for tests
│   │       └── rate_limiter.rs
│   │
│   ├── commander/                      # top-level state machine + watchdogs
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── state_machine.rs        # statig-based FSM
│   │       ├── watchdogs.rs
│   │       ├── anti_loop.rs            # deadband, hysteresis, oscillation detector
│   │       └── commander.rs            # top-level orchestrator
│   │
│   └── cli/                            # binary entry point
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── args.rs                 # clap CLI
│           └── operator_cmd.rs         # select-target, set-mode, abort
│
├── rknn-bridge/                        # C++ microservice for NPU inference
│   ├── CMakeLists.txt
│   ├── README.md
│   ├── src/
│   │   ├── main.cpp
│   │   ├── rknn_model.cpp
│   │   ├── shm_server.cpp
│   │   └── nms.cpp
│   ├── include/
│   └── models/
│       └── yolov8n_int8.rknn           # compiled model (git-lfs)
│
├── sim/
│   ├── sitl/
│   │   ├── docker-compose.yml
│   │   ├── ardupilot.sitl.toml
│   │   └── README.md
│   ├── hitl/
│   │   ├── scenarios.md
│   │   ├── run_hitl_t1.sh
│   │   ├── run_hitl_t2.sh
│   │   └── run_hitl_t3.sh
│   └── scenarios/                      # replay files + expected events
│       ├── scenario_static_target.json
│       ├── scenario_moving_target.json
│       ├── scenario_occlusion.json
│       ├── scenario_multiple_targets.json
│       └── scenario_oscillation_test.json
│
├── deploy/
│   ├── ansible/
│   │   ├── inventory.yml
│   │   ├── deploy.yml
│   │   ├── provision.yml
│   │   └── roles/
│   │       ├── common/
│   │       ├── deploy_binary/
│   │       └── systemd_setup/
│   ├── systemd/
│   │   ├── auto-targeting.service
│   │   └── rknn-bridge.service
│   ├── Dockerfile.cross                # aarch64 cross-build image
│   └── scripts/
│       ├── flash_f405.sh               # ArduPilot firmware flash helper
│       └── healthcheck.sh
│
├── tests/                              # end-to-end integration tests
│   ├── e2e_sitl_tracking.rs
│   └── e2e_replay.rs
│
└── .github/
    ├── workflows/
    │   ├── ci.yml                      # PR-check + main-build
    │   ├── nightly.yml                 # full SITL + benchmarks
    │   └── release.yml                 # tag-triggered release
    └── PULL_REQUEST_TEMPLATE.md
```

### 5.2. Шаблон `HYPOTHESES.md`

Этот файл — живой документ. Каждая гипотеза имеет уникальный ID, проходит жизненный цикл `OPEN → TESTING → DONE`, и хранит результаты тестов. Критические гипотезы (помеченные `CRITICAL`) блокируют Flight Readiness (см. раздел 4.4, пункт E1).

```markdown
# HYPOTHESES.md — Лог архитектурных гипотез

Каждая гипотеза проходит цикл: формулировка → метод проверки → результат → статус.
Статусы: `OPEN` (не проверена) | `TESTING` (в процессе) | `CONFIRMED` (подтверждена) | `REFUTED` (опровергнута) | `MITIGATED` (опровергнута, но есть workaround).

Критические гипотезы (`CRITICAL` priority) блокируют переход на следующую фазу.

---

## H-001: Существуют зрелые Rust-биндинги к RKNPU2 SDK

- **Priority:** CRITICAL
- **Owner:** TBD
- **Created:** 2026-08-01
- **Phase:** 2 (CV/Inference)
- **Related ADR:** ADR-0001

**Гипотеза:**
Существуют production-ready Rust-крейты (например, `rknn-rs`, `rusty-rknn`), которые позволяют выполнять инференс YOLOv8 INT8 моделей на NPU RK3588S напрямую из Rust, без необходимости в C++ прослойке. Если это так — мы можем упростить архитектуру и удалить модуль `rknn-bridge`.

**Метод проверки:**
1. Поиск на crates.io и GitHub по ключевым словам `rknn`, `rknpu`, `rockchip npu`.
2. Для каждого найденного крейта: проверить количество stars, последний commit, открытые issue (особенно про RK3588S), документацию.
3. Если крейт выглядит зрелым — написать минимальный пример: загрузка YOLOv8n RKNN-модели, инференс одного фрейма.
4. Замерить latency на реальном железе (Orange Pi 5).
5. Сравнить с baseline: C++ RKNN bridge (из Phase 2).

**Ожидаемый результат (predict):**
Учитывая совет #1 от девопса, ожидаем что зрелых Rust-биндингов нет. Скорее всего найдем экспериментальные крейты с последним коммитом > 1 года назад и без поддержки RK3588S.

**Результат теста:**
*(заполняется после теста)*

**Статус:** OPEN

**Mitigation plan (если опровергнута):**
Если гипотеза опровергнута — реализуем C++ микросервис `rknn-bridge` как описано в архитектуре (раздел 1.1). Связь через shared memory. Это уже заложено в design.

---

## H-002: MAVLink v2 стриминг SET_POSITION_TARGET_LOCAL_NED на 10 Hz не вызывает перегрузки FC

- **Priority:** CRITICAL
- **Owner:** TBD
- **Created:** 2026-08-01
- **Phase:** 4 (FC Adapter)
- **Related ADR:** —

**Гипотеза:**
ArduPilot на SpeedyBee F405 (F4 @ 168 MHz) способен обрабатывать 10 Hz поток сообщений `SET_POSITION_TARGET_LOCAL_NED` без деградации других функций (стабилизация, GPS, telemetry). Задержка обработки одного сообщения < 10 ms.

**Метод проверки:**
1. Подключить SpeedyBee F405 к PC через USB (SITL не подходит — нужен реальный FC firmware).
2. Запустить ArduPilot Plane firmware (последний stable).
3. Запустить SITL на PC, который будет слать telelemtry обратно.
4. Запустить Rust-тест: стриминг `SET_POSITION_TARGET_LOCAL_NED` на 10 Hz в течение 5 минут.
5. Замерить: latency обработки (через timestamps в логах ArduPilot), CPU load FC (через `STATS` MAVLink message), пропуски heartbeat.
6. Повторить на 20 Hz, 50 Hz — найти предел.

**Ожидаемый результат:**
10 Hz должно работать без проблем. 50 Hz — вероятно перегрузка.

**Результат теста:**
*(заполняется после теста)*

**Статус:** OPEN

---

## H-003: Arducam UC-852 поддерживает V4L2 + MJPEG на 720p@30 FPS под Linux на Orange Pi 5

- **Priority:** HIGH
- **Owner:** TBD
- **Created:** 2026-08-01
- **Phase:** 1 (Video Capture)
- **Related ADR:** —

**Гипотеза:**
Arducam UC-852 определяется как стандартное UVC-устройство в Linux, поддерживает формат MJPEG на разрешении 720p (1280x720) при 30 FPS через V4L2 API. Не требует проприетарных драйверов.

**Метод проверки:**
1. Подключить камеру к Orange Pi 5.
2. `lsusb` — проверить что устройство опознано.
3. `v4l2-ctl --list-formats-ext -d /dev/video0` — вывести поддерживаемые форматы.
4. `ffplay /dev/video0` — визуально проверить изображение.
5. Замерить реальную latency через тестовый бинарник `video-capture-test`.

**Результат теста:**
*(заполняется после теста)*

**Статус:** OPEN

---

## H-101: (пример шаблона — удалить перед использованием)

- **Priority:** MEDIUM | HIGH | CRITICAL
- **Owner:** <имя>
- **Created:** <YYYY-MM-DD>
- **Phase:** <номер фазы>
- **Related ADR:** <ADR-XXXX или —>

**Гипотеза:**
<Четкая, проверяемая формулировка предположения. Должна быть либо подтверждена, либо опровергнута конкретным тестом. Избегать формулировок типа «работает хорошо» — только измеримо.>

**Метод проверки:**
<Пошаговый план тестирования: какие инструменты, какие метрики, какие pass/fail критерии. Должен быть воспроизводим — другой разработчик должен суметь повторить.>

**Ожидаемый результат (predict):**
<Что ожидаем увидеть. Это важно — фиксирует наши предположения ДО теста, чтобы избежать confirmation bias.>

**Результат теста:**
<Заполняется после теста: что измерили, какие цифры, какие observation.>

**Статус:** OPEN | TESTING | CONFIRMED | REFUTED | MITIGATED

**Mitigation plan (если опровергнута):**
<Что делаем, если гипотеза не подтверждается. Должно быть actionable.>
```

### 5.3. ADR-шаблон (для полноты)

```markdown
# ADR-XXXX: <название решения>

- **Status:** Proposed | Accepted | Deprecated | Superseded by ADR-YYYY
- **Date:** YYYY-MM-DD
- **Decision makers:** <имена>
- **Related hypotheses:** H-XXX

## Context
<Почему вообще возник этот вопрос? Какие альтернативы рассматривались? Какие ограничения?>

## Decision
<Какое решение принято. Конкретно, без воды.>

## Consequences
- **Positive:** <что получаем>
- **Negative:** <чем платим>
- **Neutral:** <наблюдения>

## Alternatives considered
<Кратко: какие еще варианты были, почему отвергнуты>
```

---

## Приложение A. Консолидированная таблица KPI

| KPI | Целевое значение | Фаза проверки | Где замеряется |
|---|---|---|---|
| Video latency (capture → frame available) | < 50 ms | Phase 1 | HITL-T1 |
| Inference latency (NPU) | < 60 ms | Phase 2 | HITL-T1 |
| Inference FPS | ≥ 15 | Phase 2 | HITL-T1 |
| mAP (test dataset) | > 0.70 | Phase 2 | CI benchmark |
| Tracking accuracy (offset from GT) | < 5% кадра | Phase 3 | SITL replay |
| Lock acquisition time | < 1 s | Phase 3, 5 | SITL |
| Recovery time after occlusion | < 500 ms | Phase 3 | SITL |
| MAVLink command send latency | < 5 ms | Phase 4 | Unit test |
| End-to-end command latency (capture → FC cmd) | < 150 ms | Phase 6 | HITL-T2 |
| Watchdog triggers (normal mode) | < 1 / hour | Phase 5, 7 | HITL-T2 |
| Oscillation events (SITL 30 min) | 0 | Phase 5 | SITL |
| Stability (HITL 8 hours, no crash) | PASS | Phase 7 | HITL-T2 |
| Memory growth (8 hours) | < 50 MB | Phase 7 | HITL-T2 |
| RC override response | < 200 ms | Phase 7 | HITL-T3 |
| RTH activation time | < 1 s | Phase 7 | HITL-T3 |
| Tracking success rate (real flight) | > 90% | Phase 8 | Flight test |

## Приложение B. Эпики для Jira/Linear

Каждая фаза из Roadmap — это Epic. Задачи внутри фазы — Story/Task. Watchdog'и, KPI, гипотезы — помечаются как `critical` label.

- **EPIC-0:** Foundation & Scaffolding
- **EPIC-1:** Video Capture Pipeline
- **EPIC-2:** CV/Inference (RKNN Bridge)
- **EPIC-3:** Target Tracker
- **EPIC-4:** FC Adapter (MAVLink HAL)
- **EPIC-5:** Commander & State Machine
- **EPIC-6:** Integration & SITL Validation
- **EPIC-7:** HITL Trials
- **EPIC-8:** Flight Tests

Cross-cutting epics:
- **EPIC-INFRA:** CI/CD Pipeline & Cross-compilation
- **EPIC-DOCS:** Documentation, ADRs, Hypothesis tracking
- **EPIC-SAFETY:** Safety procedures, Flight Readiness gate
