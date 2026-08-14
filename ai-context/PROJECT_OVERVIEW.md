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

**Phase 1.1 (минимальный контур CV) — ЗАКРЫТА и hardware-validated**
(`v0.1.0-phase-1.1`, после чего — оформление репозитория на `main`).

Полный пайплайн работает на реальном NPU и проверен live:
- `rknn_init` за 32–39 ms
- NPU-инференс за **27–29 ms** (~34 FPS)
- Эталон даёт детекции на bus.jpg (class=person, class=bus)
- **Live camera demo** (2026-08-13): камера → NPU → аннотированное видео,
  5171 детекция, CPU/NPU temp 45.3/44.4 °C

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
