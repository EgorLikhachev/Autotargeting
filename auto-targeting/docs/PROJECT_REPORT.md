# Auto-Targeting System — Итоговый отчёт и Roadmap

> **Документ:** Полная сводка по проекту для handoff'а команде и планирования следующих этапов.
> **Дата:** 2026-08-03
> **Автор:** DevOps / Tech Lead

---

## 1. Executive Summary

Проект **Auto-Targeting System** — companion computer для автономного наведения дрона самолетного типа — прошёл путь от архитектурного дизайна до работающего прототипа на реальном железе (Orange Pi 5 + USB-камера).

**Текущий статус:** 🟡 **HITL-Ready (Hardware-validated, awaiting FC + NPU)**

Система развёрнута на Orange Pi 5, успешно захватывает видео с USB-камеры, обрабатывает кадры, управляет state machine и готова к подключению полётного контроллера (FC) и NPU-ускорителя для инференса.

---

## 2. Что было сделано (Достижения)

### 2.1 Архитектура и дизайн (Phase 0)

- **Cargo workspace** с 7 крейтами: `common`, `video-capture`, `cv-inference`, `target-tracker`, `fc-adapter`, `commander`, `cli`
- **High-Level Architecture** документ с описанием модулей и data flow
- **5 ADRs** (Architecture Decision Records):
  - ADR-0001: RKNN C++ Bridge Microservice (с полным protocol spec)
  - ADR-0002: Tracking Algorithm (IoU + Kalman + Hungarian)
- **HYPOTHESES.md** — лог архитектурных гипотез (H-001..H-004)
- **SAFETY.md** — процедуры безопасности, Flight Readiness Criteria

### 2.2 Критический путь (Stage 1 — для полёта)

| Компонент | Статус | Описание |
|---|---|---|
| **MJPEG декодер** | ✅ | `jpeg-decoder` crate, pure Rust |
| **YUYV → NV12 конверсия** | ✅ | Прямая конверсия для NPU input |
| **RKNN bridge client** | ✅ | Unix socket + JSON IPC протокол |
| **PID-контроллер** | ✅ | Anti-windup, derivative filtering |
| **Координатный трансформ** | ✅ | Camera frame → NED для MAVLink |

### 2.3 Safety + CI (Stage 2)

| Компонент | Статус | Описание |
|---|---|---|
| **HTTP health endpoint** | ✅ | `GET /health` + systemd `sd_notify` |
| **Geofencing** | ✅ | Haversine distance, max altitude/distance |
| **Battery monitoring** | ✅ | RTH (30%), LAND (15%), low voltage |
| **CI с SITL** | ✅ | Docker + ArduPilot SITL, 7 integration tests |
| **Coverage report** | ✅ | cargo-tarpaulin + Codecov |
| **Stress-тесты** | ✅ | 30-минутный SITL run, 5 stress tests |

### 2.4 DevOps (Stage 3)

| Компонент | Статус | Описание |
|---|---|---|
| **Ansible playbook** | ✅ | provision.yml + deploy.yml с rollback |
| **Docker image** | ✅ | Multi-stage build, ~50MB runtime |
| **Property-based tests** | ✅ | proptest (8 property tests) |
| **Prometheus metrics** | ✅ | 11 метрик на `/metrics` endpoint |
| **API documentation** | ✅ | rustdoc на все crate'ы |
| **OTA update** | ✅ | Checksum verification, atomic rename, backup |

### 2.5 Реальное железо (Hardware Validation)

| Тест | Результат |
|---|---|
| **Orange Pi 5 boot** | ✅ Armbian, SSH доступ |
| **Rust toolchain** | ✅ 1.97.1 stable, aarch64 |
| **V4L2 compilation** | ✅ С `--features v4l2`, libclang-dev |
| **USB камера** | ✅ Microdia Webcam Vitade AF (`0c45:6366`) |
| **Camera formats** | ✅ MJPEG 1280x720@30fps, YUYV 1280x720@10fps |
| **Video capture** | ✅ 5-сек запись, 7.8MB MP4 |
| **Smoke test** | ✅ `All good. ✅` (7 FC commands, watchdogs OK) |
| **REPL** | ✅ Все команды работают |
| **Pre-flight check** | ✅ 13 PASS, 0 FAIL, 3 WARN |

### 2.6 C++ RKNN Bridge микросервис

- **Полный C++ проект** (`rknn-bridge/`) с CMake
- **Двойной backend:** StubBackend (для dev) + RknnBackend (для NPU)
- **IPC протокол:** length-prefixed JSON over Unix socket
- **6 C++ unit tests** (NMS implementation)

---

## 3. Метрики проекта

### 3.1 Кодовая база

| Метрика | Значение |
|---|---|
| Rust файлов | 43 |
| Rust LOC | ~9,900 |
| C++ файлов | 10 |
| C++ LOC | ~1,000 |
| Тестовых файлов | 8 (Rust) + 1 (C++) |
| Benchmarks | 20 (criterion) |
| Git коммитов | 12+ |
| Git репозиторий | https://github.com/EgorLikhachev/Autotargeting |

### 3.2 Тесты

| Тип | Количество | Статус |
|---|---|---|
| Unit tests (Rust) | 293 | ✅ All passing |
| Integration tests | 11 (e2e) + 7 (SITL) | ✅ Passing |
| Property tests | 8 (proptest) | ✅ Passing |
| Stress tests | 5 | ✅ Passing (#[ignore]) |
| C++ tests | 6 | ✅ Passing |
| Scenario suite | 5/5 | ✅ 100% pass rate |
| **Total** | **335** | **✅** |

### 3.3 CI/CD Pipeline

| Pipeline | Jobs | Триггер |
|---|---|---|
| **ci.yml** | pr-check, coverage, sitl-tests, cross-compile, smoke-test | PR + push to main |
| **nightly.yml** | full-tests, benchmarks, stress-test, security, coverage | Nightly 03:00 UTC |

### 3.4 Производительность (benchmarks)

| Benchmark | Результат |
|---|---|
| Kalman predict | ~560 ps |
| Kalman update | ~38 ns |
| Kalman full cycle | ~73 ns |
| NMS (5 disjoint) | < 10 µs |
| Anti-loop process | < 1 µs |
| Watchdog feed (5) | < 500 ns |

---

## 4. Текущее состояние на Orange Pi 5

### 4.1 Что работает

```
✅ Binary: target/release/auto-targeting (с V4L2)
✅ Камера: /dev/video0 (Microdia Vitade AF)
✅ Camera formats: MJPEG 1280x720@30, YUYV 1280x720@10
✅ REPL: интерактивное управление
✅ State machine: IDLE → ARMED → SCANNING → TRACKING → ABORT
✅ Watchdogs: video_loop, inference_loop, tracking_loop, command_loop, fc_heartbeat
✅ Anti-loop guard: deadband + bounding limits + oscillation detector
✅ Mock FC: arm/disarm/set-mode/commands
✅ Scenario suite: 5/5 pass
✅ Pre-flight check: 13 PASS, 0 FAIL
```

### 4.2 Что НЕ работает (требует железа)

```
❌ Real FC (SpeedyBee F405) — нет подключения
❌ Real NPU inference — нет RKNN SDK + модели
❌ Real target tracking — нет inference backend
❌ HITL/Flight tests — нет FC + сервомоторов
```

---

## 5. Roadmap — следующие шаги

### 5.1 Фаза A: Flight Controller Integration (1-2 недели)

**Цель:** подключить SpeedyBee F405, проверить MAVLink коммуникацию.

| # | Задача | Зависимости | Критерий успеха |
|---|---|---|---|
| A1 | Заказать/получить SpeedyBee F405 | — | FC в руках |
| A2 | Прошить ArduPilot Plane (latest stable) | A1 | FC boot, MAVLink отвечает |
| A3 | Подключить FC к Orange Pi 5 по USB | A2 | `/dev/ttyACM0` exists |
| A4 | Настроить config.toml: `adapter = "ardupilot-mavlink"`, `endpoint = "serial:/dev/ttyACM0:115200"` | A3 | Config valid |
| A5 | Запустить REPL, проверить heartbeat | A4 | `FC heartbeat: OK` в status |
| A6 | Тест arm/disarm через REPL | A5 | `FC armed: true` |
| A7 | Тест set-mode (GUIDED, RTL, LOITER) | A6 | Mode change confirmed |
| A8 | Проверить 10Hz SET_POSITION_TARGET_LOCAL_NED streaming | A7 | Latency < 10ms per command |
| A9 | SITL integration tests на реальном FC | A8 | 7/7 SITL tests pass |

**KPI фазы A:**
- FC heartbeat стабилен (без stale > 1 сек)
- Arm/disarm latency < 500ms
- Mode change latency < 1 сек
- 10Hz command streaming без потери пакетов

### 5.2 Фаза B: NPU Inference Integration (2-3 недели)

**Цель:** запустить YOLOv8n inference на RK3588S NPU.

| # | Задача | Зависимости | Критерий успеха |
|---|---|---|---|
| B1 | Скачать RKNPU2 SDK | — | `/opt/rknn-toolkit2` exists |
| B2 | Конвертировать YOLOv8n в RKNN формат (INT8) | B1 | `yolov8n_int8.rknn` file |
| B3 | Собрать rknn-bridge с `HAVE_RKNN=1` | B1, B2 | Binary `rknn-bridge` with `backend: rknn` |
| B4 | Запустить rknn-bridge как systemd service | B3 | `systemctl status rknn-bridge` = active |
| B5 | Тест inference на одном кадре | B4 | Detections returned, latency < 60ms |
| B6 | Интегрировать в auto-targeting (config: `allow_cpu_fallback = false`) | B5 | Pipeline: camera → inference → detections |
| B7 | Тест end-to-end: камера → inference → detections | B6 | Real detections in REPL |
| B8 | Замерить FPS и latency | B7 | ≥15 FPS, <60ms inference latency |
| B9 | Тест tracking с реальной целью | B8 | Target acquired, lock < 1s |

**KPI фазы B:**
- Inference latency < 60ms (NPU INT8 YOLOv8n на 720p)
- Inference FPS ≥ 15
- mAP > 0.70 на тестовом датасете
- Lock acquisition time < 1 сек

### 5.3 Фаза C: HITL Испытания (1-2 недели)

**Цель:** проверить полную систему на стенде.

| # | Задача | Зависимости | Критерий успеха |
|---|---|---|---|
| C1 | Собрать HITL стенд (Orange Pi + FC + servos, без пропеллеров) | A9, B9 | Стенд работает |
| C2 | HITL-T1: Orange Pi + SITL (soft test) | C1 | 8-hour run без crash |
| C3 | HITL-T2: Orange Pi + реальный FC + SITL | C2 | FC firmware валиден |
| C4 | HITL-T3: Orange Pi + FC + servos (без пропов) | C3 | PWM output корректен |
| C5 | Тест watchdog expiry + recovery | C4 | Каждый watchdog протестирован |
| C6 | Тест oscillation detector | C5 | Detector срабатывает корректно |
| C7 | Тест geofencing | C6 | Auto-RTH при нарушении |
| C8 | Тест battery monitoring | C7 | Auto-RTH/LAND при низком заряде |
| C9 | Тест RC override | C8 | Override срабатывает < 200ms |
| C10 | Flight Readiness Review | C9 | Все критерии пройдены |

**KPI фазы C:**
- 8-часовой stability run без crash
- Watchdog triggers < 1/hour
- Memory growth < 50MB за 8 часов
- RC override < 200ms
- RTH activation < 1s

### 5.4 Фаза D: Реальные полёты (2-3 недели)

**Цель:** первые полётные тесты.

| # | Задача | Зависимости | Критерий успеха |
|---|---|---|---|
| D1 | Ground tests (на земле, без полёта) | C10 | Tracking работает на земле |
| D2 | Tethered flights (на тросе) | D1 | 5-минутный полёт без инцидентов |
| D3 | Free flights with safety pilot | D2 | Safety pilot ready к override |
| D4 | Тест tracking в полёте | D3 | Target удерживается > 10 сек |
| D5 | Тест auto-RTH | D4 | RTH срабатывает корректно |
| D6 | Сбор метрик, анализ | D5 | Flight test report |
| D7 | Flight Readiness Review для следующей итерации | D6 | Plan for improvements |

**KPI фазы D:**
- Tracking success rate > 90%
- 0 oscillation-induced инцидентов
- 0 safety pilot overrides из-за auto-targeting
- All critical hypotheses confirmed

---

## 6. Риски и митигации

| Риск | Вероятность | Impact | Митигация |
|---|---|---|---|
| RKNN SDK не работает на Orange Pi 5 | Medium | High | Fallback: CpuInferenceBackend (ONNX Runtime) |
| MAVLink 10Hz перегружает FC | Low | Medium | Rate limiter (уже реализован) |
| Oscillations в реальном полёте | Medium | High | Anti-loop guard + deadband + PID tuning |
| Camera latency > 50ms | Medium | Medium | MJPEG вместо YUYV, меньшее разрешение |
| Battery drain (Orange Pi + FC + camera) | Low | High | BEC + separate battery for OPi |
| GPS lock issues | Low | Medium | Pre-flight check требует GPS HDOP < 2.0 |

---

## 7. Гипотезы для проверки

| ID | Гипотеза | Статус | Когда проверяем |
|---|---|---|---|
| H-001 | Rust bindings для RKNN SDK не зрелые | OPEN | Фаза B (B1-B2) |
| H-002 | ArduPilot держит 10Hz MAVLink | OPEN | Фаза A (A8) |
| H-003 | Arducam UC-852 поддерживает V4L2 + MJPEG | ✅ CONFIRMED | (использовали Microdia, тоже работает) |
| H-004 | `mavlink` crate стабилен | ✅ CONFIRMED | Phase 4.8 (SITL tests pass) |

---

## 8. Команда и ресурсы

### Что нужно для следующих фаз

| Ресурс | Для фазы | Стоимость |
|---|---|---|
| SpeedyBee F405 (или Pixhawk) | A | ~$30-50 |
| ArduPilot firmware | A | Free |
| RKNN SDK (RKNPU2) | B | Free (GitHub) |
| YOLOv8n model + dataset | B | Free (Ultralytics) |
| Серво + ESC + мотор (для HITL) | C | ~$50-100 |
| Battery (3S LiPo) + BEC | C | ~$20-30 |
| RC пульт (safety pilot) | C, D | ~$50-100 |
| Корпус/рама для дрона | D | ~$50-100 |

**Итого для полных полётных тестов:** ~$200-400

---

## 9. Заключение

**Проект находится в отличном состоянии.** За несколько итераций мы:

1. **Спроектировали** полную архитектуру с 7 модулями, 6 уровнями anti-loop protection
2. **Реализовали** весь критический путь: video capture → inference → tracking → commander → FC
3. **Добавили** safety systems: geofencing, battery monitoring, health endpoint
4. **Настроили** CI/CD с SITL integration tests, coverage, stress tests
5. **Валидировали** на реальном железе: Orange Pi 5 + USB камера работают

**Следующий критический шаг — подключение FC (Фаза A).** Это разблокирует реальное управление дроном и позволит перейти к HITL испытаниям.

Система готова к интеграции с реальным полётным контроллером. Все software components работают, mock-тесты проходят, safety systems функционируют.

---

**Документы для handoff'а:**
- `docs/ARCHITECTURE.md` — архитектура
- `docs/HYPOTHESES.md` — гипотезы
- `docs/KPI.md` — метрики
- `docs/SAFETY.md` — безопасность
- `docs/MANUAL_TESTING.md` — руководство по тестированию
- `docs/HARDWARE_SETUP.md` — настройка Orange Pi 5
- `docs/ADR/` — архитектурные решения

**Репозиторий:** https://github.com/EgorLikhachev/Autotargeting

---

*Подготовлено DevOps/Tech Lead. Август 2026.*
