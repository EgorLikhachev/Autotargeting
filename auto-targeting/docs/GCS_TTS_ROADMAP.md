# Roadmap: GCS-TTS (UDP/MAVLink-клиент + голосовая озвучка)

> **Статус:** План утверждён.
> **Дата плана:** 2026-08-05
> **Тип проекта:** Самостоятельный, обособленный git-репозиторий. Не связан с другими проектами.

---

## 1. Постановка задачи (ТЗ)

1. Поднять симулятор **ArduPilot SITL (ArduCopter)**, отдающий MAVLink на UDP `127.0.0.1:14550`.
2. Написать на **C++17 + Qt 6** программу, которая подключается к SITL и принимает поток телеметрии.
3. Сделать отдельный модуль **голосовой озвучки** ключевых событий.

**Требования к программе**
- Транспорт UDP, парсинг через `mavlink_c_library_v2`.
- Архитектура: `транспорт → парсер MAVLink → доменная модель → TTS`. Слои разнесены по разным классам/файлам.

**Требования к модулю озвучки**
- TTS-бэкенд под Linux.
- Минимальный набор сценариев:
  - смена режима полёта;
  - arm / disarm;
  - падение батареи ниже warning / critical порогов;
  - входящие `STATUSTEXT` уровня WARNING+ — дословно;
  - спич-«статус» по горячей клавише: высота, скорость, заряд.
- **Антиспам:** одно и то же событие не чаще раза в N секунд (значение в конфиге).
- Очередь TTS **не блокирует** приём телеметрии.

**Технические требования**
- C++17, Qt 6.5+, CMake 3.20+.
- Ubuntu 22.04 / 24.04. Кроссплатформенность не требуется.

---

## 2. Структура проекта (отдельный репозиторий)

**Расположение:** `C:/Users/Egorl/OneDrive/Documents/zAiProj/verus/gcs-tts/` (рядом с `Autotargeting/`, обособленно).

**Git:** свежий `git init`, своя история, свой CMake-проект. SITL поднимается **штатно** по документации ArduPilot (без форка/модификации их исходников).

| Решение | Выбор |
|---|---|
| Проект | Отдельный git-репозиторий `gcs-tts` |
| GUI | **Без GUI** — `QCoreApplication` (Qt как event loop + сеть + потоки). Вывод телеметрии в лог/консоль. |
| TTS-бэкенд | **RHVoice** (офлайн, русский, опенсорс) под абстракцией `TtsBackend` (espeak-ng как fallback). |
| MAVLink-парсер | `mavlink/c_library_v2` (vendored), диалект `ardupilotmega`, как требует ТЗ. |
| SITL | Штатная установка ArduPilot по официальной документации (см. ссылки в ТЗ). |

### Структура каталогов
```
gcs-tts/
├─ CMakeLists.txt              (CMake 3.20+, Qt6, C++17, -Wall -Wextra)
├─ README.md                   (описание, сборка, запуск, E2E-сценарий)
├─ .gitignore
├─ config/
│  └─ gcs.example.toml         (порты, пороги battery, antispam N, tts cmd)
├─ third_party/
│  └─ mavlink/                 (vendored c_library_v2: common/, ardupilotmega/)
├─ scripts/
│  ├─ setup_sitl.sh            (инструкция/скрипт установки ArduCopter SITL)
│  └─ run_sitl.sh              (запуск SITL с --out=udp:127.0.0.1:14550)
├─ src/
│  ├─ main.cpp
│  ├─ Config.{h,cpp}           (TOML via toml++)
│  ├─ transport/
│  │  └─ UdpTransport.{h,cpp}  (QUdpSocket, bind 0.0.0.0:14550)
│  ├─ mavlink/
│  │  ├─ MavlinkParser.{h,cpp} (mavlink_parse_char, MAVLINK_COMM_0)
│  │  └─ MessageCodec.{h,cpp}  (encode HEARTBEAT, REQUEST_DATA_STREAM)
│  ├─ domain/
│  │  ├─ TelemetryState.{h,cpp}
│  │  ├─ EventDetector.{h,cpp}
│  │  └─ TelemetryEvent.h
│  ├─ tts/
│  │  ├─ TtsBackend.h          (абстракция)
│  │  ├─ RhVoiceBackend.{h,cpp}
│  │  ├─ EspeakNgBackend.{h,cpp}   (fallback)
│  │  ├─ SpeechQueue.{h,cpp}       (рабочая нить, неблокирующая)
│  │  └─ AntiSpamFilter.{h,cpp}
│  ├─ announce/
│  │  └─ StatusAnnouncer.{h,cpp}
│  └─ hotkey/
│     └─ HotkeyController.{h,cpp}  (stdin REPL + опц. /dev/input)
└─ tests/                      (QTest: парсер, детектор, антиспам)
   ├─ test_mavlink_parser.cpp
   ├─ test_event_detector.cpp
   └─ test_antispam.cpp
```

---

## 3. Целевая архитектура

Слои строго разнесены по отдельным классам/файлам (требование ТЗ):

```
 QUdpSocket ─► UdpTransport ─► MavlinkParser (c_library_v2)
                                         │ typed signals
                                         ▼
                               TelemetryState (домен)
                                         │
                          EventDetector ─► TelemetryEvent
                                         │
                          AntiSpamFilter (N сек / тип события)
                                         ▼
                            SpeechQueue (рабочая нить, неблокирующая)
                                         ▼
                          TtsBackend → RhVoiceBackend (QProcess)
                          StatusAnnouncer ◄── HotkeyController (stdin / /dev/input)
```

**Принципы:**
- Транспорт ничего не знает про MAVLink — только «принял байты → эмитнул сигнал».
- Парсер ничего не знает про сеть — только «байты → типизированные сигналы».
- Доменная модель ничего не знает про TTS — только «сигналы → состояние → события».
- TTS-слой ничего не знает про MAVLink — только «текст → речь».
- Антиспам стоит между детектором и очередью (можно отключать для критических событий).
- Очередь TTS в отдельной нити (`QThread`); приём телеметрии — в event loop основного потока.

---

## 4. Итерации (6 шт.)

### Итерация 0 — Инфраструктура: репозиторий, SITL/ArduCopter, скелет сборки
*(оценка: ~1.5 дня, диапазон 1.0–2.5)*

**Задачи:**
- `git init` в `gcs-tts/`, `.gitignore` (build/, IDE, Qt-кэш), `README.md`-скелет.
- Установить ArduPilot SITL штатно по документации (см. ссылки в ТЗ):
  - `git clone https://github.com/ArduPilot/ardupilot` (внешне от проекта, не часть репозитория).
  - Установить зависимости через `Tools/environment_install/install-prereqs-ubuntu.sh`.
  - Собрать `ArduCopter`: `cd ardupilot/ArduCopter && ../Tools/autotest/sim_vehicle.py -w`.
- `scripts/setup_sitl.sh` — автоматизация установки/сборки SITL (idempotent).
- `scripts/run_sitl.sh` — запуск с `--out=udp:127.0.0.1:14550` (и нужными параметрами для arm без RC).
- Vendored `mavlink/c_library_v2` в `third_party/mavlink/`.
- Скелет `CMakeLists.txt`: `find_package(Qt6 COMPONENTS Core Network REQUIRED)`, C++17, target `gcs-tts`, опция `BUILD_TESTS`.
- Smoke-тест: `QCoreApplication` + `QUdpSocket` слушает 14550, печатает «получено N байт».

**Артефакты:**
- Инициализированный git-репозиторий `gcs-tts/`.
- `scripts/setup_sitl.sh`, `scripts/run_sitl.sh`.
- `CMakeLists.txt`, `src/main.cpp` (smoke), `third_party/mavlink/`.

**Критерий готовности (DoD):**
- ✅ `./scripts/run_sitl.sh` запускает ArduCopter SITL, MAVLink идёт на UDP 127.0.0.1:14550.
- ✅ Бинарь `gcs-tts` собирается и при запуске печатает счётчик принятых байт на порту 14550.

**Риск:** первая сборка ArduPilot занимает 10–20 мин (норма), нужен интернет и ~3 ГБ для зависимостей.

---

### Итерация 1 — Транспорт + парсер MAVLink
*(оценка: ~2.5 дня, диапазон 2–4)*

**Задачи:**
- `UdpTransport`: бинд `QUdpSocket("0.0.0.0:14550")`, сигнал `rawFrameReceived(QByteArray)`, периодическая отправка `HEARTBEAT` + `REQUEST_DATA_STREAM` (запустить стрим телеметрии).
- `MavlinkParser`: stateful-декодер через `mavlink_parse_char()` (канал `MAVLINK_COMM_0`), эмитит типизированные сигналы:
  - `heartbeat(custom_mode, base_mode, system_status)`
  - `sysStatus(battery_voltage, battery_remaining_pct)`
  - `statustext(severity, text)`
  - `attitude(roll, pitch, yaw, yaw_rate)`
  - `globalPositionInt(lat, lon, alt, relative_alt, vx, vy, vz)`
  - `extendedSysState(armed, landed_state)`
- Демо: человекочитаемый лог всех сообщений в `qInfo()`.

**Артефакты:**
- `src/transport/UdpTransport.{h,cpp}`, `src/mavlink/MavlinkParser.{h,cpp}`, `src/mavlink/MessageCodec.{h,cpp}`.

**Критерий готовности:**
- ✅ В логе стабильно идут `HEARTBEAT`, `SYS_STATUS`, `ATTITUDE` (минимум 1 Hz).
- ✅ `STATUSTEXT` декодируется с корректным `severity`.

**Риск:** UDP 14550 может «молчать», пока клиент не проявит активность (не отправит HEARTBEAT/REQUEST_DATA_STREAM). Митигация: `UdpTransport` сразу после бинда шлёт HEARTBEAT + REQUEST_DATA_STREAM.

---

### Итерация 2 — Доменная модель + детектор событий
*(оценка: ~2.5 дня, диапазон 2–4)*

**Задачи:**
- `TelemetryState`: кэшированный снапшот:
  - `armed`, `customMode → modeName` через таблицу режимов ArduCopter (`STABILIZE=0, ALT_HOLD=2, LOITER=5, GUIDED=4, RTL=6, LAND=9, ...`).
  - `voltage`, `remaining_pct`.
  - `altitude_amsl_m`, `altitude_relative_m`, `climb_mps`.
  - `ground_speed_mps`, `air_speed_mps`.
  - `last_seen` (для обнаружения потери связи).
- `EventDetector`: сравнение «предыдущее vs текущее» → события:
  - `MODE_CHANGED(old, new)`
  - `ARMED` / `DISARMED`
  - `BATTERY_WARNING` (remaining_pct < `warning_pct`)
  - `BATTERY_CRITICAL` (remaining_pct < `critical_pct`)
  - `STATUSTEXT_WARN` (severity ≥ `MAV_SEVERITY_WARNING`, дословно)
- Гистерезис на порогах батареи (warning/critical из конфига), чтобы не дёргать между 20% и 21%.
- Юнит-тесты на `EventDetector` (граничные кейсы порогов, переходы armed→disarmed, дубли STATUSTEXT).

**Артефакты:**
- `src/domain/TelemetryState.{h,cpp}`, `src/domain/EventDetector.{h,cpp}`, `src/domain/TelemetryEvent.h`, `tests/test_event_detector.cpp`.

**Критерий готовности:**
- ✅ Юнит-тесты на `EventDetector` зелёные.
- ✅ Корректные события на смоделированной последовательности MAVLink-сообщений.

---

### Итерация 3 — TTS-каркас + RHVoice
*(оценка: ~2.5 дня, диапазон 2–4)*

**Задачи:**
- `TtsBackend` — абстрактный интерфейс: `void say(const QString& text, SpeechPriority priority)`, `void stop()`.
- `RhVoiceBackend`: запуск `rhvoice`/`spd-say` через `QProcess` (RHVoice как движок `speech-dispatcher`). Установка голоса/языка (`ru`).
- `EspeakNgBackend` (fallback) — на случай если RHVoice недоступен на 22.04.
- `SpeechQueue`: приоритетная FIFO в отдельной нити (`QThread` + `QQueue` + `QWaitCondition`), **неблокирующая** приём телеметрии.
  - Приоритеты: `INFO` < `WARNING` < `CRITICAL`.
  - При поступлении `CRITICAL` менее важные фразы в очереди скипаются (или прерывается текущая).
- Связка: `TelemetryEvent → текст → SpeechQueue`.

**Артефакты:**
- `src/tts/TtsBackend.h`, `src/tts/RhVoiceBackend.{h,cpp}`, `src/tts/EspeakNgBackend.{h,cpp}`, `src/tts/SpeechQueue.{h,cpp}`.

**Критерий готовности:**
- ✅ При arm/смене режима звучит голос.
- ✅ Приём телеметрии не подвисает на синтезе (счётчик пакетов не падает во время речи).
- ✅ Critical-событие прерывает текущую informational-фразу.

**Риск:** RHVoice на Ubuntu 22.04 может требовать сборки из исходников или PPA. Митигация: интерфейс `TtsBackend` + готовый `EspeakNgBackend`.

---

### Итерация 4 — Антиспам + хоткей «статус»
*(оценка: ~1.5 дня, диапазон 1–3)*

**Задачи:**
- `AntiSpamFilter`: `QHash<EventType, QDateTime> lastSpokenAt`, подавление того же типа чаще `antispam_seconds` из конфига.
  - Для `STATUSTEXT_WARN` — дедуп по подстроке/хэшу (чтобы не глушить разные STATUSTEXT одновременно).
  - Приоритет `CRITICAL` может обходить лимит (через флаг в событии).
- `StatusAnnouncer`: формирование фразы «Высота X м, скорость Y м/с, заряд Z%» из `TelemetryState`.
- `HotkeyController`: в headless — команды через stdin (`status`, `mute`, `unmute`, `quit`) через `QSocketNotifier(STDIN_FILENO)`; опционально глобальный хоткей через `/dev/input/event*` (с правами группы `input`).

**Артефакты:**
- `src/tts/AntiSpamFilter.{h,cpp}`, `src/announce/StatusAnnouncer.{h,cpp}`, `src/hotkey/HotkeyController.{h,cpp}`, `tests/test_antispam.cpp`.

**Критерий готовности:**
- ✅ Повторяющиеся события одного типа в течение N сек глушатся.
- ✅ Команда `status` озвучивает сводку (высота/скорость/заряд).
- ✅ `mute`/`unmute` работают.

---

### Итерация 5 — Интеграция, конфиг, тесты, доки
*(оценка: ~2.0 дня, диапазон 1.5–3)*

**Задачи:**
- `Config` (TOML via toml++) со схемой:
  ```toml
  [network]
  bind          = "0.0.0.0:14550"
  sysid         = 255
  compid        = 1
  heartbeat_hz  = 1

  [battery]
  warning_pct   = 20
  critical_pct  = 10

  [tts]
  engine    = "rhvoice"   # rhvoice | espeak-ng
  voice     = "anna"
  language  = "ru"

  [antispam]
  seconds         = 8
  bypass_critical = true

  [status]
  hotkey = "s"             # stdin-команда
  ```
- E2E-сценарий со SITL: arm → смена режима → разряд батареи (`SIM_BAT_LOW`) → STATUSTEXT → статус по хоткею.
- CI: GitHub Actions workflow (build gcs-tts, lint, tests) — опционально, если проект будет на GitHub.
- Финальный `README.md`: описание, зависимости, сборка, запуск, E2E-сценарий.

**Артефакты:**
- `src/Config.{h,cpp}`, `config/gcs.example.toml`, финальный `README.md`.

**Критерий готовности:**
- ✅ Воспроизводимый запуск по README на чистой Ubuntu 22.04.
- ✅ Все тесты зелёные.
- ✅ E2E-сценарий проходит (озвучиваются все 5 ТЗ-сценариев).

---

## 5. Сводка по времени

**1 разработчик, средний-высокий уровень, Ubuntu 22.04, локально.**

| Итерация | Likely | Оптим. | Пессим. |
|---|---|---|---|
| 0. Инфра + SITL/Copter + сборка | 1.5 д | 1.0 | 2.5 |
| 1. Транспорт + парсер | 2.5 д | 2.0 | 4.0 |
| 2. Домен + детектор | 2.5 д | 2.0 | 4.0 |
| 3. TTS + RHVoice | 2.5 д | 2.0 | 4.0 |
| 4. Антиспам + хоткей | 1.5 д | 1.0 | 3.0 |
| 5. Интеграция/конфиг/доки | 2.0 д | 1.5 | 3.0 |
| **Итого (likely)** | **~12.5 раб. дней** | **~9.5 дней** | **~20.5 дней (~2–4 недели)** |

- **Оптимистично (~9.5 дней):** опыт с MAVLink/Qt/RHVoice уже есть, SITL-Copter заводится с первой попытки, RHVoice ставится из пакета.
- **Пессимистично (~4 недели):** нюансы сборки ArduPilot из исходников, нюансы UDP-стрима, права `/dev/input` для хоткея, сборка RHVoice из исходников на 22.04, граничные случаи порогов батареи.

**В календарных сроках (с учётом буферов и ревью):**
- Likely: **~3 недели** до полностью готового, протестированного модуля.
- Полный коридор: **2–4 недели.**

---

## 6. Основные риски и митигации

| Риск | Вероятность | Влияние | Митигация |
|---|---|---|---|
| Сборка ArduPilot SITL с нуля занимает много времени/места | Высокая | Низкое | Скрипт `setup_sitl.sh` caches сборку; первая сборка 10–20 мин — это норма. |
| UDP 14550 «молчит» (SITL ждёт активности клиента) | Средняя | Высокое | В `UdpTransport` сразу слать `HEARTBEAT` + `REQUEST_DATA_STREAM`. |
| RHVoice на Ubuntu 22.04 ставится только из исходников | Средняя | Среднее | Интерфейс `TtsBackend` + fallback на `espeak-ng` (`EspeakNgBackend`). |
| Headless-хоткей (без X-сервера) нетривиален | Средняя | Низкое | Stdin-команды как основной путь (`status`, `mute`, `quit`). `/dev/input/event*` как опция. |
| Пороги батареи и гистерезис — недетерминированные события | Низкая | Среднее | Явный гистерезис в `EventDetector`: событие только на пересечение порога. Юнит-тесты. |
| `c_library_v2` vendoring — нюансы сборки с большими заголовками | Низкая | Низкое | Vendoring в `third_party/`, отдельный include-путь в CMake, диалект `ardupilotmega`. |

---

## 7. Карта соответствия ТЗ → итерации

| Требование ТЗ | Итерация |
|---|---|
| SITL ArduCopter на UDP 14550 | 0 |
| C++17 + Qt6 + CMake 3.20+ | 0 |
| Транспорт UDP | 1 |
| Парсер через `mavlink_c_library_v2` | 1 |
| Архитектура транспорт→парсер→домен→TTS | 1+2+3 |
| Смена режима полёта (озвучка) | 2+3 |
| arm / disarm (озвучка) | 2+3 |
| Батарея warning / critical | 2+3 |
| STATUSTEXT WARNING+ дословно | 2+3 |
| Спич-«статус» по горячей клавише | 4 |
| Антиспам (не чаще N сек, N в конфиге) | 4 |
| Очередь TTS не блокирует телеметрию | 3 |
| Конфиг (пороги, N, TTS) | 5 |
| Тесты, доки | 5 |

---

## 8. Чекпоинты ревью

- **После итерации 0:** демо «видим пакеты на 14550 от ArduCopter».
- **После итерации 1:** демо «человекочитаемый лог телеметрии».
- **После итерации 3:** демо «голос озвучивает arm/режим/батарею» — **MVP по ТЗ.**
- **После итерации 5:** демо полного E2E-сценария со SITL — **релиз-кандидат.**

---

## 9. История изменений

| Дата | Автор | Изменение |
|---|---|---|
| 2026-08-05 | ZCode (планирование) | Создан документ. Утверждён план, 6 итераций, оценка ~12.5 раб. дней (likely). Проект — отдельный git-репозиторий `gcs-tts`, не связан с другими проектами. SITL поднимается штатно по документации ArduPilot. |
