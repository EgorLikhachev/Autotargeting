# Handoff Checklist — передача контекста другому ИИ-агенту

## Как использовать

Скопируй содержимое **всей папки `ai-context/`** в первый промт нового агента
(или прикрепи файлы). Это даст ему полный контекст без чтения кода.

## Минимальный промт для нового агента

```
Ты продолжаешь работу над проектом Auto-Targeting — бортовая система CV для
самолёта (fixed-wing UAV) на RK3588. Ниже — контекст проекта (папка ai-context/).

Прочитай:
1. ai-context/PROJECT_OVERVIEW.md — что это
2. ai-context/CURRENT_STATE.md — что готово
3. ai-context/KNOWN_ISSUES.md — известные проблемы
4. ai-context/AGENT_INSTRUCTIONS.md — как работать

Задача: <опиши, что нужно сделать>

Полная спека: auto-targeting/docs/SDD-SPEC.md.
Доступ к SoC: ssh orangepi@192.168.0.139 (ключ в ~/.ssh/id_ed25519).
Доступ к репо: https://github.com/EgorLikhachev/Autotargeting (ветка main).
```

## Чек-лист перед handoff

- [ ] `CURRENT_STATE.md` актуален (проверь `git log --oneline -5`)
- [ ] `KNOWN_ISSUES.md` отражает последние фиксы
- [ ] Новые решения зафиксированы в `auto-targeting/docs/sdd/decisions.md`
- [ ] `progress.json` обновлён (`auto-targeting/docs/sdd/progress.json`)
- [ ] Незакоммиченных изменений нет (`git status` чистый)
- [ ] Код собирается (`cargo check` из `auto-targeting/`)

## Что НЕ нужно передавать

- Исходный код (агент читает из репо)
- `target/` (gitignored)
- `.zcode/tmp/` (локальные артефакты)
- Логи тестов с SoC (в `/tmp/` на устройстве)

## Версия

**Дата:** 2026-08-31
**Ветка:** `main` @ `33028e6`
**Фаза:** компонентная архитектура завершена (шина+SHM+systemd, soak пройден).
Следующий рубеж: Phase 1.2 (датасет+fine-tune — чинит over-detect #7/#8) и
охлаждение NPU (#13). Ветки: только main. Прогон тестов: `cargo test --workspace --lib`.
