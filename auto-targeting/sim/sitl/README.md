# SITL — Software In The Loop

> **Статус:** ✅ Готов к запуску (требует Docker на хост-машине)

ArduPilot SITL (Software In The Loop) — симулятор, который запускает тот же
firmware, что и на реальном полётном контроллере, но в user-space Linux.
Позволяет тестировать auto-targeting без реального железа.

## Быстрый старт

```bash
# 1. Запустить SITL (первая сборка ~10-15 минут, потом мгновенно)
./sim/sitl/run_sitl.sh start

# 2. Проверить MAVLink соединение
./sim/sitl/run_sitl.sh test

# 3. Запустить auto-targeting против SITL
cargo run -p auto-targeting-cli -- \
  --config sim/sitl/sitl-config.toml -- --repl

# 4. В REPL:
#    status        → проверить heartbeat
#    arm           → армировать (SITL ответит)
#    set-mode guided
#    abort         → RTL
#    quit
```

## Команды run_sitl.sh

```bash
./sim/sitl/run_sitl.sh start    # Запустить SITL (сборка при первом запуске)
./sim/sitl/run_sitl.sh stop     # Остановить SITL
./sim/sitl/run_sitl.sh restart  # Перезапустить
./sim/sitl/run_sitl.sh status   # Статус контейнера и портов
./sim/sitl/run_sitl.sh logs     # Логи SITL (Ctrl+C для выхода)
./sim/sitl/run_sitl.sh test     # Проверить MAVLink соединение
./sim/sitl/run_sitl.sh clean    # Удалить образ и логи (полная очистка)
./sim/sitl/run_sitl.sh help     # Справка
```

## Что входит

| Файл | Назначение |
|---|---|
| `Dockerfile` | Собирает ArduPilot из исходников (Plane-4.5) |
| `docker-compose.yml` | Конфигурация контейнера, проброс портов |
| `sitl_params.parm` | Параметры ArduPilot (arm без RC, безопасные настройки) |
| `sitl-config.toml` | Конфиг auto-targeting для работы с SITL |
| `run_sitl.sh` | Helper-скрипт для управления SITL |

## Порты

| Порт | Протокол | Назначение |
|---|---|---|
| 5760 | TCP | MAVLink (основной — для auto-targeting и QGroundControl) |
| 5762 | TCP | MAVLink #2 (для второго ground station) |
| 5763 | UDP | MAVLink UDP (альтернативный) |
| 14550 | UDP | Companion computer (если используем udpin) |

## Подключение auto-targeting

### Через TCP (рекомендуется)

```toml
[fc]
adapter = "ardupilot-mavlink"
endpoint = "tcpout:127.0.0.1:5760"
```

### Через UDP (альтернатива)

```toml
[fc]
adapter = "ardupilot-mavlink"
endpoint = "udpin:0.0.0.0:14550"
```

SITL в docker-compose.yml настроен на `--out=udp:0.0.0.0:14550`.

## Интеграционные тесты

В `crates/fc-adapter/tests/sitl_integration.rs` — 7 тестов против SITL:

```bash
# Запустить все SITL-тесты (SITL должен быть запущен!)
./sim/sitl/run_sitl.sh start
cargo test -p fc-adapter --test sitl_integration -- --include-ignored

# Конкретный тест
cargo test -p fc-adapter --test sitl_integration -- --include-ignored test_sitl_heartbeat
```

### Тесты

| Тест | Что проверяет |
|---|---|
| `test_sitl_connect` | Подключение к SITL по TCP |
| `test_sitl_heartbeat` | Получение HEARTBEAT за 5 сек |
| `test_sitl_arm_disarm` | Команды arm/disarm |
| `test_sitl_mode_change` | Смена режима (GUIDED, LOITER, RTL) |
| `test_sitl_attitude` | Получение ATTITUDE telemetry |
| `test_sitl_heartbeat_stability` | Стабильность heartbeat (10 сек, < 5% stale) |
| `test_sitl_heartbeat_loss_detection` | Детекция потери связи |

## QGroundControl (опционально)

Для визуального контроля можно подключить QGroundControl:

```bash
# QGroundControl автоматически найдёт SITL на 127.0.0.1:5760
qgroundcontrol &
```

В QGroundControl вы увидите:
- Текущий режим (MANUAL, GUIDED, RTL и т.д.)
- Armed/disarmed статус
- Attitude (roll/pitch/yaw)
- GPS позицию (симулированную)
- Map с позицией дрона

## MAVProxy (опционально)

Альтернатива QGroundControl — MAVProxy в терминале:

```bash
pip install mavproxy
mavproxy.py --master=tcp:127.0.0.1:5760

# Команды MAVProxy:
# > status
# > mode
# > arm throttle
# > disarm
# > mode guided
# > mode rtl
# > output add udpout:127.0.0.1:14551
```

## Troubleshooting

### "Docker не установлен"

```bash
# Ubuntu/Debian
sudo apt install -y docker.io docker-compose-plugin
sudo systemctl start docker
sudo usermod -aG docker $USER  # перелогиниться после
```

### "Порт 5760 уже занят"

```bash
# Проверить, что использует порт
sudo lsof -i :5760

# Остановить старый SITL
./sim/sitl/run_sitl.sh stop
```

### "SITL не запускается"

```bash
# Посмотреть логи
./sim/sitl/run_sitl.sh logs

# Пересобрать образ (если был сбой сборки)
./sim/sitl/run_sitl.sh clean
./sim/sitl/run_sitl.sh start
```

### "HEARTBEAT не получен"

```bash
# 1. Проверить, что SITL запущен
./sim/sitl/run_sitl.sh status

# 2. Проверить TCP соединение
nc -zv 127.0.0.1 5760

# 3. Проверить через MAVProxy
mavproxy.py --master=tcp:127.0.0.1:5760

# 4. Посмотреть логи SITL
./sim/sitl/run_sitl.sh logs | grep -i "mavlink\|heartbeat"
```

### "arm failed"

SITL может отказывать в arm, если:
- Нет GPS lock (в симуляции должен быть автоматически)
- Battery low (проверьте параметры в sitl_params.parm)
- Pre-arm checks не пройдены

Решение: в `sitl_params.parm` уже установлено `ARMING_CHECK 0` (отключить проверки).

### Сборка занимает слишком долго

Первая сборка ArduPilot из исходников — 10-15 минут. Это нормально.
Последующие запуски используют Docker cache и запускаются за секунды.

Если сборка зависла:
```bash
# Очистить кэш Docker
docker system prune -a
./sim/sitl/run_sitl.sh start
```

## Использование с реальным железом

После успешного тестирования с SITL, переход к реальному FC (SpeedyBee F405):

```bash
# 1. Подключить FC по USB
ls /dev/ttyACM*

# 2. Изменить конфиг
# endpoint = "serial:/dev/ttyACM0:115200"

# 3. ⚠️ СНЯТЬ ПРОПЕЛЛЕРЫ перед запуском!

# 4. Запустить
cargo run --release -- --config config.real_fc.toml -- --repl
```

См. `docs/MANUAL_TESTING.md` § Тест 7 для подробностей.
