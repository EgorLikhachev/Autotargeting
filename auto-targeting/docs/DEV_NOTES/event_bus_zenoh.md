# Заметки по разработке: шина событий на Zenoh (D-014)

**Дата:** 2026-08-18 · **Крейт:** `crates/event-bus` · **Исследование:** [BUS_SELECTION.md](../BUS_SELECTION.md)
**Статус миграции:** детектор (TG26-35) работает через шину в боевом
контуре — `at/detections` + `at/status/detector`. Полный план перевода
остальных компонентов — [BUS_MIGRATION_PLAN.md](../BUS_MIGRATION_PLAN.md).

## 1. Что реализовано

Типизированная шь событий/данных на **Zenoh 1.10** (peer-to-peer, без
брокера): `EventBus::listen/connect` (фиксированная топология, multicast-
scouting выключен), `TypedPublisher<T>`/`TypedSubscriber<T>` поверх
serde_json, темы проекта (`at/detections`, `at/telemetry`, `at/commands`,
`at/config`, `at/status/{c}`), payload-типы `DetectionsFrame`
(common::Detection) и `TelemetrySample`.

Кадры через шину **не** ходят — им принадлежит SHM-кольцо (D-013);
шина — события и лёгкие данные (R2).

## 2. Результаты прототипа (критерий «обмен + производительность на x86 и ARM»)

Латентность one-way (RTT/2, JSON, 2 процесса, loopback):

### x86_64 (Windows dev-хост, tcp/127.0.0.1:17447)

| Размер | p50 | p95 | p99 |
|---|---|---|---|
| 64 B | **40.4 мкс** | 50.2 | 61.6 |
| 1 KiB | 52.4 мкс | 71.8 | 80.3 |
| 8 KiB | 124.3 мкс | 179.9 | 204.8 |

### RK3588 (aarch64, та же топология)

| Размер | zenoh p50 / p95 / p99 | UDS-базлайн p50 |
|---|---|---|
| 64 B | **588 / 861 / 901 мкс** | 28 мкс |
| 1 KiB | 814 / 859 / 879 мкс | 376 мкс |
| 8 KiB | 627 / 674 / 1092 мкс | 406 мкс |

**Против требования R3 (≤1 мс цель / ≤5 мкс допуск):** zenoh на ARM
укладывается в цель по p50/p95 (макс 1.09 мс p99 @8KiB — в допуске ×5).
Оверхед против сырого UDS — ~10–20× на малых сообщениях (стек TCP +
фрейминг zenoh + serde); для наших 10–50 Гц телеметрии и 30 Гц детекций
запас достаточный. Резерв оптимизации: unix-сокет-транспорт zenoh вместо
tcp/lo (не исследован в прототипе — TCP покрывает и будущий R10).

## 3. Зависимости и размер (критерий «оценены зависимости и размер»)

| Метрика | Значение |
|---|---|
| Крейтов `zenoh*` в lock-файле | 26 |
| Всего транзитивных зависимостей event-bus | 427 (workspace-wide lock: 593) |
| Бинарник bus_latency (ARM, release, strip=symbols) | **10.7 МБ** |
| Бинарник bus_latency (x86) | 10.6 МБ |
| Внешних системных зависимостей | **0** (чистый Rust, TLS не подключён) |
| Vendoring | стандартный `cargo vendor` (проверен сборкой офлайн не был — TODO) |

Для сравнения: rknn-bridge-клиент (UDS) — 1 МБ, но без типов/QoS/второго
хоста. 10 МБ на eMMC стенда — приемлемо (диск 57 ГБ).

## 4. Сложность интеграции (критерий)

- API после 1–2 итераций — прямолинейный (`EventBus::listen/connect`,
  `publisher::<T>/subscriber::<T>`); основная боль — **zenoh 1.10 сделал
  `Config` непрозрачным**: программная настройка только через
  `insert_json5` (документированный путь; закодирован в `base_config`).
- Runtime-требование: **multi-thread tokio** (current-thread паникует —
  ошибка zenoh-runtime). Для нашего стека — уже так.
- Первые сообщения после `declare_*` теряются, пока декларации
  расходятся между peer-ами: встроили settle-паузу 300–400 мс;
  production-фикс — declare заранее при старте компонента или
  `open.return_conditions.declares=true` (в реестре будущих работ).
- Windows/x86 и Linux/ARM: один код, сборка без платформенных правок.

## 5. Ограничения (зафиксированы)

1. Транспорт прототипа — TCP/loopback; unix-транспорт zenoh и SHM-режим
   не включались (латентность на ARM можно ещё ужать).
2. QoS не настраивался (best-effort put); reliable-режим для
   `at/commands` — при интеграции (zenoh reliability в консистентности
   put опций).
3. Serialize = JSON; CBOR/bincode для 8 KiB-детекций даст ~30% меньше
   payload — в реестре.
4. Scouting выключен: подключение — по явному endpoint; systemd-порядок
   старта (кто listener) — при интеграции.
5. Брокера нет и не планируется: топология фиксированная (R9).

## 6. Воспроизведение

```bash
# На любом хосте (два терминала):
cargo run --release -p event-bus --example bus_latency -- server zenoh
cargo run --release -p event-bus --example bus_latency -- client zenoh 300
# Unix-базлайн:
... -- server uds   /   ... -- client uds 300
cargo test -p event-bus   # in-process roundtrip
```
