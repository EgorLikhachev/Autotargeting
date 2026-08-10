# Project Overview — Auto-Targeting

## Что это

**Auto-Targeting System** — бортовая система компьютерного зрения для коптера:
получает кадры с камеры → распознаёт объекты → сопровождает выбранную цель →
управляет автопилотом для её удержания в кадре.

**Целевая платформа:** Orange Pi 5 (RK3588, 8× Cortex-A55 + NPU 6 TOPS),
ArduPilot FC, USB/MIPI камера.

## Стек

- **Rust** (edition 2021, MSRV 1.75) — 10 крейтов, основной код
- **C++** — микросервис `rknn-bridge` для NPU-инференса (librknnrt.so 2.3.0)
- **Python** — конвертация моделей (rknn-toolkit2), калибровка
- **MAVLink v2** — связь с автопилотом
- **TOML + figment** — конфигурация (env-override через `AT_` префикс)
- **CI/CD:** GitHub Actions (fmt/clippy/test/coverage/cross-compile aarch64/QEMU/SITL Docker)

## Текущая фаза

**Phase 1.1 (минимальный контур CV) — ЗАКРЫТА** (v0.1.0-phase-1.1).

Полный пайплайн работает на реальном NPU:
- `rknn_init` за 32–39 ms
- NPU-инференс за **27–29 ms** (~34 FPS)
- Эталон даёт **47 детекций** на bus.jpg (class=person, class=bus)
- C++ bridge через zero-copy API + sigmoid → **1342 детекции person** на bus.jpg

## Что НЕ входит (границы Phase 1.1)

- Своя обученная модель (базовая — COCO YOLOv8n, не знает классы стенда)
- CAN-шина, финальные интерфейсы БВК
- Автоматическое сопровождение движущихся целей (MVP — статика)
- Замкнутая петля CV→автопилот (главный риск, отдельное исследование)

## Следующий этап

**Задача 1.2** — свой датасет (палатка/ящик/бензовоз/джип) + fine-tune YOLOv8n +
реальная камера (USB/MIPI) + интеграция full-loop (`run_full()` сейчас stub).

См. [`CURRENT_STATE.md`](CURRENT_STATE.md) для деталей готовности по модулям.
