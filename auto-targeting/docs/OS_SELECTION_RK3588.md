# Выбор дистрибутива Linux для Orange Pi 5 (RK3588)

**Дата:** 2026-08-16 · **Решение:** D-012 ([sdd/decisions.md](sdd/decisions.md))
**Контекст:** production-траектория БВС; текущий стенд — стоковый образ
Orange Pi (Debian 12, vendor kernel `6.1.99-rockchip-rk3588`).

> **Про «Tokyo Linux»:** такого дистрибутива не существует (проверено
> поиском; ближайшее — AlmaLinux Day Tokyo, TLUG — мероприятия, не ОС).
> Вероятно имелся в виду **Yocto** — он разобран ниже наравне с остальными.

---

## 1. Жёсткое ограничение, которое отсекает большинство вариантов

Весь NPU-стек проекта — `rknn-bridge` + `librknnrt.so` 2.3.0 + zero-copy
`rknn_set_io_mem` — работает **только с проприетарным vendor-драйвером
`rknpu`** из Rockchip BSP-ядра 5.10/6.1.

Mainline-путь другой: в ядро 6.18 вошёл open-source драйвер
**`accel/rocket`** (Collabora, reverse-engineered). Но он **несовместим с
RKNN SDK по userspace ABI** — цитата автора драйвера (Tomeu Vizoso):
*«Is your open source kernel driver compatible with the proprietary rknn
and rkllm SDKs? No, it's not and it couldn't be»*. Open-source-альтернатива
— Mesa **Teflon** (TFLite delegate, Mesa 25.3) — требует перевода моделей
на TFLite и переписывания инференс-стека.

**Вывод: для нашего стека — ТОЛЬКО дистрибутивы с vendor 6.1 BSP-ядром.**
Mainline-only (Fedora aarch64, openSUSE, Debian mainline, Armbian edge) —
исключены из production-рассмотрения.

## 2. Матрица кандидатов

| Дистрибутив | Ядро | NPU (librknnrt) | Поддержка | Вердикт |
|---|---|---|---|---|
| Стоковый Orange Pi Debian 12 (текущий) | vendor 6.1.99 | ✅ | образы Orange Pi, апдейты эпизодические | ⚠️ работает, но не production |
| **Armbian (vendor branch)** | vendor 6.1 BSP | ✅ | **official Standard support OPi5 (2026)**, apt | ✅ **рекомендация** |
| Ubuntu Rockchip (Joshua-Riek) | vendor 6.1 | ✅ | **репозиторий архивирован 2026-04-29** | ❌ мёртв; fork defcom5 — только OPi5B |
| Yocto + meta-rockchip / JeffyCN | vendor 6.1-rkr5 | ✅ | RKNN-патчи в meta-rockchip ещё в review (v2) | 🔶 для серии, не сейчас |
| Armbian edge / Fedora / openSUSE | mainline 6.12+ / 6.18 rocket | ❌ | — | ❌ несовместимо с RKNN SDK |

## 3. Почему текущий образ не годится для production

1. **Нет kernel-headers в репозиториях** — `gspca_ov534` (PS Eye) пришлось
   собирать из полного дерева исходников ядра с github (~1.9 GB, git-clone;
   процедура — [CAMERA_PS_EYE_TEST.md](CAMERA_PS_EYE_TEST.md) §3). На любой
   смене ядра — повторение.
2. **Нет систематических security-обновлений** ни userland, ни ядра.
3. **GCC12** — блокирует `cpu-onnx` (prebuilt ONNX Runtime требует
   libstdc++ GCC13; D-008, KNOWN_ISSUES).

## 4. Рекомендация: Armbian, vendor 6.1, userland Ubuntu 24.04

### Обоснование

- **Official support** Orange Pi 5 на [armbian.com/boards/orangepi5](
  https://www.armbian.com/boards/orangepi5) — «Standard support, 2026»
  (не community/csc-уровень).
- **Vendor 6.1 BSP-ядро** — наш NPU-путь переносится 1:1: `librknnrt.so`,
  `rknn-bridge`, zero-copy API, `/dev/rknpu` — без изменений кода.
- **apt-обновления** ядра и userland; kernel-headers доступны через
  `linux-headers-vendor-rk3588` / `armbian-config` (fallback — сборка
  `armbian/build`, что всё равно проще стока).
- **Ubuntu 24.04 userland = GCC13** → `cpu-onnx` впервые соберётся **на
  устройстве** (закрывает давнее ограничение; альтернативный userland —
  Debian 13 trixie, тоже GCC13+, если хочется остаться ближе к Debian).
- Инфраструктура проекта (systemd-юниты, `.deb`-мышление, Docker SITL,
  Rust-тулчейн) переносится без изменений.

### Известные риски

- Качество vendor-ветки Armbian для RK3588 следует проверять на стенде:
  NPU-deqfreq, VPU, термальные зоны. Smoke-тест из
  [HARDWARE_TEST_RESULTS.md](HARDWARE_TEST_RESULTS.md) §6 — наш критерий
  приёмки после миграции.
- Модуль `gspca_ov534` нужно пересобрать под ядро Armbian (headers из apt
  делают это тривиальным;procedure §3 докамера-отчёта).

## 5. Yocto — «финальная форма», но не сейчас

Если название «Tokyo» означало **Yocto**: это build-фреймворк для
воспроизводимых embedded-образов, и для серийного БВС он идеален
(минимальный образ, lock-step версий, OTA, свои SDK). Но:

- meta-rockchip только **в process** принятия RKNN-поддержки (patch v1/v2
  в review); рабочий путь сегодня — community-слой JeffyCN/meta-rockchip с
  веткой `rk-6.1-rkr5`.
- Требуется build-сервер (сборка образа — часы) и пере-интеграция всего
  стека (cargo-деплой, systemd, Docker для SITL — по-другому).
- Для команды из 1–2 человек на этапе лётных испытаний это остановка
  разработки на недели.

**Отложено до Phase 7 (сериализация/production-hardening).** Когда
перейдём — связка `meta-rockchip` (vendor) + собственный layer
`meta-autotargeting` (rknn-bridge, наши юниты, модель) — прямой путь.

## 6. План миграции (когда решим)

1. Скачать образ Armbian (OPi5, vendor kernel, Ubuntu 24.04 server) →
   записать на spare-SD (стенд не трогаем до приёмки).
2. Smoke-тест: 294 unit + bridge + NPU init/infer (`scripts/run_hardware_tests.sh`).
3. Проверить NPU-deqfreq/thermal zones (наши system-telemetry зонды).
4. Установить headers, собрать `gspca_ov534` по §3, проверить PS Eye.
5. Развернуть деплой (systemd-юниты), прогнать `live_camera_demo` A/B со
   старым стендом.
6. Перенести загрузчик на eMMC, старую SD — в архив.

## Источники

- [Armbian — Orange Pi 5 board support](https://www.armbian.com/boards/orangepi5)
- [Armbian forum: NPU only works on 6.1 vendor kernel](https://forum.armbian.com/topic/56374-expected-default-graphics-acceleration-for-rk3588/)
- [Tomeu Vizoso: Rocket vs RKNN SDK UABI](https://blog.tomeuvizoso.net/2024/06/rockchip-npu-update-4-kernel-driver-for.html)
- [Phoronix: Rocket NPU driver в Linux 6.18](https://www.phoronix.com/news/Rockchip-NPU-Linux-Mesa)
- [CNX-Software: RK3588 mainline status](https://www.cnx-software.com/2024/12/21/rockchip-rk3588-mainline-linux-support-current-status-and-future-work-for-2025/)
- [Collabora mainline-status matrix](https://gitlab.collabora.com/hardware-enablement/rockchip-3588/notes-for-rockchip-3588/-/blob/main/mainline-status.md)
- [ubuntu-rockchip архивирован (2026-04-29)](https://github.com/Joshua-Riek/ubuntu-rockchip)
- [Yocto: RKNN patch для meta-rockchip (review)](https://lists.yoctoproject.org/g/yocto-patches/topic/patch_meta_rockchip_add/117505173)
- [JeffyCN/meta-rockchip](https://github.com/JeffyCN/meta-rockchip)
- [DietPi: Rocket vs vendor rknpu](https://dietpi.com/forum/t/orange-pi-5-plus-npu-driver-support-for-docker-projects-e-g-rkllama/23969)
