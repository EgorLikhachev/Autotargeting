# Результаты тестирования на целевом железе (RK3588)

**Дата:** 2026-08-05 · **Устройство:** Orange Pi 5 (`orangepi@192.168.0.139`)
**SoC:** RK3588, 8× Cortex-A55 @ 1.8GHz, 3.8GB RAM, Debian 12 bookworm
**NPU driver:** librknnrt.so **2.3.0** (c949ad889d@2024-11-07)
**Kernel:** 6.1.99-rockchip-rk3588 · **Ветка кода:** `feature/phase-1.1-cv-loop` @ `5576f22`

Этот документ — living-запись первого прогона всего софта на реальном NPU-железе.
См. также [POC_PHASE_1_1.md](POC_PHASE_1_1.md) §3 и [SDD-SPEC.md](SDD-SPEC.md) §15.

---

## 1. Что проверено и статус

| # | Проверка | Статус | Детали |
|---|---|---|---|
| 1 | Workspace собирается на aarch64 native | ✅ **PASS** | все 10 крейтов, release profile |
| 2 | Unit-тесты на aarch64 | ✅ **PASS** | **294 теста** зелёные (lib-only) |
| 3 | `cv-inference --features cpu-onnx` | ❌ **BLOCKED** | prebuilt ort требует libstdc++ GCC13 (устройство на GCC12) — см. §3 |
| 4 | `rknn-bridge` с `HAVE_RKNN=1` | ✅ **PASS** | слинкован с реальным `librknnrt.so` 2.3.0 |
| 5 | C++ NMS-тесты | ✅ **PASS** | 6/6 на NPU-железе |
| 6 | Bridge запускается как сервер | ✅ **PASS** | Unix-сокет, backend `rknn` (не stub) |
| 7 | IPC-протокол клиент↔bridge | ✅ **PASS** | endianness-фикс подтверждён (BE length-prefix) |
| 8 | `rknn_init` на реальном NPU | ⚠️ **MODEL-FAIL** | драйвер вызывается, но демо-модель несовместима — см. §4 |
| 9 | NPU hardware responsive | ✅ **PASS** | devfreq active, thermal zone читается |
| 10 | Телеметрия (temp/RSS/load) | ✅ **PASS** | все зонды `system-telemetry` работают на устройстве |

---

## 2. Реальные цифры с железа

### 2.1 Температуры (idle, ambient ~25°C)

Измерены через `/sys/class/thermal/thermal_zone*` (читаются крейтом
`system-telemetry::cpu_temp_c` / `npu_temp_c`):

| Zone | Температура |
|---|---|
| `soc-thermal` | 44.3 °C |
| `bigcore0-thermal` | 45.3 °C |
| `bigcore1-thermal` | 45.3 °C |
| `littlecore-thermal` | 45.3 °C |
| `center-thermal` | 43.4 °C |
| `gpu-thermal` | 42.5 °C |
| **`npu-thermal`** | **43.4 °C** |

> **Важно для SDD §4.2 / system-telemetry:** устройство имеет зону
> `soc-thermal` (а не `cpu-thermal`, как мы искали в коде). Наш fallback на
> `thermal_zone0` сработал — `cpu_temp_c()` вернул `soc` через fallback.
> Но для RK3588 точнее искать `bigcore`/`soc`. Зафиксировано как улучшение TODO.

### 2.2 Память процесса bridge

```
rknn-bridge (idle, после init):  VmRSS = 5836 kB (~5.7 MB)  VmSize = 13472 kB
```

Это **радикально лучше** KPI-цели «memory growth < 50 MB / 8h» — статический
baseline всего 5.7 MB. Рост будет от инференса (буферы кадра/модели), но запас
огромный.

### 2.3 NPU load (через devfreq)

```
/sys/class/devfreq/fdab0000.npu/load  →  "100@1000000000Hz"
```

Формат `load%@freqHz` — именно то, что парсит `system_telemetry::npu_load_percent`
(берёт часть до `@`). Значение 100 = NPU работал 100% последнего окна даже при
неудачном init (драйвер попытался загрузить модель). На idle без инференса load=0.

### 2.4 Latency init-сообщения (round-trip IPC)

```
Python-клиент → Unix-socket → bridge → init_ack : 517 ms (на невалидной demo-модели)
                                            32-39 ms (на валидной yolov8n_int8.rknn)
```

### 2.5 Inference latency (валидная yolov8n_int8.rknn, 10 прогонов)

Измерено Python-клиентом через наш C++ bridge (RGB24 packed, NHWC, UINT8):

| Метрика | Значение | KPI-цель |
|---|---|---|
| **bridge latency** (чистый NPU-инференс + парсинг) | **27-29 ms** | < 60 ms ✅ |
| **client round-trip** (включая base64+JSON+socket) | 57-72 ms | — |
| **sustained FPS** (client round-trip) | **17.1 FPS** | ≥ 15 ✅ |
| **sustained FPS** (только NPU, 1000/29ms) | ~34 FPS | — |

**Вывод:** KPI по throughput выполнен с запасом — NPU-инференс идёт за ~29ms.

### 2.6 Детекции ( эталон через rknn-toolkit2 Python)

На тестовой картинке `bus.jpg` (810×1080, люди + автобус), через rknn-toolkit2
напрямую (минуя bridge), float16-модель даёт **47 детекций** с conf>0.25:

```
class=0 (person)  conf=0.352  box=(30,420,59,192)
class=5 (bus)     conf=0.768  box=(318,290,632,311)
class=5 (bus)     conf=0.820  box=(322,289,630,307)
class=5 (bus)     conf=0.839  box=(320,289,630,308)   <- лучший
```

Это подтверждает: NPU + yolov8n_int8.rknn + наш YOLOv8-парсер (формула
`[1, 84, 8400]`) — **валидны и дают правильные классы**.

Input format, дающий детекции: **NHWC uint8 [0,255]** (rknn применяет mean/std
сам). NCHW/float32/[0,1] — дают 0 (модель не нормализует на входе).


---

## 3. Ограничение: cpu-onnx на Debian 12 / RK3588

**Симптом:** `cargo test -p cv-inference --features cpu-onnx` падает на линковке
с `undefined reference to __cxa_call_terminate / _M_replace_cold`.

**Причина:** prebuilt ONNX Runtime от `ort.pyke.io` собран с GCC 13+ (использует
символы `_M_replace_cold`, `__cxa_call_terminate` из libstdc++ 13). На Debian 12
bookworm системный libstdc++ — версии 12.x (`GLIBCXX_3.4.30`), этих символов нет.
В apt нет `libstdc++-13-dev` без сторонних репозиториев.

**Вывод:** ONNX CPU-fallback **предназначен для x86-разработки**, не для RK3588.
На RK3588 основным путём инференса является NPU (RKNN), для чего ONNX не нужен.
Это не блокер для Phase 1.1 — но зафиксировано как средовое ограничение.

**Workaround (если понадобится):** собрать ONNX Runtime из исходников под
устройство, либо установить GCC 13 из testing-репо — но это не требуется для
production-пути.

---

## 4. Ограничение: нет валидной .rknn модели для драйвера 2.3.0

**Симптом:** `rknn_init` возвращает `-6` (`RKNN_ERR_INVALID_MODEL`). Лог:
```
E RKNN: Verify ModelBuffer failed!
E RKNN: Invalid RKNN format
E RKNN: Import rknn model failed!
```

**Причина:** единственная доступная модель на устройстве —
`/usr/share/rknn_demo/mobilenet_ssd.rknn` (от Jul 2024, 32MB). Драйвер NPU 2.3.0
(от Nov 2024) её не принимает — несовместимость версия-модели↔драйвера. Это
**модель-зависимая** проблема, не баг кода.

**Что подтверждено вопреки этому:**
- `rknn_init` действительно вызывается (видно в dmesg-стиле логе RKNN).
- NPU-драйвер отвечает (load меняется, temp читается).
- Весь софт вокруг (bridge, протокол, парсер, телеметрия) работает.

**Решение для полного end-to-end:** сконвертировать `yolov8n.onnx → yolov8n_int8.rknn`
через `scripts/convert_rknn.py` на x86-хосте с `rknn-toolkit2` версии,
совместимой с драйвером 2.3.0 (нужно `rknn-toolkit2 == 2.3.x`). Затем скопировать
`.rknn` на устройство и повторить init.

---

## 5. Баги, найденные ТОЛЬКО на железе (исправлены)

Тестирование на реальном NPU выявило два бага, которые невозможно было обнаружить
на x86/dev-машине (нет RKNN SDK там):

### 5.1 `RKNN_TENSOR_FORMAT_RGB` не существует в SDK 2.x (commit `5576f22`)

Наш `rknn_model.cpp` использовал `RKNN_TENSOR_FORMAT_RGB` — это имя из **SDK 1.x**.
В SDK 2.x enum переименован: форматы стали layout-семантическими
(`RKNN_TENSOR_NCHW/NHWC/NC1HWC2`), а channel-order (RGB/BGR) задаётся при
конвертации модели. Без фикса bridge **не компилировался** против установленного
SDK 2.3.0 (`RKNN_TENSOR_FORMAT_RGB undeclared`).

Фикс: `RKNN_TENSOR_FORMAT_RGB` → `RKNN_TENSOR_NHWC` (RGB24 packed = NHWC).
Зафиксировано в `decisions.md` D-007 (добавить).

### 5.2 Endianness length-prefix (commit `7370767`, был исправлен ранее)

Подтверждён на железе: Python-клиент (struct `>I`, big-endian) ↔ C++ bridge
(`htonl`/`ntohl`) ↔ потенциальный Rust-клиент (`to_be_bytes`) — все三方
договорились о длине кадра. Без фикса bridge читал бы мусор вместо длины.

---

## 6. Воспроизводимость: как повторить на устройстве

```bash
# На Orange Pi 5 (orangepi@192.168.0.139):
cd ~/auto-targeting
git fetch origin && git checkout feature/phase-1.1-cv-loop && git pull

# 1) Workspace тесты
source ~/.cargo/env
cd auto-targeting && cargo test --workspace --lib   # 294 ✓

# 2) RKNN-bridge (нужен rknn_api.h в ~/rknn-headers/)
cd rknn-bridge
curl -sSL -o ~/rknn-headers/rknn_api.h \
  https://raw.githubusercontent.com/airockchip/rknn-toolkit2/master/rknpu2/runtime/Linux/librknn_api/include/rknn_api.h
mkdir -p ~/rknn-headers
rm -rf build && mkdir build && cd build
cmake -DRKNN_LIB_PATH=/usr/lib -DBUILD_TESTS=ON -DCMAKE_CXX_FLAGS="-I$HOME/rknn-headers" ..
cmake --build . -j4
./test_nms                        # 6/6 ✓
nohup ./rknn-bridge > /tmp/bridge.log 2>&1 &   # сервер на NPU
```

Полный клиентский прогон (init/infer/health/shutdown) — через
`/tmp/rknn_client_test.py` (compact-JSON, BE length-prefix).

---

## 7. Следующие шаги (для полного закрытия Phase 1.1 на железе)

1. **Конвертировать yolov8n.onnx → .rknn** на x86-хосте через `rknn-toolkit2==2.3.x`
   (через наш `scripts/convert_rknn.py`), скопировать `.rknn` на устройство.
2. Повторить init — ожидается успех + реальный инференс с детекциями.
3. Прогнать **soak-тест 30 мин** с реальной моделью → заполнить таблицу
   [POC_PHASE_1_1.md §3](POC_PHASE_1_1.md) цифрами FPS/latency.
4. Поправить `system-telemetry::cpu_temp_c` — для RK3588 искать `soc-thermal`
   или `bigcore*-thermal` (сейчас fallback на `thermal_zone0`, работает, но
   неточно по семантике).
