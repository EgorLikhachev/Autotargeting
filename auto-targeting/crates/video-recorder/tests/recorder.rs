//! Тесты TG26-125: ffmpeg-writer smoke + мультипроцессная интеграция
//! (продюсер + рекордер + параллельный потребитель — проверка
//! неклокируемости).
//!
//! Запуск интеграционных (Linux, нужны examples/build):
//!   cargo test -p video-recorder -- --include-ignored

use std::process::Command;
use video_recorder::{FfmpegRawWriter, RecorderConfig};

/// Утилита: ffprobe → число кадров и кодек (None — ffprobe недоступен).
fn ffprobe_summary(path: &std::path::Path) -> Option<(String, i64)> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=codec_name,nb_frames",
            "-of", "csv=p=0",
            path.to_str()?,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let mut it = s.trim().split(',');
    let codec = it.next()?.to_string();
    let frames: i64 = it.next()?.parse().ok()?;
    Some((codec, frames))
}

/// Smoke: 30 синтетических RGB-кадров 64×48 → MP4 → ffprobe (h264, 30 кадров).
/// Корректность воспроизведения — критерий готовности TG26-125.
#[test]
#[ignore = "requires ffmpeg + ffprobe in PATH"]
fn ffmpeg_writer_produces_playable_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("smoke.mp4");
    let (w, h, n) = (64u32, 48u32, 30usize);
    let mut writer = FfmpegRawWriter::spawn(out.to_str().unwrap(), w, h, 30).unwrap();

    let mut frame = vec![0u8; w as usize * h as usize * 3];
    for i in 0..n {
        // Паттерн: бегущая полоса.
        for (j, px) in frame.chunks_exact_mut(3).enumerate() {
            let v = ((j + i * 64) % 256) as u8;
            px.copy_from_slice(&[v, 255 - v, (v / 2) + 64]);
        }
        writer.write_frame(&frame).unwrap();
    }
    writer.finish().unwrap();

    let (codec, frames) = ffprobe_summary(&out).expect("ffprobe failed");
    assert_eq!(codec, "h264");
    assert!(frames >= n as i64, "expected >= {n} frames, got {frames}");
    assert!(out.metadata().unwrap().len() > 1024, "file suspiciously small");
}

/// Интеграционный (Linux + SHM): продюсер в отдельном процессе публикует
/// кадры; рекордер пишет MP4; ПАРАЛЛЕЛЬНО второй потребитель читает тот же
/// стрим — метрика неклокируемости (VERIFIED>0, TORN=0).
#[test]
#[ignore = "linux multiprocess SHM: needs examples built + ffmpeg/ffprobe"]
fn recorder_records_without_blocking_other_consumers() {
    if !FfmpegRawWriter::ffmpeg_available() {
        eprintln!("[skip] no ffmpeg");
        return;
    }
    let name = format!("at-rec-{}.frames", std::process::id());
    let _ = shmem_buffer::remove_segment(&name);
    let dir = tempfile::tempdir().unwrap();
    let mp4 = dir.path().join("rec.mp4");

    // 1. Продюсер: 320×240 NV12 @ 50 FPS, 6 секунд.
    let mut producer = Command::new(exe("shmem_producer"))
        .args([
            "--name", &name, "--width", "320", "--height", "240",
            "--fps", "50", "--seconds", "6",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn producer");
    std::thread::sleep(std::time::Duration::from_millis(700)); // сегмент создан

    // 2. Рекордер: OSD (если есть системный шрифт), 5 секунд.
    let mut rec_cmd = Command::new(current_bin());
    rec_cmd.args([
        "--name", &name,
        "--out", mp4.to_str().unwrap(),
        "--fps", "50",
        "--seconds", "5",
        "--quiet-timeout", "8",
    ]);
    if let Some(font) = font_path() {
        rec_cmd.arg("--font").arg(&font);
    } else {
        eprintln!("[note] no system font: recording without OSD text");
    }
    let recorder = rec_cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn recorder");

    // 3. Параллельный потребитель — доказательство неклокируемости.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let consumer = Command::new(exe("shmem_consumer"))
        .args(["--name", &name, "--mode", "next", "--seconds", "4"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn consumer");

    let cons_out = consumer.wait_with_output().unwrap();
    let rec_out = recorder.wait_with_output().unwrap();
    let _ = producer.wait();

    // Рекордер отработал успешно.
    assert!(rec_out.status.success(), "recorder failed: {}", String::from_utf8_lossy(&rec_out.stdout));
    let rec_summary = String::from_utf8_lossy(&rec_out.stdout).to_string();
    eprintln!("recorder: {rec_summary}");

    // Параллельный потребитель: кадры читались, ни один не порван.
    let cons_summary = String::from_utf8_lossy(&cons_out.stdout).to_string();
    eprintln!("consumer: {cons_summary}");
    assert!(cons_summary.contains("TORN=0"), "torn reads while recording!");
    assert!(
        cons_summary.contains("VERIFIED=") && !cons_summary.contains("VERIFIED=0"),
        "parallel consumer starved while recorder was running"
    );

    // MP4 валиден и воспроизводим: h264, кадры есть.
    let (codec, frames) = ffprobe_summary(&mp4).expect("ffprobe failed");
    assert_eq!(codec, "h264");
    assert!(frames >= 30, "expected >= 30 frames, got {frames}");
    assert!(mp4.metadata().unwrap().len() > 4096);
    let _ = shmem_buffer::remove_segment(&name);
}

fn exe(name: &str) -> std::path::PathBuf {
    // target/<profile>/deps/../examples/<name> и target/<profile>/<bin>.
    let mut p = std::env::current_exe().expect("exe path");
    p.pop();
    if p.file_name().is_some_and(|s| s == "deps") {
        p.pop();
    }
    p.push("examples");
    p.push(name);
    p
}

fn current_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().expect("exe path");
    p.pop();
    if p.file_name().is_some_and(|s| s == "deps") {
        p.pop();
    }
    p.join(if cfg!(windows) { "video-recorder.exe" } else { "video-recorder" })
}

fn font_path() -> Option<String> {
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    CANDIDATES
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
}

#[test]
fn config_default_is_valid() {
    let cfg = RecorderConfig::default();
    assert_eq!(cfg.output, "output/rec.mp4");
}
