# Тест альтернативной камеры: Sony PlayStation Eye (OV534 + OV7721)

**Дата:** 2026-08-15/16 · **Устройство:** Orange Pi 5 (`orangepi@192.168.0.139`)
**Стенд:** та же конфигурация, что для Arducam OV9782 (см.
[HARDWARE_TEST_RESULTS.md](HARDWARE_TEST_RESULTS.md) §2.7a) — RK3588, Debian 12,
ядро `6.1.99-rockchip-rk3588`, librknnrt 2.3.0.

**Итог: камера ПОЛНОСТЬЮ РАБОЧА на стенде** — после сборки kernel-модуля
`gspca_ov534` (вендорское ядро поставляется без него) и серии фиксов в нашем
коде. End-to-end demo (камера → NPU → детекции → MP4) отработал: 84 кадра,
80 906 детекций за 15 с.

---

## 1. Идентификация

| Параметр | Значение |
|---|---|
| Устройство | Sony PlayStation Eye (PS3, 2007) |
| USB | `1415:2000` OmniVision/Nam Tai, «USB Camera-B4.09.24.1», USB 2.0 |
| Мост / сенсор | OV534 / OV7721 |
| Класс видео-интерфейса | **Vendor Specific (0xFF)** — НЕ UVC |
| Драйвер | `gspca_ov534` (GSPCA subdriver, mainline) |
| Аудио | 4-микрофонный массив, стандартный USB-Audio — работает из коробки |

## 2. Блокер: вендорское ядро без драйвера

PS Eye — не-UVC камера: её видео-интерфейс использует проприетарный протокол
OV534 (bulk-эндпоинты). Драйвер `gspca_ov534` в mainline есть, но ядро
Orange Pi собрано со всеми GSPCA-сабдрайверами `is not set`
(в дереве модулей — один `gspca_main.ko`). Аудиодрайвер камеру при этом
забирает (карта `CameraB409241`), видео-ноды нет вообще.

## 3. Remediation: сборка out-of-tree модуля (успешно)

`CONFIG_MODVERSIONS` в ядре выключен → out-of-tree модуль соберётся без
CRC-проверки символов. Ветка [orangepi-xunlong/linux-orangepi](
https://github.com/orangepi-xunlong/linux-orangepi) `orange-pi-6.1-rk35xx`
— ровно 6.1.99.

```bash
# На Orange Pi 5:
sudo apt install -y libelf-dev
git clone --depth 1 --branch orange-pi-6.1-rk35xx --single-branch \
    https://github.com/orangepi-xunlong/linux-orangepi.git ~/linux-opi
cd ~/linux-opi
zcat /proc/config.gz > .config
./scripts/config --module USB_GSPCA_OV534   # включаем сабдрайвер
./scripts/config --disable LOCALVERSION_AUTO                # иначе git-суффикс
./scripts/config --set-str LOCALVERSION '-rockchip-rk3588'  # vermagic = running
yes '' | make ARCH=arm64 olddefconfig
make ARCH=arm64 -j8 modules_prepare          # ~4 мин
make ARCH=arm64 M=drivers/media/usb/gspca modules
# UTS_RELEASE = "6.1.99-rockchip-rk3588" — совпал с running-ядром 1:1

sudo modprobe gspca_main                     # вендорский, из /lib/modules
sudo insmod drivers/media/usb/gspca/gspca_ov534.ko
# → gspca_main: ov534-2.14.0 probing 1415:2000 → /dev/video0

# Персистентность (переживает ребут):
sudo cp drivers/media/usb/gspca/gspca_ov534.ko /lib/modules/$(uname -r)/extra/
sudo depmod -a
echo gspca_ov534 | sudo tee /etc/modules-load.d/gspca_ov534.conf
```

Установлено на стенде — камера поднимается автоматически после загрузки.

## 4. Форматы и частоты (посре драйвера)

Сенсор отдаёт **только несжатое**: YUYV 4:2:2 и GRBG Bayer. MJPG нет —
decode-путь меняется с JPEG-декодера на конверсию пикселей.

| Режим | Целевой FPS | Реальный (v4l2-ctl, 200 кадров) | Кадр | Полоса |
|---|---|---|---|---|
| YUYV 640×480 | 60 | **60.02** | 614 KB | 36.9 MB/s |
| YUYV 640×480 | 30 | 30.01 | 614 KB | 18.4 MB/s |
| YUYV 320×240 | 187 | **184.61** | 150 KB | 28.4 MB/s |
| YUYV 320×240 | 125 | 124.90 | 150 KB | 19.2 MB/s |
| YUYV 320×240 | 100 | 99.92 | 150 KB | 15.4 MB/s |
| YUYV 320×240 | 60 | 59.95 | 150 KB | 9.2 MB/s |

Управление: brightness/contrast/saturation/hue, авто-WB, авто-gain,
exposure (в ручном режиме).

## 5. Найденные и исправленные баги (по ходу теста)

Четыре бага всплыли только на этой камере — все исправлены в `main`:

| # | Баг | Симптом | Фикс (commit) |
|---|---|---|---|
| 1 | `v4l` crate виснет на gspca: `start()` ок, но `recv()` не отдаёт ни кадра (rc=124 по timeout), при том что v4l2-ctl стримит на полной скорости | Зависание бенчмарка | `--backend direct` (V4l2DirectSource) — примеры получили выбор бэкенда |
| 2 | `v4l2_direct.rs`: `data_len` не определён (E0425) — видно только при сборке с `v4l2`+`v4l2-direct` вместе | Сборка падает | `bytesused.min(mapped.len)` (`a02b89a`) |
| 3 | `S_PARM timeperframe` на 4 байта раньше: numerator@8/denominator@12 вместо 12/16 | Камера оставалась на дефолтных 30 FPS (интервал ровно 33.4 мс) при запросе 187 | offsets 12/16 (`280c98e`) |
| 4 | `yuyv_to_rgb24`/`yuyv_to_nv12`: чтение `data[i+3]` последней пары пикселей за границей | panic на первом же кадре (index 153601 > len 153600) | Попарная обработка [Y0,U,Y1,V] + регресс-тесты (`ec10230`) |
| 5 | `live_camera_demo`: pump-цикл `try_send().is_err() => break` рвался на **Full** (не только Closed) | Захват умирал ровно на 5-м кадре — и эта же ошибка была ошибочно списана на `v4l` crate ещё в Arducam-сессии | `pump_frames()`: break только по Closed (`a15e427`) |

Плюс: rknn-bridge (однопоточный) мог зависнуть на мёртвом клиентском сокете
от прошлой аварийной сессии — лечится рестартом. TODO на коннект-таймауты
в bridge (см. KNOWN_ISSUES).

## 6. Rust-бенчмарки (`camera_latency --format yuyv --backend direct`)

### Sequential (capture + YUYV→RGB24 decode в одном цикле)

| Режим | Capture p50 | Decode p50 | **Total p50** | Sustained |
|---|---|---|---|---|
| 640×480@60 | 9.60 ms | 7.03 ms | **16.63 ms** (=60.0 FPS) | 46.1 FPS* |
| 320×240@187 | 4.51 ms | 0.86 ms | **5.37 ms** (=186 FPS) | 131.4 FPS* |

\* sustained ниже 1/total из-за стартового заполнения канала (камера
стартует ~0.4 с) — интервал p50 показывает реальный темп конвейера.

**Пайплайн упирается только в камеру**: total p50 = периоду кадра камеры
(16.63 мс ≈ 1/60, 5.37 мс ≈ 1/186) — capture+decode успевают за каждый кадр.

### Pipeline (capture-only потолок, decode вне пути)

| Режим | Capture p50/p95 | Sustained |
|---|---|---|
| 640×480@60 | 16.64 / 16.75 ms | 46.0 FPS |
| 320×240@100 | 10.00 / 10.11 ms | 77.9 FPS |
| 320×240@187 | 5.37 / 5.39 ms | 126.7 FPS |

## 7. Live demo end-to-end (камера → NPU → детекции → MP4)

`live_camera_demo --format yuyv --backend direct 640x480@60, 15 с`:

| Метрика | Значение |
|---|---|
| Кадров захвачено | **84** (vs 5 до фикса pump-цикла) |
| Детекций всего | **80 906** (~963/кадр — известная проблема NMS/int8, KNOWN_ISSUES №7/8) |
| Inference latency | avg 90 ms (min 86 / max 217) — round-trip c base64+JSON |
| Sustained FPS конвейера | 5.6 (лимит: инференс-RT, не камера) |
| RSS | 20.6 MB |
| CPU / NPU temp | 38.8 / 37.9 °C |

Артефакты: `output/pseye_live/` (84 аннотированных JPEG, detections.jsonl,
telemetry.jsonl, summary.json, `processed.mp4` 1.7 MB).

## 8. Сравнение с Arducam OV9782

| | **Arducam OV9782 USB** | **Sony PS Eye** |
|---|---|---|
| Год/класс | современная UVC, global shutter | 2007, не-UVC (OV534), rolling shutter |
| Формат | MJPG (сжатие в камере) | YUYV/GRBG без сжатия |
| Макс. режим | 640×480@100 (MJPG), 1280×720@60 | 640×480@60, **320×240@187** |
| Драйвер на стоковом ядре | да (UVC built-in) | **нет** — нужна сборка модуля (сделано) |
| Capture backend | оба (v4l / direct) | **только direct** (v4l crate виснет) |
| Capture+decode total p50 (640×480) | 32 ms (MJPG decode 9 ms; capture 23 ms через v4l) | **16.6 ms** (YUYV decode 7 ms) |
| Минимальный latency-режим | 640×480@100 → 10 ms/кадр | **320×240@187 → 5.4 мс/кадр** |
| Полоса USB | 1.78 MB/s (сжатие) | до 36.9 MB/s (raw) — ок для USB 2.0 |
| Аудио | нет | 4-мик массив (работает) |
| NPU-конвейер | end-to-end OK | **end-to-end OK** (84 кадра, 80 906 детекций) |

## 9. Выводы и рекомендации

1. **PS Eye совместим со стендом** после установки модуля; ядро менять не
   пришлось (NPU-стек не тронут).
2. Для **минимальной задержки** захвата PS Eye — лучший из протестированных:
   320×240@187 даёт кадр каждые 5.4 мс; полный capture+decode укладывается
   в период кадра камеры.
3. Для **разрешения** — OV9782 (1280×720) предпочтительнее; PS Eye выше
   640×480 не умеет.
4. Rolling shutter PS Eye — минус для быстрого panoramiрования на БВС;
   OV9782 (global shutter) остаётся референсом.
5. Для PS Eye использовать только `--backend direct`; `v4l` crate на gspca
   зависает (задокументировано в KNOWN_ISSUES).
6. Sustained-FPS конвейера ограничен **инференсом** (90 мс round-trip с
   base64): следующий шаг для обоих камер один — SCM_RIGHTS/SHM вместо
   base64 (KNOWN_ISSUES №2).

## 10. Воспроизведение

```bash
# Модуль (один раз, уже установлен на стенде — см. §3)
v4l2-ctl -d /dev/video0 --list-formats-ext          # форматы

cd ~/auto-targeting/auto-targeting
cargo build --release -p video-capture --example camera_latency \
    --features 'v4l2,v4l2-direct'
cargo build --release -p cv-inference --example live_camera_demo \
    --features 'cpu-onnx,v4l2-cam,v4l2-direct-cam'

# Бенчмарк
./target/release/examples/camera_latency --device /dev/video0 \
    --width 320 --height 240 --fps 187 --count 400 \
    --format yuyv --backend direct --pipeline

# Live demo (bridge запустить заранее)
cd rknn-bridge/build && nohup ./rknn-bridge >/tmp/bridge.log 2>&1 &
cd ~/auto-targeting/auto-targeting
./target/release/examples/live_camera_demo --device /dev/video0 \
    --width 640 --height 480 --fps 60 --seconds 15 \
    --format yuyv --backend direct --output output/pseye_live
ffmpeg -framerate 6 -i output/pseye_live/frames/seq_%06d.jpg \
    -c:v libx264 -pix_fmt yuv420p output/pseye_live/processed.mp4
```
