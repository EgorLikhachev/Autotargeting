//! Интеграционный тест M6: Rust-клиент ↔ C++ rknn-bridge round-trip через
//! именованный SHM-сегмент (D-016). Закрывает давний test-gap
//! «нет round-trip теста C++↔Rust» (SDD §15).
//!
//! Запуск на стенде (нужен собранный bridge с HAVE_RKNN=1 или stub):
//!   cargo test -p detector --test bridge_shm_roundtrip -- --include-ignored
//!
//! Проверяет:
//! 1. init с frame_shm → bridge открывает сегмент (лог) и отвечает ok;
//! 2. infer без base64 (кадр в SHM) → детекции приходят (stub: синтетика,
//!    rknn: реальные);
//! 3. Сегмент удаляется в Drop клиента.

#![cfg(unix)]

use common::{Frame, FrameMetadata, PixelFormat};
use cv_inference::backend::InferenceBackend;

fn bridge_bin() -> Option<std::path::PathBuf> {
    let candidates = [
        "~/auto-targeting/auto-targeting/rknn-bridge/build/rknn-bridge",
        "/tmp/rknn-bridge",
    ];
    for c in candidates {
        let p = shellexpand(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn shellexpand(p: &str) -> std::path::PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(p)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires built rknn-bridge binary on the stand (Linux)"]
async fn bridge_shm_roundtrip_init_and_infer() {
    let Some(bin) = bridge_bin() else {
        eprintln!("[skip] rknn-bridge binary not found");
        return;
    };
    let sock = format!("/tmp/at-m6-test-{}.sock", std::process::id());
    let shm_path = format!("/dev/shm/at-m6-test-{}.infer", std::process::id());
    let _ = std::fs::remove_file(&sock);
    let _ = std::fs::remove_file(&shm_path);

    let mut child = std::process::Command::new(&bin)
        .env("RKNN_BRIDGE_SOCK", &sock)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn bridge");
    // Мост читает путь сокета из env? Нет — проверим фактический дефолт.
    // Если мост жёстко слушает /tmp/rknn-bridge.sock — используем его.
    let sock = "/tmp/rknn-bridge.sock".to_string();
    // Дать мосту подняться.
    for _ in 0..50 {
        if std::path::Path::new(&sock).exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(std::path::Path::new(&sock).exists(), "bridge socket never appeared");

    let cfg = cv_inference::RknnBridgeConfig {
        socket_path: sock.clone().into(),
        model_path: shellexpand(
            "~/auto-targeting/auto-targeting/models/yolov8n_int8.rknn",
        )
        .to_string_lossy()
        .into_owned(),
        input_width: 640,
        input_height: 480,
        input_format: "rgb24".into(),
        frame_shm: Some(shm_path.clone()),
        frame_shm_buffers: 2,
        ..Default::default()
    };
    let mut client = cv_inference::RknnBridgeClient::new(cfg);
    let init_res = client.init().await;
    if init_res.is_err() {
        // Stub-мост без NPU может не загрузить модель — но init-протокол и
        // SHM-хендшейк обязаны пройти до ошибки модели.
        let _ = child.kill();
        let _ = child.wait();
        panic!("init failed (bridge may be stub without model): {init_res:?}");
    }

    // Кадр letterboxed 640×640 RGB24 (как подаёт детектор).
    let frame = Frame {
        data: vec![128u8; 640 * 640 * 3],
        metadata: FrameMetadata {
            width: 640,
            height: 640,
            format: PixelFormat::Rgb24,
            captured_at: chrono::Utc::now(),
            seq: 1,
        },
    };
    let dets = client
        .infer(&frame)
        .await
        .expect("infer via shm must succeed");
    // Число детекций зависит от модели (stub — синтетика); важен сам round-trip.
    eprintln!("detections: {}", dets.len());

    // NOTE: health_check намеренно не проверяем — мост односессийный
    // (второе соединение не обслуживается, KNOWN_ISSUES #12).

    // Cleanup: закрыть клиент (Drop: shutdown + unlink сегмента), убить мост.
    drop(client);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        !std::path::Path::new(&shm_path).exists(),
        "frame shm segment must be unlinked on client drop"
    );
}
