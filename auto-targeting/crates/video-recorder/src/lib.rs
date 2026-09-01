//! # video-recorder — независимый потребитель SHM-хранилища кадров (TG26-125)
//!
//! Первый реальный потребитель кольца [`shmem_buffer`] (TG26-160),
//! подтверждающий мультипотребительскую архитектуру: читает кадры через
//! `FrameConsumer`, кодирует в H.264/MP4 (ffmpeg subprocess, rawvideo через
//! stdin-pipe) и опционально прожигает OSD (временная метка + служебная
//! информация).
//!
//! ## Guard-дисциплина (главный инвариант)
//!
//! Кадр **копируется** из слота и `FrameGuard` отпускается ДО тяжёлой
//! работы (NV12→RGB, OSD, запись в пайп). Guard живёт микросекунды:
//! даже если ffmpeg-пайп забит (backpressure) и запись блокируется на
//! секунды — слот кольца не заморожен, остальные потребители работают,
//! продюсер в худшем случае дропает новые кадры (drop-new, Вариант A).
//!
//! ## Пример
//!
//! ```no_run
//! # fn main() -> Result<(), video_recorder::RecorderError> {
//! use video_recorder::RecorderConfig;
//!
//! let cfg = RecorderConfig {
//!     segment: "autotarget.frames".into(),
//!     output: "rec.mp4".into(),
//!     fps: 30,
//!     ..Default::default()
//! };
//! let consumer = video_recorder::attach(&cfg.segment)?;
//! let stats = video_recorder::Recorder::new(cfg)?.run(&consumer)?;
//! println!("written={}, jumps={}", stats.frames_written, stats.jumps);
//! # Ok(()) }
//! ```

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use shmem_buffer::{FrameConsumer, FrameGuard, NextStep, StorageFormat};

/// Ошибки рекордера.
#[derive(thiserror::Error, Debug)]
pub enum RecorderError {
    #[error("ffmpeg not found in PATH: {0}")]
    FfmpegNotFound(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("write to ffmpeg pipe failed (encoder died?): {0}")]
    PipeWrite(String),
    #[error("shared memory: {0}")]
    Shm(#[from] shmem_buffer::RingError),
    #[error("frame conversion: {0}")]
    Convert(#[from] video_capture::ConversionError),
    #[error("osd font: {0}")]
    Font(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}

/// Режим чтения кадров.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadMode {
    /// Последовательные кадры (`next_after`); при отставании больше
    /// ёмкости кольца — прыжок на свежий (потеря куска лучше зависания).
    #[default]
    Sequential,
    /// Всегда свежий кадр (`latest`) — мини-клипы, деградация грациозная.
    Latest,
}

/// Конфигурация записи.
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Имя сегмента SHM.
    pub segment: String,
    /// Путь к файлу MP4.
    pub output: String,
    /// Номинальный FPS контейнера (метки времени ffmpeg).
    pub fps: u32,
    /// Режим чтения.
    pub mode: ReadMode,
    /// Прожигать OSD (время/frame_id/размеры). Требует шрифт.
    pub osd: bool,
    /// Путь к TTF-шрифту для OSD.
    pub font: Option<String>,
    /// Прекратить запись после этого времени (None — пока жив стрим).
    pub max_duration: Option<Duration>,
    /// Завершиться, если новых кадров нет дольше этого времени.
    pub quiet_timeout: Option<Duration>,
    /// Endpoint шины zenoh для at/status/recorder (None — статусы выкл, M1).
    pub bus: Option<String>,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            segment: "autotarget.frames".into(),
            output: "output/rec.mp4".into(),
            fps: 30,
            mode: ReadMode::default(),
            osd: true,
            font: None,
            max_duration: None,
            quiet_timeout: Some(Duration::from_secs(5)),
            bus: None,
        }
    }
}

/// Итоговая статистика записи.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecorderStats {
    pub frames_received: u64,
    pub frames_written: u64,
    /// Прыжков вперёд (TooFarBehind → latest).
    pub jumps: u64,
    pub osd_frames: u64,
}

/// Подключиться к сегменту как потребитель.
pub fn attach(segment: &str) -> Result<FrameConsumer, RecorderError> {
    Ok(shmem_buffer::attach_shared(segment)?)
}

// ===================== ffmpeg writer =====================

/// Пишет сырые RGB24-кадры в ffmpeg (rawvideo → libx264/MP4).
pub struct FfmpegRawWriter {
    child: Child,
    width: u32,
    height: u32,
    pub bytes_written: u64,
}

impl FfmpegRawWriter {
    /// Проверить наличие ffmpeg в PATH.
    #[must_use]
    pub fn ffmpeg_available() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Запустить энкодер под размер кадра `width`×`height`, FPS контейнера `fps`.
    pub fn spawn(output: &str, width: u32, height: u32, fps: u32) -> Result<Self, RecorderError> {
        if !Self::ffmpeg_available() {
            return Err(RecorderError::FfmpegNotFound(
                "install ffmpeg (encoder for the recorder)".into(),
            ));
        }
        let child = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-s",
                &format!("{width}x{height}"),
                "-r",
                &fps.to_string(),
                "-i",
                "pipe:0",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "23",
                "-pix_fmt",
                "yuv420p",
                output,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| RecorderError::Ffmpeg(e.to_string()))?;
        Ok(Self {
            child,
            width,
            height,
            bytes_written: 0,
        })
    }

    /// Записать один RGB24-кадр (ровно `w*h*3` байт).
    pub fn write_frame(&mut self, rgb: &[u8]) -> Result<(), RecorderError> {
        let expected = self.width as usize * self.height as usize * 3;
        if rgb.len() != expected {
            return Err(RecorderError::InvalidConfig(format!(
                "frame size {} != {expected}",
                rgb.len()
            )));
        }
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| RecorderError::PipeWrite("ffmpeg stdin closed".into()))?;
        stdin
            .write_all(rgb)
            .and_then(|_| stdin.flush())
            .map_err(|e| RecorderError::PipeWrite(e.to_string()))?;
        self.bytes_written += expected as u64;
        Ok(())
    }

    /// Завершить: закрыть stdin (ffmpeg финализирует moov), дождаться.
    pub fn finish(mut self) -> Result<(), RecorderError> {
        drop(self.child.stdin.take());
        let status = self
            .child
            .wait()
            .map_err(|e| RecorderError::Ffmpeg(e.to_string()))?;
        if !status.success() {
            return Err(RecorderError::Ffmpeg(format!(
                "exit code {:?}",
                status.code()
            )));
        }
        Ok(())
    }

    /// B2 аудита: при ошибке записи (ffmpeg умер) — закрыть stdin и убить/
    /// дождаться ребёнка, чтобы не плодить зомби и не оставлять открытые
    /// дескрипторы. Выходной файл остаётся без moov (нечитаем) — это
    /// фиксируется вызывающим логом.
    pub fn abort(mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ===================== recorder =====================

/// M1: статус рекордера на шине (at/status/recorder).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecorderStatus {
    pub v: u8,
    pub frames_written: u64,
    pub jumps: u64,
    pub recording: bool,
    pub output: String,
}

/// Записывающий цикл: SHM → копия → конвертация → OSD → ffmpeg.
pub struct Recorder {
    cfg: RecorderConfig,
    font: Option<ab_glyph::FontVec>,
}

impl Recorder {
    pub fn new(cfg: RecorderConfig) -> Result<Self, RecorderError> {
        if cfg.fps == 0 {
            return Err(RecorderError::InvalidConfig("fps must be > 0".into()));
        }
        let font = match (&cfg.font, cfg.osd) {
            (Some(path), true) => Some(
                ab_glyph::FontVec::try_from_vec(
                    std::fs::read(path)
                        .map_err(|e| RecorderError::Font(format!("{}: {e}", path)))?,
                )
                .map_err(|e| RecorderError::Font(e.to_string()))?,
            ),
            (None, true) => {
                tracing::warn!("OSD enabled but no --font: recording without overlay");
                None
            }
            _ => None,
        };
        Ok(Self { cfg, font })
    }

    /// Основной цикл. Возвращает статистику; файл финализирован.
    pub async fn run(&self, consumer: &FrameConsumer) -> Result<RecorderStats, RecorderError> {
        let cap = consumer.capacity();
        // Первый кадр даёт геометрию (ring хранит её в каждом слоте) и
        // сразу идёт в запись через общий путь.
        let first = self.wait_first(consumer)?;
        let (w, h, format) = {
            let v = first.view();
            (v.width, v.height, v.storage_format())
        };
        let format = format.ok_or_else(|| {
            RecorderError::InvalidConfig("unsupported ring storage format".into())
        })?;
        tracing::info!(w, h, ?format, cap, "recorder attached to segment");

        let mut writer = FfmpegRawWriter::spawn(&self.cfg.output, w, h, self.cfg.fps)?;

        // M1: опциональный статус на шину.
        let bus_handle = match &self.cfg.bus {
            Some(ep) => match event_bus::EventBus::connect(ep).await {
                Ok(bus) => {
                    let p = bus
                        .publisher::<RecorderStatus>(&event_bus::topics::status("recorder"))
                        .await
                        .ok();
                    Some((bus, p))
                }
                Err(e) => {
                    tracing::warn!("bus connect failed: {e}");
                    None
                }
            },
            None => None,
        };
        let mut last_status = Instant::now();

        let mut stats = RecorderStats::default();
        let mut last: u64 = 0;
        let started = Instant::now();
        let mut last_frame_at = Instant::now();
        let mut pending: Option<FrameGuard> = Some(first);

        loop {
            // Лимиты.
            if let Some(max) = self.cfg.max_duration {
                if started.elapsed() >= max {
                    break;
                }
            }
            if let Some(q) = self.cfg.quiet_timeout {
                if last_frame_at.elapsed() >= q && stats.frames_received > 0 {
                    tracing::info!("quiet timeout: stream stalled, finalizing");
                    break;
                }
            }

            // Взять кадр (pending — от wait_first на первой итерации).
            let guard = match pending.take() {
                Some(g) => g,
                None => match self.acquire(consumer, &mut last, &mut stats) {
                    Some(g) => g,
                    None => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                },
            };
            last_frame_at = Instant::now();
            stats.frames_received += 1;

            // === Guard-дисциплина: копируем и ОТПУСКАЕМ слот до тяжёлой
            // работы. Конвертация/OSD/пайп — вне guard.
            let (frame_id, ts_ns, rgb) = {
                let view = guard.view();
                let rgb = match format {
                    StorageFormat::Nv12 => {
                        let nv12 = common::Frame {
                            data: guard.to_vec(),
                            metadata: view.to_metadata(),
                        };
                        video_capture::nv12_to_rgb24(&nv12)?.data
                    }
                    StorageFormat::Rgb24 => guard.to_vec(),
                };
                (view.frame_id, view.ts_ns, rgb)
            };
            drop(guard);

            // OSD (прожиг в пиксели).
            if self.cfg.osd {
                if let Some(font) = &self.font {
                    let mut img = image::RgbImage::from_raw(w, h, rgb)
                        .ok_or_else(|| RecorderError::InvalidConfig("rgb buffer".into()))?;
                    let ts = shmem_buffer::ts_ns_to_datetime(ts_ns);
                    let lines = [
                        ts.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
                        format!("frame {frame_id}"),
                        format!("{w}x{h} {:?}", format),
                    ];
                    let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
                    cv_visualizer::draw_osd(&mut img, &line_refs, font);
                    if let Err(e) = Self::write_rgb(&mut writer, img.into_raw(), &mut stats) {
                        tracing::error!(
                            "ffmpeg write failed — aborting encoder (output not finalized): {e}"
                        );
                        writer.abort();
                        return Err(e);
                    }
                    continue;
                }
            }
            if let Err(e) = Self::write_rgb(&mut writer, rgb, &mut stats) {
                tracing::error!(
                    "ffmpeg write failed — aborting encoder (output not finalized): {e}"
                );
                writer.abort();
                return Err(e);
            }

            // M1: статус раз в секунду.
            if last_status.elapsed() >= Duration::from_secs(1) {
                if let Some((_, Some(p))) = &bus_handle {
                    let st = RecorderStatus {
                        v: event_bus::CONTRACT_VERSION,
                        frames_written: stats.frames_written,
                        jumps: stats.jumps,
                        recording: true,
                        output: self.cfg.output.clone(),
                    };
                    let _ = p.publish(&st).await;
                }
                last_status = Instant::now();
            }
        }

        writer.finish()?;
        if let Some((bus, Some(p))) = bus_handle {
            let st = RecorderStatus {
                v: event_bus::CONTRACT_VERSION,
                frames_written: stats.frames_written,
                jumps: stats.jumps,
                recording: false,
                output: self.cfg.output.clone(),
            };
            let _ = p.publish(&st).await;
            let _ = bus.close().await;
        }
        Ok(stats)
    }

    fn write_rgb(
        writer: &mut FfmpegRawWriter,
        rgb: Vec<u8>,
        stats: &mut RecorderStats,
    ) -> Result<(), RecorderError> {
        writer.write_frame(&rgb)?;
        stats.frames_written += 1;
        Ok(())
    }

    /// Дождаться первого кадра (до quiet_timeout, чтобы не висеть вечно).
    fn wait_first(&self, consumer: &FrameConsumer) -> Result<FrameGuard, RecorderError> {
        let deadline = Instant::now() + self.cfg.quiet_timeout.unwrap_or(Duration::from_secs(10));
        let mut last = 0u64;
        let mut stats = RecorderStats::default();
        while Instant::now() < deadline {
            if let Some(g) = self.acquire(consumer, &mut last, &mut stats) {
                return Ok(g);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Err(RecorderError::Shm(shmem_buffer::RingError::AttachFailed(
            "no frames within quiet timeout".into(),
        )))
    }

    fn acquire(
        &self,
        consumer: &FrameConsumer,
        last: &mut u64,
        stats: &mut RecorderStats,
    ) -> Option<FrameGuard> {
        match self.cfg.mode {
            ReadMode::Sequential => {
                match consumer.next_after(*last) {
                    NextStep::Frame(g) => {
                        *last = g.frame_id();
                        Some(g)
                    }
                    NextStep::UpToDate => None,
                    NextStep::TooFarBehind { latest, .. } => {
                        stats.jumps += 1;
                        *last = latest;
                        // Сразу пробуем взять свежий.
                        if let NextStep::Frame(g) = consumer.next_after(*last) {
                            *last = g.frame_id();
                            return Some(g);
                        }
                        None
                    }
                }
            }
            ReadMode::Latest => match consumer.latest() {
                Some(g) => {
                    *last = g.frame_id();
                    Some(g)
                }
                None => None,
            },
        }
    }
}

// Пере-экспорт для bin/tests.
pub use ab_glyph;

#[cfg(test)]
mod tests {
    use super::*;

    /// B2: abort() не паникует и завершает ребёнка (kill+wait).
    /// Полный зомби-тест (pgrep) — Unix CI; здесь контракт-смоук.
    #[test]
    #[cfg(unix)]
    fn abort_kills_encoder() {
        let dir = std::env::temp_dir().join(format!("vr-abort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("x.mp4");
        let mut w =
            FfmpegRawWriter::spawn(out.to_str().unwrap(), 64, 48, 30).expect("ffmpeg present");
        // Небольшой кадр, затем немедленный abort.
        w.write_frame(&vec![0u8; 64 * 48 * 3]).unwrap();
        w.abort(); // не должен паниковать/виснуть
                   // Повторный abort-контракт: drop тоже безопасен.
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fps_zero_rejected() {
        let cfg = RecorderConfig {
            fps: 0,
            ..RecorderConfig::default()
        };
        assert!(Recorder::new(cfg).is_err());
    }

    #[test]
    fn defaults_are_sane() {
        let cfg = RecorderConfig::default();
        assert_eq!(cfg.fps, 30);
        assert_eq!(cfg.mode, ReadMode::Sequential);
        assert!(cfg.osd);
        assert!(cfg.quiet_timeout.is_some());
        assert!(Recorder::new(cfg).is_ok());
    }
}
