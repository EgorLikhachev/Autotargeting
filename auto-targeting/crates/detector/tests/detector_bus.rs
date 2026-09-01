//! Интеграционный тест TG26-35 (все ОС, без NPU): in-process кольцо +
//! детектор на mock-бэкенде + шина zenoh → событие at/detections с
//! корректным контрактом (frame_seq = id кольца, bbox/class/conf, dims).

use std::time::Duration;

use detector::{BackendKind, Detector, DetectorConfig};
use event_bus::{BusConfig, EventBus};

fn synthetic_nv12(w: u32, h: u32, id: u64) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 3 / 2) as usize];
    for b in &mut v[..(w * h) as usize] {
        *b = (id % 251) as u8;
    }
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detector_publishes_detections_from_ring_to_bus() {
    const W: u32 = 64;
    const H: u32 = 48;
    const CAP: u32 = 8;

    // 1. Шина: слушатель (listener) — он же первый peer.
    let bus_hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17449".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();

    // 2. In-process кольцо + продюсер synthetic NV12.
    let ring_cfg = shmem_buffer::RingConfig {
        capacity: CAP,
        width: W,
        height: H,
        format: shmem_buffer::StorageFormat::Nv12,
    };
    let producer = shmem_buffer::create_in_process(&ring_cfg).unwrap();
    for id in 1..=4u64 {
        producer
            .publish(&synthetic_nv12(W, H, id), shmem_buffer::now_ns() + id)
            .unwrap();
    }

    // 3. Детектор: mock-бэкенд (без модели/NPU), подключается к шине.
    let bus_det = EventBus::connect("tcp/127.0.0.1:17449").await.unwrap();
    let cfg = DetectorConfig {
        segment: "test-ignored".into(), // сегмент не используется: consumer передаётся явно
        backend: BackendKind::Mock,
        quiet_timeout: Duration::from_secs(10),
        max_duration: Some(Duration::from_secs(2)),
        status_interval: Duration::from_secs(1),
        ..DetectorConfig::default()
    };
    let consumer = producer.consumer();
    let detector = Detector::new(cfg).unwrap();

    // Подписчик на события ДО старта детектора (объявления распространяются).
    let sub = bus_hub.subscribe_detections().await.unwrap();
    let status_sub = bus_hub
        .subscriber::<detector::DetectorStatus>(&event_bus::topics::status("detector"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let stats = detector.run(&consumer, &bus_det).await.unwrap();
    assert!(stats.frames_processed >= 1, "no frames processed");

    // 4. Контракт события.
    let event = sub.recv_timeout(Duration::from_secs(5)).await.unwrap();
    assert_eq!(event.frame_w, W);
    assert_eq!(event.frame_h, H);
    assert!(event.frame_seq >= 1, "frame_seq must be a ring frame id");
    assert!(event.captured_at.timestamp_millis() > 0);
    // bbox/class/conf присутствуют в типе; mock может дать пустой вектор —
    // тогда контракт проверяем сериализацией поля.
    let js = serde_json::to_value(&event).unwrap();
    assert!(js["detections"].is_array());
    for d in js["detections"].as_array().unwrap() {
        assert!(d["bbox"]["x"].is_u64());
        assert!(d["bbox"]["width"].is_u64());
        assert!(d["class"].is_string());
        assert!(d["confidence"].is_f64());
        assert!(d["frame_seq"].is_u64());
    }

    // 5. Статус-топик: метрики контура.
    let status = status_sub
        .recv_timeout(Duration::from_secs(5))
        .await
        .unwrap();
    assert!(status.frames_processed >= 1);
    assert_eq!(status.frame_w, W);
    assert_eq!(status.frame_h, H);
    assert!(status.fps >= 0.0);

    let _ = bus_det.close().await;
    let _ = bus_hub.close().await;
}
