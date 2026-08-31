# Project Overview — Auto-Targeting

## Что это

**Auto-Targeting System** — бортовая система компьютерного зрения для
**самолёта (fixed-wing UAV)**: получает кадры с камеры → распознаёт объекты →
сопровождает выбранную цель → управляет автопилотом для её удержания в кадре.
Всё за многоуровневой защитой от осцилляций (state machine, watchdogs, deadband,
oscillation detector).

**Целевая платформа:** Orange Pi 5 (RK3588, 8× Cortex-A55 + NPU 6 TOPS),
ArduPilot FC, USB/MIPI камера.

## Стек

- **Rust** (edition 2021, MSRV 1.75) — 10 крейтов, основной код
- **C++** — микросервис `rknn-bridge` для NPU-инференса (librknnrt.so 2.3.0)
- **Python** — конвертация моделей (rknn-toolkit2), калибровка
- **MAVLink v2** — связь с автопилотом
- **TOML + figment** — конфигурация (env-override через `AT_` префикс)
- **CI/CD:** GitHub Actions (fmt/clippy/test/coverage/cross-compile aarch64/QEMU/SITL Docker + docs lint)

## Текущая фаза

**Компонентная архитектура завершена (2026-08-31):** кадры — SHM-кольцо,
события — шина Zenoh, все компоненты — независимые сервисы systemd.

Ключевые цифры (RK3588, всё измерено на живом железе):
- Детектор: **27 FPS** (SHM-путь, чистый NPU 29.5 мс; было 9.9 на base64)
- Полный контур camera→detector→tracker→commander→FC — на шине,
  замкнут и проверен на ArduPlane SITL (128 коррекций)
- Soak 30 мин: 8/8 сервисов, 0 рестартов, RSS +5.4 МБ
- ⚠️ NPU sustained 83–87 °C — нужно охлаждение до полётов

## Что НЕ входит (границы Phase 1.1)

- Своя обученная модель (базовая — COCO YOLOv8n, не знает классы стенда)
- CAN-шина, финальные интерфейсы БВК
- Автоматическое сопровождение движущихся целей (MVP — статика)
- Замкнутая петля CV→автопилот (главный риск, отдельное исследование)

## Следующий этап

**Задача 1.2** — свой датасет (палатка/ящик/бензовоз/джип) + fine-tune YOLOv8n +
подключить `V4l2DirectSource` в live-demo (модуль готов) + интеграция full-loop
(`run_full()` сейчас stub).

См. [`CURRENT_STATE.md`](CURRENT_STATE.md) для деталей готовности по модулям.
