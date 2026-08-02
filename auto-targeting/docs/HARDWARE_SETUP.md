# Headless Setup — Orange Pi 5

> Пошаговая инструкция по подготовке Orange Pi 5 к железным испытаниям
> auto-targeting без монитора (только SSH + Type-C питание).

## Что нужно

| Компонент | Назначение |
|---|---|
| Orange Pi 5 (RK3588S) | Вычислитель |
| USB камера (Arducam UC-852 или любая UVC) | Видеопоток |
| Флешка (microSD ≥16GB, Class 10) | Загрузочная ОС |
| Картридер | Для записи образа на флешку |
| Type-C БП (≥5V/3A, лучше 5V/4A) | Питание |
| WiFi или Ethernet | SSH доступ |
| Компьютер (Linux/Mac/Windows) | Запись образа + SSH клиент |

## Шаг 1: Выбор ОС

Рекомендую **Armbian** — стабильнее всего для headless setup.

### Альтернативы:
- **Ubuntu 22.04** (от производителя Orange Pi) — официальная, но более тяжёлая
- **Debian 12** — лёгкая, но меньше драйверов

**Рекомендация: Armbian Bookworm (Debian 12 based)**

## Шаг 2: Запись образа на флешку

### На компьютере:

```bash
# 1. Скачать Armbian для Orange Pi 5
#    https://www.armbian.com/orange-pi-5/
#    Выбрать: "Server" (без GUI, ~500MB)

# 2. Проверить что флешка определилась
lsblk
# Например: /dev/sdX (замените X на свою букву!)

# 3. Размонтировать флешку (если примонтирована)
sudo umount /dev/sdX*

# 4. Записать образ (ВНИМАНИЕ: замените /dev/sdX и путь к образу!)
sudo dd if=Armbian_*.img of=/dev/sdX bs=4M conv=fsync status=progress

# Или использовать balenaEtcher / Rufus (Windows/Mac)
```

## Шаг 3: Первая загрузка (headless)

### 3.1 Подготовка WiFi (если используете WiFi, не Ethernet)

Перед вставкой флешки в Orange Pi — отредактируйте на компьютере:

```bash
# Монтируем boot раздел флешки
sudo mkdir -p /mnt/armbian_boot
sudo mount /dev/sdX1 /mnt/armbian_boot

# Создаём файл с WiFi credentials
sudo nano /mnt/armbian_boot/armbian_first_run.txt
```

Содержимое `armbian_first_run.txt`:
```
FR_general_delete_this_file_after_done=done
FR_net_ethernet_enabled=1
FR_net_wifi_enabled=1
FR_net_wifi_ssid=ВАШ_WIFI_SSID
FR_net_wifi_password=ВАШ_WIFI_ПАРОЛЬ
FR_net_static_ip=
FR_net_gateway=
FR_net_dns=8.8.8.8
```

```bash
sudo umount /mnt/armbian_boot
```

### 3.2 Загрузка

1. Вставьте флешку в Orange Pi 5
2. Подключите USB камеру
3. Подключите Type-C БП
4. Дождитесь загрузки (~1-2 минуты, красный LED → зелёный LED)

### 3.3 Поиск IP адреса

На компьютере:

```bash
# Вариант 1: если знаете MAC (на плате)
nmap -sn 192.168.1.0/24 | grep -B 5 "AA:BB:CC:DD:EE:FF"

# Вариант 2: сканировать всю подсеть
nmap -sn 192.168.1.0/24

# Вариант 3: через роутер (DHCP leases)
# Зайти в web-интерфейс роутера → DHCP → Connected devices

# Вариант 4: mDNS (если поддерживается)
ping orangepi5.local
```

### 3.4 Первый вход по SSH

```bash
# По умолчанию Armbian: root / 1234
ssh root@192.168.1.XXX

# При первом входе — попросят сменить пароль и создать пользователя
# Создайте пользователя: orangepi
# Пароль: выберите сложный
```

## Шаг 4: Базовая настройка ОС

После первого входа:

```bash
# Обновить систему
apt update && apt upgrade -y

# Установить timezone
timedatectl set-timezone Europe/Moscow

# Установить hostname
hostnamectl set-hostname auto-targeting-opi

# Установить необходимые пакеты
apt install -y \
    build-essential \
    cmake \
    git \
    curl \
    wget \
    v4l-utils \
    usbutils \
    ffmpeg \
    htop \
    net-tools

# libclang-dev нужен для V4L2 (v4l crate)
apt install -y libclang-dev

# Добавить пользователя в группы video и dialout
usermod -aG video,daemon orangepi

# Перелогиниться
exit
ssh orangepi@192.168.1.XXX
```

## Шаг 5: Установка Rust

```bash
# Установить Rust (stable)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal

# Активировать
source ~/.cargo/env

# Проверить
rustc --version
cargo --version

# Добавить aarch64 target (не нужен на самом OPi, но для понимания)
rustup target list --installed
```

## Шаг 6: Клонирование проекта

```bash
# Клонировать репозиторий
cd ~
git clone https://github.com/EgorLikhachev/Autotatgeting.git auto-targeting
cd auto-targeting

# Проверить структуру
ls -la
cat README.md
```

## Шаг 7: Сборка с V4L2

```bash
# Сборка с поддержкой V4L2 (требует libclang-dev)
cargo build --release --features video-capture/v4l2 -p auto-targeting-cli

# Это займёт ~10-15 минут на Orange Pi 5
# Если не хватает памяти — добавьте swap:
sudo fallocate -l 4G /swapfile
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab

# Проверить бинарник
./target/release/auto-targeting --help
```

## Шаг 8: Проверка камеры

```bash
# 1. Проверить что камера определилась
lsusb
# Должна быть строка вроде: "Bus 001 Device 004: ID ... Arducam ..."

# 2. Проверить V4L2 устройство
ls -la /dev/video*
# Должно быть: /dev/video0

# 3. Проверить поддерживаемые форматы
v4l2-ctl --device /dev/video0 --list-formats-ext

# 4. Записать тестовое видео (5 секунд)
ffmpeg -f v4l2 -input_format mjpeg -video_size 1280x720 -i /dev/video0 \
    -t 5 -y /tmp/camera_test.mp4

# 5. Скачайте на компьютер и проверьте
# На компьютере:
scp orangepi@192.168.1.XXX:/tmp/camera_test.mp4 .
ffplay camera_test.mp4   # или открыть в VLC
```

## Шаг 9: Запуск auto-targeting

```bash
# 1. Smoke test (mock FC, без камеры)
./target/release/auto-targeting --mock-all

# 2. Интерактивный REPL
./target/release/auto-targeting --repl
# В REPL: help, status, arm, scan, select-target 1, abort, quit

# 3. Scenario runner
./target/release/auto-targeting scenario --all sim/scenarios/

# 4. С реальной камерой (mock FC)
# Создать конфиг
cp config.example.toml config.camera.toml
nano config.camera.toml
# Изменить:
#   [video]
#   device = "/dev/video0"
#   format = "mjpeg"
#   [fc]
#   adapter = "mock"

./target/release/auto-targeting --config config.camera.toml --repl
```

## Шаг 10: Запуск всех железных тестов

```bash
# Скачать проверочный скрипт
chmod +x scripts/run_hardware_tests.sh
./scripts/run_hardware_tests.sh
```

Скрипт проверит:
- ✅ Наличие камеры
- ✅ V4L2 форматы
- ✅ Запись видео
- ✅ Smoke test
- ✅ REPL базовый цикл
- ✅ Scenario suite
- ✅ V4l2Source probe

## Troubleshooting

### Камера не определяется

```bash
# Проверить USB
lsusb
dmesg | tail -30

# Перезагрузить USB
echo 0 | sudo tee /sys/bus/usb/devices/usb1/authorized
echo 1 | sudo tee /sys/bus/usb/devices/usb1/authorized

# Проверить питание (камере нужно 500mA)
lsusb -v | grep -i "max power"
```

### Не хватает памяти для сборки

```bash
# Добавить swap (см. Шаг 7)
sudo fallocate -l 4G /swapfile
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile

# Или собирать на dev-машине и кросс-компилировать:
# На dev-машине:
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu --features video-capture/v4l2
scp target/aarch64-unknown-linux-gnu/release/auto-targeting orangepi@IP:~/
```

### SSH обрывается

```bash
# Настроить keepalive в ~/.ssh/config на компьютере:
Host orangepi
    HostName 192.168.1.XXX
    User orangepi
    ServerAliveInterval 60
    ServerAliveCountMax 3

# Или использовать tmux на Orange Pi:
ssh orangepi@IP
tmux new -s auto-targeting
# ... работа ...
# Ctrl+B, D — отключиться (процессы продолжают)
# tmux attach -t auto-targeting — переподключиться
```

### Cargo build слишком медленный

```bash
# 1. Использовать --release с fewer codegen units
cargo build --release --features video-capture/v4l2 -j 4

# 2. Или кросс-компиляция с dev-машины (в 10x быстрее)
# На dev-машине:
rustup target add aarch64-unknown-linux-gnu
sudo apt install gcc-aarch64-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu --features video-capture/v4l2

# Скопировать на Orange Pi:
scp target/aarch64-unknown-linux-gnu/release/auto-targeting orangepi@IP:~/
```

## Готовность к испытаниям

После прохождения всех шагов у вас должно быть:

- [ ] Orange Pi 5 загружается с флешки
- [ ] SSH доступ работает
- [ ] Rust установлен
- [ ] Проект клонирован и собран
- [ ] Камера определяется (`lsusb` + `/dev/video0`)
- [ `v4l2-ctl` показывает форматы
- [ ] FFmpeg записывает видео
- [ ] `--mock-all` smoke test проходит
- [ ] REPL работает
- [ ] Scenario suite проходит (5/5)

Если все пункты выполнены — **система готова к тестам с реальным FC** (когда будет SpeedyBee F405).

См. `docs/MANUAL_TESTING.md` для детальных инструкций по каждому тесту.
