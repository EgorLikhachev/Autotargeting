# AI Context — Auto-Targeting

> **Цель этой папки:** дать новому ИИ-агенту (или разработчику) полную картину
> проекта **без чтения исходного кода**. Каждый документ автономен и читается
> за 1–2 минуты. Вместе они покрывают: что это, как устроено, что готово, что
> делать дальше.
>
> Это **переносимый контекст** — скопируйте содержимое `ai-context/` в чат
> нового агента, и он сможет продолжить работу с того же места.

## Когда использовать

- **Handoff между агентами:** следующий ИИ-ассистент получает эти файлы как
  стартовый контекст → не нужно читать код или всю SDD-SPEC.
- **Onboarding нового разработчика:** быстрый вход в проект без погружения в
  исходники.
- **Перед планированием следующего этапа:** актуальный статус + известные
  проблемы под рукой.

## Состав (читать сверху вниз)

| # | Файл | Что даёт |
|---|---|---|
| 1 | [`PROJECT_OVERVIEW.md`](PROJECT_OVERVIEW.md) | 1-страничная выжимка: что строим, стек, целевая платформа |
| 2 | [`GLOSSARY.md`](GLOSSARY.md) | Термины проекта (RK3588, NPU, RKNN, MAVLink, SITL, FOV, NED, ROI…) |
| 3 | [`CURRENT_STATE.md`](CURRENT_STATE.md) | Что готово по модулям + реальные цифры с железа (FPS, latency, температура) |
| 4 | [`ARCHITECTURE_QUICK.md`](ARCHITECTURE_QUICK.md) | Слои + Mermaid-диаграмма + ключевые trait-контракты |
| 5 | [`KEY_FILES.md`](KEY_FILES.md) | Map: что лежит в каком крейте, точки входа, как запускать |
| 6 | [`KNOWN_ISSUES.md`](KNOWN_ISSUES.md) | 5 TODO из аудита + средовые ограничения |
| 7 | [`AGENT_INSTRUCTIONS.md`](AGENT_INSTRUCTIONS.md) | Как новому агенту работать: SDD-workflow, anti-loop лимиты, правила коммитов |
| 8 | [`HANDOFF_CHECKLIST.md`](HANDOFF_CHECKLIST.md) | Чек-лист передачи контекста другому агенту |

## Связь с полными документами

Эта папка — **выжимка**. Полные версии (пути от git-root):
- `README.md` (git-root) — канонический front-door: бейджи, quickstart, конфиг
- `CONTRIBUTING.md` / `CHANGELOG.md` / `SECURITY.md` (git-root) — OSS-конвенции
- `auto-targeting/docs/SDD-SPEC.md` — спецификация (единственный источник истины)
- `auto-targeting/docs/PROJECT_REPORT.md` — полный отчёт о проделанной работе
- `auto-targeting/docs/HARDWARE_TEST_RESULTS.md` — результаты тестирования на RK3588
- `auto-targeting/docs/sdd/decisions.md` — журнал архитектурных решений (D-001…D-011)
- `auto-targeting/docs/sdd/progress.json` — трекер этапов (machine-readable)
- `auto-targeting/docs/BUS_MIGRATION_PLAN.md` — план перевода всех компонентов на шину Zenoh

## Версия контекста

**Дата генерации:** 2026-08-14
**Ветка:** `main` @ `33028e6`
**Phase:** 1.1 закрыта (minimal CV loop работает end-to-end на NPU, validated на железе).
Репозиторий оформлен по OSS-конвенциям (README/LICENSE/CONTRIBUTING/CHANGELOG на git-root).
