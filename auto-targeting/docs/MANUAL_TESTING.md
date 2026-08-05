# Manual Testing Guide

> **Цель:** проверить систему вручную без реального железа (дрона), используя
> только Orange Pi 5 (или dev-машину x86), synthetic video и mock FC.
>
> **Аудитория:** разработчик/тестировщик, у которого есть доступ к Orange Pi 5
> по SSH и/или dev-машина с Linux.

---

## Оглавление

1. [Подготовка окружения](#1-подготовка-окружения)
2. [Тест 1: Smoke test (mock-all)](#тест-1-smoke-test-mock-all)
3. [Тест 2: Интерактивный REPL](#тест-2-инактивный-repl)
4. [Тест 3: Scenario runner (5 сценариев)](#тест-3-scenario-runner-5-сценариев)
5. [Тест 4: V4L2 с vivid (синтетическая камера)](#тест-4-v4l2-с-vivid-синтетическая-камера)
6. [Тест 5: SITL ArduPilot (симулятор FC)](#тест-5-sitl-ardupilot-симулятор-fc)
7. [Тест 6: Реальная камера Arducam UC-852](#тест-6-реальная-камера-arducam-uc-852)
8. [Тест 7: Реальный FC SpeedyBee F405](#тест-7-реальный-fc-speedybee-f405)
9. [Тест 8: RKNN bridge (NPU inference)](#тест-8-rknn-bridge-npu-inference)
10. [Тест 9: Полный end-to-end pipeline](#тест-9-полный-end-to-end-pipeline)
11. [Запись сессии (screen recording)](#запись-сессии-screen-recording)
12. [SSH-доступ к Orange Pi 5](#ssh-доступ-к-orange-pi-5)
13. [Критерии успеха (итоговая таблица)](#критерии-успеха-итоговая-таблица)
14. [Troubleshooting](#troubleshooting)

---

## 1. Подготовка окружения

### На dev-машине (x86 Linux)

```bash
# Клонировать репозиторий
git clone https://github.com/EgorLikhachev/Autotargeting.git
cd Autotargeting

# Установить Rust (если нет)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# Проверить сборку
cargo build --workspace
cargo test --workspace
```

**Ожидаемый результат:** 185 тестов проходят, 0 ошибок сборки.

### На Orange Pi 5 (aarch64)

```bash
# SSH подключение (замените IP на свой)
ssh username@192.168.1.100

# На Orange Pi: установить зависимости
sudo apt update
sudo apt install -y build-essential cmake libclang-dev v4l-utils

# Установить Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# Клонировать и собрать с V4L2
git clone https://github.com/EgorLikhachev/Autotargeting.git
cd Autotargeting
cargo build --release --features video-capture/v4l2
```

**Ожидаемый результат:** бинарник `target/release/auto-targeting` создан.

---

## Тест 1: Smoke test (mock-all)

**Цель:** проверить, что базовый pipeline работает end-to-end на mock'ах.

### Команда

```bash
cargo run -p auto-targeting-cli -- --mock-all
```

### Что происходит

- Создаётся `MockFcAdapter` (имитация FC)
- Commander проходит цикл: IDLE → ARMED → SCANNING
- 5 циклов симуляции с fake corrections
- Watchdog `video_loop` триггерится (намеренно, 101ms > 100ms limit)
- Выводится summary

### Ожидаемый вывод

```
=== Mock-all smoke test summary ===
State machine:    SCANNING
Transitions:      2
FC commands sent: 7
FC armed:         true
FC mode:          Guided

Anti-loop guard:
  Allowed:         5
  Suppressed:      0
  Oscillations:    0

Watchdogs:
  fc_heartbeat    elapsed=  505ms / limit= 1000ms  expired=false
  video_loop      elapsed=  101ms / limit=  100ms  expired=true

All good. ✅
```

### ✅ Критерий успеха

- `FC armed: true` — arm сработал
- `FC mode: Guided` — set_mode сработал
- `FC commands sent: 7` — команды отправлены (arm + set_mode + 5 corrections)
- `Watchdogs: video_loop expired=true` — watchdog корректно сработал
- `All good. ✅` в конце

---

## Тест 2: Интерактивный REPL

**Цель:** проверить интерактивное управление системой.

### Команда

```bash
cargo run -p auto-targeting-cli -- --repl
```

### Что происходит

Запускается интерактивная консоль. Введите команды:

```
help                          # список команд
status                        # текущее состояние
arm                           # армировать
scan                          # начать сканирование
select-target 1               # выбрать цель 1
status                        # проверить TRACKING
simulate-attitude 0.1 0.2 1.5 # внедрить attitude
status                        # увидеть новое attitude
watchdogs                     # проверить watchdogs
abort                         # ABORT + RTL
status                        # проверить ABORT
reset                         # вернуться в IDLE
quit                          # выход
```

### ✅ Критерий успеха

- `status` после `arm` показывает `State machine: ARMED`
- `select-target 1` переводит в `TRACKING`
- `abort` переводит в `ABORT`, отправляет RTL
- `reset` возвращает в `IDLE`
- `simulate-attitude` обновляет attitude в `status`
- Все команды выполняются без паник/ошибок

### Автоматизированная проверка (piped input)

```bash
echo -e "arm\nscan\nselect-target 1\nstatus\nabort\nquit" | \
  cargo run -p auto-targeting-cli -- --repl 2>&1 | grep -E "State machine|OK|ABORT"
```

**Ожидаемый вывод:**
```
State machine:    SCANNING (transitions: 2)
OK: armed
OK: scanning for targets
OK: target 1 selected — transition to TARGET_SELECTED
OK: lock acquired — now TRACKING target 1
State machine:    TRACKING (transitions: 4)
!! ABORT !! — state set to ABORT
OK: RTL command sent to FC
```

---

## Тест 3: Scenario runner (5 сценариев)

**Цель:** запустить готовые тестовые сценарии и проверить KPI.

### Команды

```bash
# Запустить один сценарий
cargo run -p auto-targeting-cli -- scenario sim/scenarios/scenario_static_target.json

# Запустить все сценарии
cargo run -p auto-targeting-cli -- scenario --all sim/scenarios/

# С verbose логированием
cargo run -p auto-targeting-cli -- -v scenario sim/scenarios/scenario_occlusion.json
```

### Ожидаемый вывод (все сценарии)

```
=== Scenario Suite Summary ===
SCENARIO                            STATUS   FRAMES     CMDS    TRANS      WDT
---------------------------------------------------------------------------
moving_target_horizontal            PASS        600        2        4        0
multiple_targets_selection          PASS        450        2        4        0
occlusion_recovery                  PASS        600        2        4        0
oscillation_resistance              PASS        600        2        4        0
static_target                       PASS        300        2        4        0
---------------------------------------------------------------------------
Total: 5 passed, 0 failed (100% pass rate)
```

### ✅ Критерий успеха

- **5/5 сценариев PASS** (100% pass rate)
- `WDT` (watchdog triggers) = 0 для всех
- `STATUS` = PASS для каждого сценария
- Exit code = 0 (можно проверить: `echo $?`)

### Описание сценариев

| Сценарий | Что тестирует |
|---|---|
| `scenario_static_target.json` | Базовый lock acquisition, статичная цель |
| `scenario_moving_target.json` | Tracker prediction, yaw correction |
| `scenario_occlusion.json` | Kalman prediction, recovery после 1с gap |
| `scenario_multiple_targets.json` | Выбор цели оператором из нескольких |
| `scenario_oscillation_test.json` | Anti-loop guard, устойчивость к erratic motion |

---

## Тест 4: V4L2 с vivid (синтетическая камера)

**Цель:** проверить V4l2Source с реальным V4L2 устройством без физической камеры.

> **Требует:** Linux с поддержкой `vivid` kernel module (Ubuntu/Debian).

### Подготовка

```bash
# Загрузить vivid module (создаёт /dev/video0 с test patterns)
sudo modprobe vivid

# Проверить, что устройство создано
ls -la /dev/video*
# Ожидается: /dev/video0 ... /dev/videoN

# Установить libclang (для сборки v4l crate)
sudo apt install -y libclang-dev

# Установить v4l-utils для проверки
sudo apt install -y v4l-utils

# Проверить форматы, поддерживаемые vivid
v4l2-ctl --device /dev/video0 --list-formats-ext
```

### Сборка с V4L2

```bash
cargo build -p video-capture --features v4l2
```

### Запуск тестов

```bash
# Запустить vivid-gated тесты
cargo test -p video-capture --features v4l2 -- --include-ignored vivid
```

### Ожидаемый вывод

```
running 2 tests
test v4l2_stub::tests::vivid_probe_succeeds ... ok
test v4l2_stub::tests::vivid_query_formats_succeeds ... ok

test result: ok. 2 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
```

### Ручная проверка probe

```bash
# Создать временный binary, который вызывает V4l2Source::probe()
cat > /tmp/v4l2_probe_test.rs << 'EOF'
use video_capture::{V4l2Source, VideoSource};
use common::PixelFormat;

#[tokio::main]
async fn main() {
    let mut src = V4l2Source::new("/dev/video0", 640, 480, 30)
        .with_format(PixelFormat::Yuyv);
    
    match src.probe() {
        Ok(probe) => println!("Probe OK:\n{probe}"),
        Err(e) => eprintln!("Probe failed: {e}"),
    }
}
EOF
```

### ✅ Критерий успеха

- `v4l2-ctl --list-formats-ext` показывает форматы (YUYV, MJPG и т.д.)
- V4L2 тесты проходят (2 passed)
- `probe()` не возвращает ошибку для поддерживаемого формата
- `probe()` возвращает ошибку для неподдерживаемого формата

### Выгрузка vivid

```bash
sudo modprobe -r vivid
```

---

## Тест 5: SITL ArduPilot (симулятор FC)

**Цель:** проверить ArduPilotMavlinkAdapter против реального ArduPilot firmware
в симуляторе.

> **Требует:** Docker на dev-машине (Ubuntu/Debian).
> **Время первой сборки:** ~10-15 минут (компиляция ArduPilot).
> **Время последующих запусков:** ~5 секунд (Docker cache).

### Шаг 1: Запуск SITL

```bash
# В корне проекта
./sim/sitl/run_sitl.sh start
```

**Что произойдёт:**
1. Проверка Docker
2. Если образа нет — сборка ArduPilot из исходников (~10-15 мин)
3. Запуск контейнера
4. Ожидание открытия порта 5760 (~30 сек после сборки)

**Ожидаемый вывод:**
```
[INFO] Запуск ArduPilot SITL...
[INFO] Образ не найден. Первая сборка займёт ~10-15 минут...
[INFO] Сборка ArduPilot из исходников...
... (10-15 минут логов сборки) ...
[INFO] Ожидание запуска SITL...
[INFO] Проверка MAVLink на порту 5760...
  Ожидание... (15/60)
[OK] SITL запущен! MAVLink доступен на tcp://127.0.0.1:5760

[INFO] Подключение auto-targeting:
  cargo run -p auto-targeting-cli -- --config sim/sitl/sitl-config.toml -- --repl
```

### Шаг 2: Проверка MAVLink соединения

```bash
# Проверить, что SITL отвечает
./sim/sitl/run_sitl.sh test
```

**Ожидаемый вывод (если установлен pymavlink):**
```
[INFO] Тест MAVLink соединения с SITL...
[INFO] Тест через pymavlink...
Ожидание heartbeat...
✅ HEARTBEAT получен! mode=0, armed=false

Тест arm/disarm...
  Armed: true
  Armed: false

✅ MAVLink тест пройден!
```

### Шаг 3: Запуск auto-targeting против SITL

```bash
# В корне проекта (другой терминал)
cargo run -p auto-targeting-cli -- \
  --config sim/sitl/sitl-config.toml -- --repl
```

### Шаг 4: Тестовые команды в REPL

```
status                        # проверить, что heartbeat получен (FC heartbeat: OK)
arm                           # армировать (SITL должен ответить)
status                        # FC armed: true
set-mode guided               # переключить в GUIDED
status                        # FC mode: Guided
set-mode loiter               # переключить в LOITER
status                        # FC mode: Loiter
abort                         # RTL — SITL должен перейти в RTL
status                        # FC mode: Rtl
disarm
quit
```

### Шаг 5: Автоматизированные тесты (опционально)

```bash
# Запустить 7 интеграционных тестов против SITL
cargo test -p fc-adapter --test sitl_integration -- --include-ignored
```

**Тесты:**
| Тест | Что проверяет |
|---|---|
| `test_sitl_connect` | Подключение к SITL по TCP |
| `test_sitl_heartbeat` | Получение HEARTBEAT за 5 сек |
| `test_sitl_arm_disarm` | Команды arm/disarm |
| `test_sitl_mode_change` | Смена режима (GUIDED, LOITER, RTL) |
| `test_sitl_attitude` | Получение ATTITUDE telemetry |
| `test_sitl_heartbeat_stability` | Стабильность heartbeat (10 сек, < 5% stale) |
| `test_sitl_heartbeat_loss_detection` | Детекция потери связи |

### Подключение через QGroundControl (опционально)

```bash
# Установить QGroundControl (если нет)
# См. https://docs.qgroundcontrol.com/master/en/getting_started/download_and_install.html

# Запустить QGroundControl, оно автоматически найдёт SITL на 127.0.0.1:5760
qgroundcontrol &
```

В QGroundControl вы увидите:
- Карта с позицией дрона (Canberra, Australia по умолчанию)
- Текущий режим (MANUAL, GUIDED, RTL)
- Armed/disarmed статус
- Attitude (roll/pitch/yaw) в реальном времени

### Проверка через MAVProxy (альтернатива)

```bash
# Установить MAVProxy
pip install pymavlink mavproxy

# Подключиться к SITL
mavproxy.py --master=tcp:127.0.0.1:5760

# В MAVProxy:
# > status
# > mode
# > arm throttle
# > disarm
# > mode guided
# > mode rtl
```

### Управление SITL

```bash
./sim/sitl/run_sitl.sh status   # статус контейнера и портов
./sim/sitl/run_sitl.sh logs     # логи SITL (Ctrl+C для выхода)
./sim/sitl/run_sitl.sh restart  # перезапуск
./sim/sitl/run_sitl.sh stop     # остановка
./sim/sitl/run_sitl.sh clean    # полная очистка (образ + логи)
```

### ✅ Критерий успеха

- `./sim/sitl/run_sitl.sh start` завершается успешно (порт 5760 открыт)
- `./sim/sitl/run_sitl.sh test` проходит без ошибок
- `status` в REPL показывает `FC heartbeat: OK` (не STALE)
- `arm` срабатывает, `FC armed: true` в `status`
- `set-mode guided` меняет `FC mode: Guided`
- `abort` отправляет RTL, `FC mode: Rtl`
- 7 интеграционных тестов проходят (`cargo test -- --include-ignored`)
- QGroundControl (если запущен) видит те же изменения
- Latency команды (от ввода до изменения режима) < 1 сек

### Структура файлов SITL

```
sim/sitl/
├── Dockerfile           # Сборка ArduPilot из исходников
├── docker-compose.yml   # Конфигурация контейнера
├── sitl_params.parm     # Параметры ArduPilot (arm без RC, и т.д.)
├── sitl-config.toml     # Конфиг auto-targeting для SITL
├── run_sitl.sh          # Helper-скрипт (start/stop/test/logs/...)
└── README.md            # Подробная документация
```

### Остановка SITL

```bash
./sim/sitl/run_sitl.sh stop
```

---

## Тест 6: Реальная камера Arducam UC-852

**Цель:** проверить V4l2Source с реальной USB-камерой.

> **Требует:** Orange Pi 5 + Arducam UC-852 (USB).

### Подключение и проверка

```bash
# Подключить камеру в USB-порт Orange Pi 5

# Проверить, что камера определилась
lsusb | grep -i arducam
# или
lsusb
# Ожидается: Bus 001 Device 004: ID ... Arducam ...

# Проверить, что /dev/video* создан
ls -la /dev/video*
v4l2-ctl --list-devices

# Проверить поддерживаемые форматы
v4l2-ctl --device /dev/video0 --list-formats-ext
# Ожидается: MJPG и/или YUYV на 1280x720@30fps
```

### Запись тестового видео (для проверки камеры)

```bash
# Записать 5 секунд видео в файл
ffmpeg -f v4l2 -input_format mjpeg -video_size 1280x720 -i /dev/video0 \
  -t 5 -y /tmp/camera_test.mp4

# Воспроизвести (если есть ffplay)
ffplay /tmp/camera_test.mp4

# Или скопировать на dev-машину для просмотра
scp username@orangepi:/tmp/camera_test.mp4 .
```

### Запуск auto-targeting с реальной камерой

```bash
# На Orange Pi 5
cd Autotargeting

# Создать конфиг
cp config.example.toml config.camera.toml
nano config.camera.toml
# Установить:
#   [video]
#   device = "/dev/video0"
#   width = 1280
#   height = 720
#   fps = 30
#   format = "mjpeg"
#   [fc]
#   adapter = "mock"  # пока без реального FC

# Запустить (с mock FC, реальная камера)
cargo run --release --features video-capture/v4l2 -- \
  --config config.camera.toml -- --repl
```

### Проверка latency

```bash
# Замерить latency камеры напрямую
v4l2-ctl --device /dev/video0 --set-fmt-video=width=1280,height=720,pixelformat=MJPG \
  --stream-mmap --stream-count=30 --stream-to=/dev/null 2>&1 | grep -E "fps|latency"
```

### ✅ Критерий успеха

- `lsusb` видит Arducam
- `v4l2-ctl --list-formats-ext` показывает MJPG 1280x720@30fps
- FFmpeg записывает видео без ошибок
- Воспроизведение показывает изображение (не чёрный экран)
- Latency камеры < 50ms (KPI из HYPOTHESES.md H-003)
- auto-targeting запускается без ошибок V4L2

### Если камера не определяется

```bash
# Проверить dmesg
dmesg | tail -20

# Перезагрузить USB
sudo uhubctl -a cycle -l 1-1

# Проверить USB-питание (Arducam требует 500mA+)
lsusb -v | grep -i "max power"
```

---

## Тест 7: Реальный FC SpeedyBee F405

**Цель:** проверить ArduPilotMavlinkAdapter с реальным полётным контроллером.

> **Требует:** SpeedyBee F405 с ArduPilot Plane firmware.

### Подключение

```bash
# Подключить SpeedyBee F405 к Orange Pi 5 по USB

# Проверить, что FC определился как serial device
ls -la /dev/ttyACM* /dev/ttyUSB*
# Ожидается: /dev/ttyACM0 (USB CDC) или /dev/ttyUSB0 (FTDI)

# Проверить dmesg
dmesg | tail -10
# Ожидается: "cdc_acm: USB CDC device" или "ftdi_sio"
```

### Настройка ArduPilot (через QGroundControl)

1. Подключить FC к компьютеру с QGroundControl
2. В QGroundControl → Parameters:
   - `SERIAL0_PROTOCOL` = 1 (MAVLink v1) или 2 (MAVLink v2)
   - `SERIAL0_BAUD` = 115200
3. В QGroundControl → Plan:
   - Установить ArduPilot Plane firmware (последний stable)
4. Отключить от компьютера, подключить к Orange Pi 5

### Проверка MAVLink соединения

```bash
# Установить mavproxy
pip install pymavlink mavproxy

# Подключиться к FC
mavproxy.py --master=/dev/ttyACM0 --baudrate=115200

# В MAVProxy:
# > status
# Ожидается: heartbeat от FC
# > mode
# Ожидается: текущий режим (MANUAL, STABILIZE, и т.д.)
# > exit
```

### Запуск auto-targeting с реальным FC

```bash
# На Orange Pi 5
cd Autotargeting

# Конфиг
cp config.example.toml config.fc.toml
nano config.fc.toml
# Установить:
#   [fc]
#   adapter = "ardupilot-mavlink"
#   endpoint = "serial:/dev/ttyACM0:115200"
#   baud_rate = 115200
#   heartbeat_timeout_ms = 1000

# Запустить (С ОСТОРОЖНОСТЬЮ — пропеллеры СНЯТЬ!)
cargo run --release -- --config config.fc.toml -- --repl
```

### ⚠️ БЕЗОПАСНОСТЬ

```
⚠️  ПЕРЕД ЗАПУСКОМ С РЕАЛЬНЫМ FC:
1. СНЯТЬ ПРОПЕЛЛЕРЫ с моторов
2. Закрепить дрон (стяжки/тиски)
3. Убедиться, что battery подключена, но motors НЕ будут вращаться
4. Иметь RC пульт наготове для override
5. Первые тесты — только arm/disarm, БЕЗ mode guided
```

### Тестовые команды в REPL

```
status                        # проверить heartbeat
arm                           # армировать (моторы НЕ вращаются без throttle)
disarm                        # разармировать
set-mode guided               # переключить в GUIDED
set-mode rtl                  # переключить в RTL
set-mode loiter               # переключить в LOITER
abort                         # ABORT + RTL
disarm
quit
```

### ✅ Критерий успеха

- `status` показывает `FC heartbeat: OK` (heartbeat получен за < 1с)
- `arm` срабатывает, `FC armed: true`
- В QGroundControl (если подключён параллельно) видно arm/disarm
- `set-mode` меняет режим, видимый в QGroundControl
- `abort` переводит FC в RTL
- Latency команды (от ввода до изменения режима) < 1с

### Проверка 10Hz streaming (H-002)

```bash
# Запустить auto-targeting, выбрать цель, затем в другом терминале:
mavproxy.py --master=/dev/ttyACM0 --baudrate=115200

# В MAVProxy:
# > output add udpout:127.0.0.1:14551
# > status
# Наблюдать SET_POSITION_TARGET_LOCAL_NED сообщения — должны быть 10/сек
```

---

## Тест 8: RKNN bridge (NPU inference)

**Цель:** проверить C++ rknn-bridge микросервис.

> **Требует:** Orange Pi 5 (RK3588S с NPU).

### Сборка rknn-bridge

```bash
# На Orange Pi 5
cd Autotargeting/rknn-bridge

# Без RKNN SDK (stub backend — возвращает fake detections)
mkdir build && cd build
cmake ..
make

# Проверить, что binary создан
ls -la rknn-bridge
./rknn-bridge --help
```

### Ожидаемый вывод `--help`

```
=== rknn-bridge ===
Socket: /tmp/rknn-bridge.sock
Model:  

Usage: /path/to/rknn-bridge [options]
Options:
  -s, --socket PATH   Unix socket path (default: /tmp/rknn-bridge.sock)
  -m, --model PATH    Path to .rknn model file (loaded on init message)
  -h, --help          Show this help
```

### Запуск bridge

```bash
# Терминал 1: запустить bridge
./rknn-bridge --socket /tmp/rknn-bridge.sock

# Ожидаемый вывод:
# [bridge] Using backend: stub
# [ShmServer] Listening on /tmp/rknn-bridge.sock
# [bridge] Ready. Waiting for init message...
```

### Ручная проверка IPC (через netcat)

```bash
# Терминал 2: подключиться к socket
sudo apt install -y netcat-openbsd
nc -U /tmp/rknn-bridge.sock

# Отправить health check (вставить в nc):
{"type":"health"}
# Ожидаемый ответ:
# {"type":"health_ack","ok":true,"model_loaded":false,"npu_utilization":-1.000000,"backend":"stub"}

# Отправить init:
{"type":"init","model_path":"/tmp/model.rknn","input_width":1280,"input_height":720,"input_format":"nv12","confidence_threshold":0.45,"nms_threshold":0.45}
# Ожидаемый ответ:
# {"type":"init_ack","ok":true,"output_classes":80,"backend":"stub"}

# Отправить shutdown:
{"type":"shutdown"}
# Ожидаемый ответ:
# {"type":"shutdown_ack"}
```

### Сборка с реальным RKNN SDK

```bash
# Скачать RKNPU2 SDK
git clone https://github.com/airockchip/rknn-toolkit2 /opt/rknn-toolkit2

# Собрать с RKNN
cd Autotargeting/rknn-bridge
rm -rf build && mkdir build && cd build
cmake -DRKNN_SDK_PATH=/opt/rknn-toolkit2 ..
make

# Ожидаемый вывод cmake:
# -- Found RKNN library: /opt/rknn-toolkit2/runtime/Linux/librknn_api/aarch64/librknnrt.so
# Ожидаемый вывод при запуске:
# [bridge] Using backend: rknn
```

### ✅ Критерий успеха (stub)

- `./rknn-bridge --help` работает
- Bridge запускается и слушает на socket
- Health check через nc возвращает `ok:true, backend:stub`
- Init message загружает (stub) модель, возвращает `output_classes:80`
- Shutdown корректно завершает bridge

### ✅ Критерий успеха (real NPU)

- `cmake` находит `librknnrt.so`
- Bridge запускается с `backend: rknn`
- Init с реальным `.rknn` файлом возвращает `ok:true`
- Inference возвращает detections с реальными координатами
- Latency inference < 60ms (KPI из HYPOTHESES.md)

---

## Тест 9: Полный end-to-end pipeline

**Цель:** проверить весь pipeline с реальной камерой + реальным FC (mock NPU).

> **Требует:** Orange Pi 5 + Arducam UC-852 + SpeedyBee F405 (пропеллеры СНЯТЬ).

### Конфигурация

```bash
# config.full.toml
[video]
device = "/dev/video0"
width = 1280
height = 720
fps = 30
format = "mjpeg"
queue_depth = 3

[inference]
model_path = "/opt/auto-targeting/models/yolov8n_int8.rknn"
confidence_threshold = 0.45
nms_threshold = 0.45
track_classes = ["person"]
bridge_socket = "/tmp/rknn-bridge.sock"
allow_cpu_fallback = false

[fc]
adapter = "ardupilot-mavlink"
endpoint = "serial:/dev/ttyACM0:115200"
baud_rate = 115200
system_id = 255
component_id = 1
target_system_id = 1
target_component_id = 1
command_rate_hz = 10
heartbeat_timeout_ms = 1000

[commander]
video_loop_wdt_ms = 100
inference_loop_wdt_ms = 200
tracking_loop_wdt_ms = 50
command_loop_wdt_ms = 100
deadband_fraction = 0.05
loss_hysteresis_ms = 500
max_yaw_rate_dps = 30.0
max_pitch_rate_dps = 15.0
max_offset_fraction = 0.30
oscillation_window = 30
oscillation_threshold = 0.5
oscillation_abort_count = 3

log_file = "/var/log/auto-targeting/auto-targeting.log"
log_filter = "info,auto_targeting=debug"
```

### Запуск

```bash
# Терминал 1: запустить rknn-bridge
cd Autotargeting/rknn-bridge/build
./rknn-bridge --socket /tmp/rknn-bridge.sock --model /opt/auto-targeting/models/yolov8n_int8.rknn

# Терминал 2: запустить auto-targeting
cd Autotargeting
cargo run --release --features video-capture/v4l2 -- \
  --config config.full.toml -- --repl
```

### REPL сессия

```
status                        # проверить, что FC heartbeat OK, камера работает
arm                           # армировать
scan                          # начать сканирование
# (помахать человеком перед камерой)
# В логах должны появиться detections
select-target 1               # выбрать обнаруженную цель
status                        # TRACKING
watchdogs                     # все watchdogs OK
# Повернуть камеру — дрон должен пытаться удержать цель
anti-loop                     # проверить, что oscillations = 0
abort                         # RTL
disarm
quit
```

### ✅ Критерий успеха

- Камера захватывает видео без ошибок
- Inference возвращает detections (видно в логах)
- FC heartbeat стабилен (не STALE)
- `select-target` переводит в TRACKING
- При повороте камеры отправляются correction commands (10Hz)
- Watchdogs не триггерятся (0 в нормальном режиме)
- Anti-loop guard не активируется (0 oscillations)
- `abort` корректно переводит FC в RTL

---

## Запись сессии (screen recording)

### asciinema (для терминальных сессий)

```bash
# Установить asciinema
sudo apt install asciinema

# Начать запись
asciinema rec /tmp/test_session.cast

# Выполнить тесты
cargo run -p auto-targeting-cli -- --repl
# ... команды ...

# Остановить: Ctrl+D или exit

# Воспроизвести
asciinema play /tmp/test_session.cast

# Конвертировать в GIF (опционально)
# Установить: pip install asciinema2gif
asciinema2gif /tmp/test_session.cast /tmp/test_session.gif
```

### script (стандартная утилита)

```bash
# Записать терминальную сессию
script -t /tmp/test_session.timing -a /tmp/test_session.log

# Выполнить тесты...

# Остановить: exit

# Воспроизвести с таймингами
scriptreplay -t /tmp/test_session.timing -a /tmp/test_session.log
```

### Запись видео с экрана (для QGroundControl + терминала)

```bash
# Установить OBS Studio или ffmpeg
sudo apt install -y ffmpeg

# Запись экрана (X11)
ffmpeg -f x11grab -s 1920x1080 -i :0.0 -r 30 -c:v libx264 \
  -preset fast -crf 23 /tmp/screen_recording.mp4

# Остановить: q в терминале ffmpeg или Ctrl+C
```

### Запись логов

```bash
# Сохранить stdout+stderr в файл + показать в терминале
cargo run -p auto-targeting-cli -- --repl 2>&1 | tee /tmp/repl_session.log

# Сохранить с timestamp
cargo run -p auto-targeting-cli -- --repl 2>&1 | \
  ts "[%Y-%m-%d %H:%M:%S]" | tee /tmp/repl_session_timestamped.log
```

### Сохранение результатов тестов

```bash
# Запустить все тесты и сохранить результат
cargo test --workspace 2>&1 | tee /tmp/test_results.log
cargo test --workspace 2>&1 | grep "test result" > /tmp/test_summary.txt

# Запустить scenarios и сохранить
cargo run -p auto-targeting-cli -- scenario --all sim/scenarios/ 2>&1 | \
  tee /tmp/scenario_results.log

# Запустить benchmarks и сохранить
cargo bench --workspace 2>&1 | tee /tmp/benchmark_results.log
```

---

## SSH-доступ к Orange Pi 5

### Настройка SSH на Orange Pi 5

```bash
# На Orange Pi 5:
sudo systemctl enable ssh
sudo systemctl start ssh

# Узнать IP-адрес
ip addr show | grep "inet " | grep -v 127.0.0.1
# Ожидается: inet 192.168.1.100/24
```

### Подключение с dev-машины

```bash
# Базовое подключение
ssh username@192.168.1.100

# С пробросом портов (для QGroundControl)
ssh -L 5760:localhost:5760 username@192.168.1.100

# С пробросом X11 (для GUI приложений)
ssh -X username@192.168.1.100

# С копированием ключа (чтобы не вводить пароль)
ssh-copy-id username@192.168.1.100
```

### Передача файлов

```bash
# Скопировать бинарник на Orange Pi
scp target/aarch64-unknown-linux-gnu/release/auto-targeting \
  username@192.168.1.100:/home/username/

# Скопировать всю папку
scp -r Autotargeting username@192.168.1.100:/home/username/

# Синхронизация (быстрее для повторных передач)
rsync -avz --exclude target/ --exclude .git/ \
  ./ username@192.168.1.100:/home/username/Autotargeting/
```

### Удалённая разработка (VS Code)

```bash
# Установить VS Code + extension "Remote - SSH"
# F1 → "Remote-SSH: Connect to Host" → username@192.168.1.100
# Открыть папку Autotargeting
```

### Удалённый запуск тестов

```bash
# Запустить тесты на Orange Pi через SSH
ssh username@192.168.1.100 "cd Autotargeting && cargo test --workspace 2>&1" | \
  tee /tmp/remote_test_results.log

# Запустить REPL на Orange Pi (интерактивно)
ssh -t username@192.168.1.100 "cd Autotargeting && cargo run -- --repl"
```

### Мониторинг в реальном времени

```bash
# SSH + tmux для persistent сессий
ssh username@192.168.1.100
tmux new -s auto-targeting

# В tmux: запустить bridge
cd Autotargeting/rknn-bridge/build && ./rknn-bridge

# Ctrl+B, C — новое окно
# Запустить auto-targeting
cd Autotargeting && cargo run --release -- --repl

# Ctrl+B, D — отключиться (процессы продолжают работать)
# Переподключиться: tmux attach -t auto-targeting
```

---

## Критерии успеха (итоговая таблица)

| # | Тест | Критерий успеха | Статус |
|---|---|---|---|
| 1 | Smoke test (--mock-all) | `All good. ✅`, 7 FC commands, video_loop watchdog expired | ✅ |
| 2 | REPL | arm→scan→select→abort→reset работает интерактивно | ✅ |
| 3 | Scenarios | 5/5 PASS (100% pass rate), 0 watchdog triggers | ✅ |
| 4 | V4L2 + vivid | 2 vivid-gated теста проходят, probe() работает | ✅ (с libclang) |
| 5 | SITL ArduPilot | heartbeat OK, arm/disarm, set-mode, abort работают | 🚧 требует Docker |
| 6 | Arducam UC-852 | lsusb видит, v4l2-ctl показывает форматы, latency < 50ms | 🚧 требует железо |
| 7 | SpeedyBee F405 | heartbeat OK, arm/disarm, set-mode работают, < 1с latency | 🚧 требует железо |
| 8 | RKNN bridge (stub) | bridge запускается, health/init/shutdown через nc работают | ✅ |
| 8b | RKNN bridge (real NPU) | backend: rknn, inference < 60ms | 🚧 требует NPU |
| 9 | Full pipeline | камера+FC+inference работают вместе, TRACKING удерживается | 🚧 требует всё железо |

### Количественные KPI (для проверки)

| KPI | Цель | Где замерять | Как |
|---|---|---|---|
| Video latency | < 50 ms | Тест 6 | `v4l2-ctl --stream-mmap` |
| Inference latency | < 60 ms | Тест 8b | В логах bridge |
| Lock acquisition | < 1 с | Тест 9 | В REPL `status` |
| MAVLink command latency | < 5 ms | Тест 7 | mavproxy `status` |
| FC heartbeat stability | < 1 триггер/час | Тест 7 | `watchdogs` в REPL |
| Oscillation events | 0 за 30 мин | Тест 9 | `anti-loop` в REPL |
| Scenario pass rate | 100% | Тест 3 | `scenario --all` |
| Unit test pass rate | 100% | Всегда | `cargo test --workspace` |
| Clippy warnings | 0 | Всегда | `cargo clippy -- -D warnings` |

---

## Troubleshooting

### `cargo build` — ошибка "libclang not found"

```bash
# Решение: установить libclang-dev
sudo apt install -y libclang-dev

# Или собрать без V4L2
cargo build  # без --features v4l2
```

### V4L2 — "device not found"

```bash
# Проверить подключение
lsusb
dmesg | tail -20

# Проверить права
ls -la /dev/video*
sudo chmod 666 /dev/video0  # временно
# Или добавить пользователя в группу video:
sudo usermod -a -G video $USER
# Перелогиниться
```

### MAVLink — "heartbeat lost"

```bash
# Проверить скорость порта
stty -F /dev/ttyACM0  # должно быть 115200

# Проверить MAVLink трафик
cat /dev/ttyACM0 | xxd | head -20

# Попробовать другие скорости
mavproxy.py --master=/dev/ttyACM0 --baudrate=57600
mavproxy.py --master=/dev/ttyACM0 --baudrate=921600
```

### REPL — "Cannot start a runtime from within a runtime"

Это известная ошибка, если использовать `block_on` в async контексте.
Решение: все тесты должны быть `#[tokio::test]`, не использовать `Runtime::new()`.

### Scenario runner — "final state mismatch"

```bash
# Запустить с verbose
cargo run -- -v scenario sim/scenarios/scenario_name.json

# Проверить логи — увидеть, что произошло
grep "state transition" /tmp/scenario.log
```

### RKNN bridge — "couldn't find any valid shared libraries matching: libclang.so"

```bash
# Установить libclang
sudo apt install -y libclang-dev

# Или указать путь
export LIBCLANG_PATH=/usr/lib/llvm-14/lib
```

### Docker — "permission denied"

```bash
# Добавить пользователя в группу docker
sudo usermod -aG docker $USER
# Перелогиниться
newgrp docker
```

### Очистка места

```bash
# Очистить build artifacts
cargo clean

# Очистить только target
rm -rf target/

# Проверить размер
du -sh target/
```

---

## Чек-лист перед полевыми испытаниями

```
□ Все 185 unit-тестов проходят
□ 5/5 scenarios проходят
□ Clippy и fmt чистые
□ V4L2 работает с vivid (Тест 4)
□ SITL работает (Тест 5)
□ RKNN bridge запускается (Тест 8, stub)
□ Реальная камера работает (Тест 6) — latency < 50ms
□ Реальный FC работает (Тест 7) — heartbeat, arm, mode change
□ Полный pipeline работает (Тест 9)
□ Запись сессии настроена (asciinema/script)
□ SSH доступ настроен
□ Pre-flight checklist пройден (SAFETY.md)
□ Пропеллеры СНЯТЫ для HITL тестов
□ Battery заряжена + 20% margin
□ Safety pilot готов к override
□ Логи сохраняются
```

---

**После прохождения всех тестов система готова к HITL испытаниям (Phase 7)
и реальным полётам (Phase 8).**
