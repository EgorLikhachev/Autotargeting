# План перевода компонентов на шину Zenoh

**Дата:** 2026-08-18 · **Шина:** `event-bus` (D-014, Zenoh 1.x)
**Текущий статус:** детектор (TG26-35) **уже на шине** — `at/detections` +
`at/status/detector` работают в боевом контуре на RK3588 (293/293 событий).

## 0. Принципы миграции

1. **Кадры никогда не ездят по шине** — им принадлежит SHM-кольцо
   (`shmem-buffer`, D-013: 18 мкс/460 КБ против ~0.6–0.8 мс по шине).
   Шина — только события и лёгкие данные (≤10 КБ).
2. **Каждый компонент подключается независимо** — переводим по одному,
   старые связки работают параллельно до приёмки.
3. **Контракты фикшируются ДО кода**: сообщение = serde-тип в `event-bus`,
   тема = константа в `topics`; изменение контракта — ADR.
4. **Статус обязателен** для каждого компонента (`at/status/{c}`):
   fps/latency/ошибки — наблюдаемость с первого дня.
5. **Команды — reliable** (`at/commands`): zenoh put с reliability
   reliable (QoS-задача M0); телеметрия/детекции — best-effort.

## 1. Целевая топология тем

```text
камера ─▶ SHM ring (кадры, D-013) ─▶ детектор ─▶ at/detections   [ГОТОВО]
                                 └▶ рекордер  ─▶ at/status/recorder

MAVLink FC ◀──▶ fc-adapter ──▶ at/telemetry   (attitude/GPS/бат, 10–50 Гц)
                          ├──▶ at/fc_events   (режим, arm, heartbeat)
трекер:    at/detections  ─▶ at/tracks        (сопровождаемые цели)
классиф.:  at/detections  ─▶ at/classifications
commander: at/tracks + at/telemetry ─▶ at/commands (ROI/position-target)
конфиг:    at/config (+query) ─▶ at/config_ack
CLI/GCS:   подписан на всё, публикует at/commands
статусы:   at/status/{camera,recorder,detector,tracker,fc,bridge}
```

## 2. Фазы (по нарастанию риска)

### M0 — Контракты и QoS (2–3 ч)
- `event-bus`: `TelemetrySample` расширить (GPS/батарея/режим), добавить
  `CommandMsg`, `TrackMsg`, `FcEvent`, `StatusEnvelope`; поле версии `v`.
- reliable-put для `at/commands` (consistency zenoh).
- **Готовность:** serde-roundtrip всех типов (тест); reliable проверен A/B.

### M1 — Статусы существующих компонентов (3–4 ч, риск минимален)
- `camera_publisher` → `at/status/camera` (fps, дропы, формат, dims);
- `video-recorder` → `at/status/recorder` (кадры, jumps, путь файла);
- **Готовность:** подписчик на стенде видит статусы обоих.

### M2 — Трекер (8–10 ч, чистое добавление)
- Крейт `tracker`: подписка `at/detections` → Kalman+Hungarian
  (`target-tracker`) → `at/tracks`
  (`TrackMsg{track_id, bbox, velocity, class, conf, age, frame_seq}`);
- при необходимости пиксельного контекста — кадр из кольца по `frame_seq`.
- **Готовность:** треки из живых детекций на стенде; e2e в бюджете.

### M3 — fc-adapter: MAVLink ↔ шина (8–12 ч, safety-зона)
- MAVLink-нить → `at/telemetry` (rate-limited) + `at/fc_events`;
  подписка `at/commands` → MAVLink; heartbeat в `at/status/fc`.
- **Готовность:** телеметрия АП на шине; команда шины доходит до FC (SITL);
  тайминги в пределах watchdog-бюджетов commander.

### M4 — commander на шине (6–8 ч)
- Подписки: `at/tracks`, `at/telemetry`, `at/fc_events`;
  публикация `at/commands` — state machine/anti-loop/watchdogs без изменений.
- **Готовность:** замкнутый контур детекция→трек→команда целиком на шине
  (SITL-сценарий).

### M5 — Управление и конфигурация (6–8 ч)
- `cli`/REPL: подписка на статусы/треки/телеметрию, публикация команд;
- `at/config`: конфиг-сервис (zenoh query) + `at/config_ack`;
- GCS-мост (будущее): тот же контракт поверх TCP (R10) — смена кода не нужна.
- **Готовность:** оператор видит и управляет всем через шину.

### M6 — rknn-bridge: устранение base64-потолка (10–14 ч, performance)
- **Вариант B (предпочтителен)**: SCM_RIGHTS/SHM-мост (SDD §15 #2) — кадры
  мимо шины; bridge публикует `at/status/bridge`;
- Вариант A: кадры 640² через zenoh-cpp (замерить ~1.2 МБ payload!).
- **Готовность:** infer round-trip ≤35 мс (NPU 29 + накладные) →
  детектор ≥25 FPS (сейчас ~10).

## 3. Сквозные работы

- **systemd**: bus-listener стартует первым; каждый юнит — `--bus on|off`;
- **Мониторинг**: сводка по `at/status/*` (частоты, латентности, ошибки);
- **Тесты**: каждая фаза — интеграционный тест по паттерну детектора
  (in-process ring + bus, все ОС); M3/M4 — SITL-сценарии;
- **Откат**: feature-flag `--bus on|off` у каждого компонента до полной
  приёмки следующей фазы.

## 4. Оценки и порядок

| Фаза | Компонент | Оценка | Риск | Зависимость |
|---|---|---|---|---|
| M0 | контракты+QoS | 2–3 ч | — | — |
| M1 | статусы camera/recorder | 3–4 ч | мин. | M0 |
| M2 | трекер | 8–10 ч | низкий | M0 |
| M3 | fc-adapter | 8–12 ч | **средний** (safety) | M0 |
| M4 | commander | 6–8 ч | **средний** | M2+M3 |
| M5 | cli+config | 6–8 ч | низкий | M1–M4 |
| M6 | bridge-путь | 10–14 ч | средний | независимая |
| | **Итого** | **~44–59 ч** | | |

**Параллелизация:** M1 ∥ M2 ∥ M6 независимы; M3 стартует после M0;
M4 — после M2+M3; M5 замыкает. Критический путь: M0→M3→M4 (~17–23 ч).
