# Agent Instructions — как новому ИИ-агенту работать с проектом

## Принцип: Spec-Driven Development (SDD)

**SDD-SPEC.md — единственный источник истины.** Любая фича/изменение начинаются
с правки спецификации, затем агент читает diff и генерирует код. Код без
изменения спеки — нелегитимен (кроме bugfix'ов, приводящих код в соответствие
со спекой).

Полная спека: `auto-targeting/docs/SDD-SPEC.md` (920 строк, 15 разделов).

## Anti-loop политика (ОБЯЗАТЕЛЬНО)

- **Максимум 5 шагов** на любую автоматическую подзадачу. При достижении лимита
  вывести: `⚠️ Достигнут лимит итераций для <подзадача>. Требуется решение человека.`
- **Если разница между текущим и ожидаемым состоянием не уменьшается за 3
  последовательных шага** — остановиться, переключиться на альтернативный подход.
- **Защитный тайм-аут:** любое авт-исправление кода не дольше 10 шагов без
  внешнего подтверждения.
- Веди лог решений в `auto-targeting/docs/sdd/decisions.md` (контекст → решение
  → последствия). При зацикливании — откат по этому логу.

## Правила коммитов

Полная конвенция — в `CONTRIBUTING.md` (git-root). Кратко:
- **Conventional Commits**: `feat(...):`, `fix(...):`, `docs(...):`, `chore(...)`, `perf(...)`, `refactor(...)`
- Скоупы — имена крейтов: `common`, `video-capture`, `cv-inference`, `yolov8`, `rknn`, `hw`, `ci`, `deps`...
- Каждый коммит — с подробным body (что, зачем, какой критерий закрывает)
- По умолчанию работаем в `main` (малые правки); фича-ветки — для крупных
  этапов, после merge — удалять локально и на remote (текущее состояние: main-only)
- Не переписывай историю без явного разрешения пользователя
- Коммить и пушь только по явной просьбе пользователя
- Safety-critical изменения (`commander/`, `fc-adapter/`, `target-tracker/`, watchdogs) — требуют 2 аппрува + тесты state-machine/oscillation

## Платформа и сборка

- **Целевая платформа:** Linux/aarch64 (RK3588). На Windows/x86 часть крейтов
  не собирается (V4L2, sd-notify, Unix-сокеты) — это нормально.
- **Проверка сборки на x86 Windows:** `cargo check -p yolov8 -p cv-inference -p
  cv-visualizer -p system-telemetry -p common --lib` (Linux-only крейты skip)
- **ONNX (cpu-onnx):** только x86/Linux. На RK3588 — NPU (RKNN).
- **Релизный профиль:** `panic = "abort"`, `lto = "thin"`, `opt-level = 3`.

## Доступ к SoC (Orange Pi 5)

- IP: `192.168.0.139`, user: `orangepi`, пароль: `orangepi`
- SSH-ключ уже установлен (`~/.ssh/id_ed25519`)
- `librknnrt.so` 2.3.0 в `/usr/lib/`, `rknn_api.h` в `~/rknn-headers/`
- Rust установлен (`source ~/.cargo/env`)
- Python venv: `source ~/rknn-venv/bin/activate` (rknn-toolkit2 2.3.0)
- Модель: `~/auto-targeting/auto-targeting/models/yolov8n_int8.rknn`
- Сборка bridge: `cd ~/auto-targeting/auto-targeting/rknn-bridge/build &&
  cmake --build . -j4`

## Первый шаг нового агента

1. Прочитай эту папку (`ai-context/`) целиком
2. Прочитай git-root `README.md` (quickstart, конфиг) + `CONTRIBUTING.md` (конвенции)
3. Прочитай `auto-targeting/docs/SDD-SPEC.md` (если нужен глубокий контекст)
4. Проверь `git log --oneline -10` — актуальная история
5. Спроси пользователя, что делать дальше
6. Перед любым изменением — убедись, что понимаешь anti-loop политику
