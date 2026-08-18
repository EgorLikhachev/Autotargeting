# Заметки по разработке: видеорекордер — потребитель SHM-хранилища (TG26-125)

**Дата:** 2026-08-18 · **Крейт:** `crates/video-recorder` · **Задача:** TG26-125

## 1. Архитектура

Первый реальный потребитель кольца `shmem-buffer` (TG26-160, D-013):
подтверждает мультипотребительскую модель на практике.

```
SHM ring (NV12) ─▶ FrameConsumer ─▶ копия кадра ─▶ [guard DROP]
                                                └▶ NV12→RGB24 (integer)
                                                   └▶ OSD (draw_osd)
                                                      └▶ ffmpeg stdin-pipe
                                                         (rawvideo → libx264 → MP4)
```

* **Кодирование**: ffmpeg subprocess (`-f rawvideo -pix_fmt rgb24 -i pipe:0
  -c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p`) — без новых
  native-зависимостей, головной кодировщик уже обкатан на стенде.
* **OSD**: `cv_visualizer::draw_osd` — ISO-метка (мс), frame_id, WxH@формат;
  шрифт `--font` (DejaVuSansMono на устройстве).
* **Режимы**: `next` (последовательный; TooFarBehind → прыжок на latest)
  и `latest`.

## 2. Guard-дисциплина (ключевой инвариант)

Кадр **копируется** из слота (`FrameGuard::to_vec`) и guard **дропается до**
конвертации/OSD/записи в пайп. Guard живёт микросекунды. Если ffmpeg-пайп
забит (медленный x264 → backpressure) и `write_all` блокируется на секунды —
слот кольца НЕ заморожен: остальные потребители читают, продюсер в худшем
случае дропает новые кадры (drop-new, Вариант A). Это свойство проверено
интеграционным тестом (параллельный потребитель VERIFIED>0, TORN=0).

## 3. Результаты на стенде (RK3588, 2026-08-18)

Живой прогон: продюсер 640×480 NV12 @30 FPS (15 с) + рекордер (OSD, 12 с)
+ параллельный потребитель (next, 8 с):

| Компонент | Результат |
|---|---|
| Продюсер | published=446, dropped=0 |
| Рекордер | **RECORDED=353, OSD=353**, JUMPS=1 (catch-up на старте) |
| Параллельный потребитель | **VERIFIED=237, TORN=0** — не заблокирован |
| MP4 (ffprobe) | **h264, 640×480, 353 кадра, 11.77 с, 98.6 КБ** |

Тесты: 2 unit + doctest кросс-платформенно; smoke (ffmpeg→ffprobe h264) и
мультипроцессная интеграция — зелёные на RK3588 (`--include-ignored`).

## 4. Ограничения

1. **fps контейнера — номинальный** (`--fps`): CFR-таймкоды; реальный темп
   продюсера может отличаться (VFR по ts_ns — в реестре будущего).
2. **Геометрия фиксирована сегментом** (ring хранит один формат) — смена
   разрешения на лету не поддерживается by design.
3. **Крэш ffmpeg mid-write**: partial MP4 без moov (нечитаем) — фиксируется
   статистикой/ошибкой; ротация/по-кадровый MJPEG-fallback — будущее.
4. **OSD прожигается в пиксели** (burn-in) — «чистая» копия без наложений
   не сохраняется параллельно (можно вторым рекордером без OSD).
5. Аудио — вне скоупа.

## 5. Использование

```bash
# На стенде (продюсер уже пишет в autotarget.frames):
video-recorder --name autotarget.frames --out output/rec.mp4 \
    --fps 30 --seconds 30 --osd \
    --font /usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf

# Тесты:
cargo test -p video-recorder                                # unit+doc
cargo test -p video-recorder -- --include-ignored            # + ffmpeg/integration (Linux)
```
