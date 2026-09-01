//! Интеграционный тест M2 (все ОС): издатель fake-детекций на шине →
//! компонент tracker → подписчик at/tracks. Полный цикл M2 без железа.

use std::time::Duration;

use common::{BoundingBox, Detection};
use event_bus::{BusConfig, EventBus, TrackMsg};
use tracker_crate::{detections_to_frame, Tracker, TrackerConfig};

fn det(x: u32, y: u32, seq: u64, conf: f32) -> Detection {
    Detection {
        bbox: BoundingBox {
            x,
            y,
            width: 40,
            height: 80,
        },
        class: "person".into(),
        class_id: 0,
        confidence: conf,
        frame_seq: seq,
        detected_at: chrono::Utc::now(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracker_consumes_detections_and_publishes_tracks() {
    // 1. Шина: listener + подписчик треков.
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17451".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();
    let tracks_sub = hub.subscribe_tracks().await.unwrap();
    let status_sub = hub
        .subscriber::<tracker_crate::TrackerStatus>(&event_bus::topics::status("tracker"))
        .await
        .unwrap();
    let det_pub = hub.publish_detections().await.unwrap();

    // 2. Трекер.
    let tracker_bus = EventBus::connect("tcp/127.0.0.1:17451").await.unwrap();
    let tracker = Tracker::new(TrackerConfig {
        bus: String::new(),
        max_duration: Some(Duration::from_secs(3)),
        status_interval: Duration::from_secs(1),
        quiet_timeout: Some(Duration::from_secs(8)),
        ..TrackerConfig::default()
    })
    .unwrap();
    let handle = tokio::spawn(async move { tracker.run(&tracker_bus).await.unwrap() });

    // Ждём распространение declare-ов.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // 3. Издаём движущуюся детекцию (10 кадров, сдвиг 10px/кадр).
    for seq in 1..=10u64 {
        let frame = detections_to_frame(seq, vec![det(100 + (seq * 10) as u32, 200, seq, 0.9)]);
        det_pub.publish(&frame).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    // 4. Треки пришли; трек-ид стабильный (одна цель).
    let mut seen_ids = std::collections::HashSet::new();
    let mut msgs = 0;
    while let Ok(t) = tracks_sub.recv_timeout(Duration::from_millis(300)).await {
        seen_ids.insert(t.track_id);
        msgs += 1;
        if msgs >= 5 {
            break;
        }
    }
    assert!(msgs >= 1, "no tracks published");
    assert_eq!(
        seen_ids.len(),
        1,
        "one moving target must map to one track: {seen_ids:?}"
    );

    // Первый полный трек — контракт полей.
    let t: TrackMsg = tracks_sub
        .recv_timeout(Duration::from_millis(200))
        .await
        .unwrap();
    assert!(t.frame_seq >= 1);
    assert!(t.bbox.width > 0);
    assert_eq!(t.v, event_bus::CONTRACT_VERSION);

    // 5. Статус компонента.
    let st = status_sub
        .recv_timeout(Duration::from_secs(3))
        .await
        .unwrap();
    assert!(st.frames_in >= 1);
    assert!(st.tracks_published >= 1);

    let _ = handle.await;
    let _ = hub.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_targets_two_tracks() {
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17452".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();
    let tracks_sub = hub.subscribe_tracks().await.unwrap();
    let det_pub = hub.publish_detections().await.unwrap();

    let tracker_bus = EventBus::connect("tcp/127.0.0.1:17452").await.unwrap();
    let tracker = Tracker::new(TrackerConfig {
        max_duration: Some(Duration::from_secs(2)),
        quiet_timeout: Some(Duration::from_secs(6)),
        ..TrackerConfig::default()
    })
    .unwrap();
    let handle = tokio::spawn(async move { tracker.run(&tracker_bus).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Две далёкие цели — разные треки.
    for seq in 1..=8u64 {
        let frame = detections_to_frame(seq, vec![det(50, 50, seq, 0.8), det(500, 300, seq, 0.7)]);
        det_pub.publish(&frame).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut ids = std::collections::HashSet::new();
    let mut msgs = 0;
    while let Ok(t) = tracks_sub.recv_timeout(Duration::from_millis(300)).await {
        ids.insert(t.track_id);
        msgs += 1;
        if msgs >= 8 {
            break;
        }
    }
    assert!(msgs >= 2, "expected tracks, got {msgs}");
    assert_eq!(
        ids.len(),
        2,
        "two targets must produce exactly two tracks: {ids:?}"
    );

    let _ = handle.await;
    let _ = hub.close().await;
}
