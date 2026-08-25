//! Интеграционный тест M4 (все ОС): замкнутая петля на шине.
//! fc-bridge (mock FC) публикует телеметрию; тест публикует треки
//! (как трекер); commander (bus-режим) выбирает цель и шлёт коррекции
//! через MockFcAdapter; статус виден на at/status/commander.

use std::time::Duration;

use commander::bus_runner::{CommanderBus, CommanderBusConfig, CommanderStatus};
use commander::{Commander, CommanderConfig};
use event_bus::{topics, BusConfig, EventBus, TelemetrySample, TrackMsg, CONTRACT_VERSION};

fn track_msg(id: u64, cx: f32, cy: f32) -> TrackMsg {
    TrackMsg {
        v: CONTRACT_VERSION,
        track_id: id,
        frame_seq: id,
        bbox: common::BoundingBox {
            x: (cx - 25.0) as u32,
            y: (cy - 30.0) as u32,
            width: 50,
            height: 60,
        },
        vx: 0.0,
        vy: 0.0,
        class: "person".into(),
        class_id: 0,
        confidence: 0.9,
        age: 1,
        misses: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commander_closes_loop_from_tracks_and_telemetry() {
    // Шина: listener-хаб.
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17453".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();
    let pub_side = EventBus::connect("tcp/127.0.0.1:17453").await.unwrap();

    // fc-bridge (mock FC): телеметрия 20 Гц, 4 с.
    let mut fc_adapter = fc_adapter::build_adapter(&common::FcConfig {
        adapter: "mock".into(),
        ..Default::default()
    });
    fc_adapter.connect().await.unwrap();
    let fc_bridge = fc_bridge::FcBridge::new(fc_bridge::BridgeConfig {
        telemetry_hz: 20,
        max_duration: Some(Duration::from_secs(4)),
        ..Default::default()
    });
    let bridge_bus = EventBus::connect("tcp/127.0.0.1:17453").await.unwrap();
    let fc_task =
        tokio::spawn(async move { fc_bridge.run(fc_adapter.as_mut(), &bridge_bus).await });

    // Commander (bus-режим): mock FC, центр кадра 320×240, 4 с.
    let cmd_fc = fc_adapter::build_adapter(&common::FcConfig {
        adapter: "mock".into(),
        ..Default::default()
    });
    let mut commander = Commander::new(CommanderConfig::default(), cmd_fc);
    let bus_cfg = CommanderBusConfig {
        frame_center: (320.0, 240.0),
        max_duration: Some(Duration::from_secs(4)),
        ..CommanderBusConfig::default()
    };
    let cmd_bus = EventBus::connect("tcp/127.0.0.1:17453").await.unwrap();
    let cmd_task = tokio::spawn(async move {
        CommanderBus::new(bus_cfg)
            .run(&mut commander, &cmd_bus)
            .await
    });

    // Подписчики (до старта публикации).
    let status_sub = hub
        .subscriber::<CommanderStatus>(&topics::status("commander"))
        .await
        .unwrap();
    let tele_sub = hub.subscribe_telemetry().await.unwrap();
    let tracks_pub = pub_side.publish_tracks().await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Трекер-роль: цель справа-снизу от центра — offset (60, 40).
    for seq in 1..=30u64 {
        tracks_pub
            .publish(&track_msg(seq, 320.0 + 60.0, 240.0 + 40.0))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let cmd_stats = cmd_task.await.unwrap().unwrap();
    let fc_stats = fc_task.await.unwrap().unwrap();

    // Петля замкнулась: треки приняты, телеметрия текла.
    assert!(cmd_stats.tracks_received >= 20, "tracks: {}", cmd_stats.tracks_received);
    assert!(cmd_stats.telemetry_received >= 20, "tele: {}", cmd_stats.telemetry_received);
    assert!(fc_stats.telemetry_published >= 20);
    // Коррекции: offset (60,40) вне deadband → rate-limiter послал.
    assert!(
        cmd_stats.corrections_sent + cmd_stats.corrections_suppressed >= 1,
        "no correction attempts: sent={} supp={}",
        cmd_stats.corrections_sent,
        cmd_stats.corrections_suppressed
    );

    // Статус на шине: ждём кадр с выбранной целью (первые статусы могут
    // быть Scanning — цель выбирается с первым треком).
    let mut got_target = false;
    for _ in 0..10 {
        let st = status_sub.recv_timeout(Duration::from_secs(3)).await.unwrap();
        assert_eq!(st.v, CONTRACT_VERSION);
        assert!(
            matches!(st.state.as_str(), "Tracking" | "TrackingDegraded" | "Scanning" | "TargetSelected"),
            "state: {}",
            st.state
        );
        if st.active_target.is_some() {
            got_target = true;
            break;
        }
    }
    assert!(got_target, "commander never selected a target");

    // Телеметрия (от fc-bridge) приходила.
    let t = tele_sub.recv_timeout(Duration::from_secs(5)).await.unwrap();
    assert!(t.t_ms > 0);

    let _ = pub_side.close().await;
    let _ = hub.close().await;
}

/// Deadband: цель в центре — коррекции подавлены, не посланы.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commander_suppresses_centered_target() {
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17454".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();
    let pub_side = EventBus::connect("tcp/127.0.0.1:17454").await.unwrap();

    let mut fc = fc_adapter::build_adapter(&common::FcConfig {
        adapter: "mock".into(),
        ..Default::default()
    });
    fc.connect().await.unwrap();
    let fb = fc_bridge::FcBridge::new(fc_bridge::BridgeConfig {
        telemetry_hz: 20,
        max_duration: Some(Duration::from_secs(3)),
        ..Default::default()
    });
    let fb_bus = EventBus::connect("tcp/127.0.0.1:17454").await.unwrap();
    let fc_task = tokio::spawn(async move { fb.run(fc.as_mut(), &fb_bus).await });

    let cmd_fc = fc_adapter::build_adapter(&common::FcConfig {
        adapter: "mock".into(),
        ..Default::default()
    });
    let mut commander = Commander::new(CommanderConfig::default(), cmd_fc);
    let cfg = CommanderBusConfig {
        max_duration: Some(Duration::from_secs(3)),
        ..CommanderBusConfig::default()
    };
    let cmd_bus = EventBus::connect("tcp/127.0.0.1:17454").await.unwrap();
    let cmd_task = tokio::spawn(async move { CommanderBus::new(cfg).run(&mut commander, &cmd_bus).await });

    let tracks_pub = pub_side.publish_tracks().await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    // Цель ровно в центре (320, 240) — внутри deadband.
    for seq in 1..=20u64 {
        tracks_pub.publish(&track_msg(seq, 320.0, 240.0)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let stats = cmd_task.await.unwrap().unwrap();
    let _ = fc_task.await.unwrap();
    // Практически всё подавлено (deadband), послано ~0.
    assert!(
        stats.corrections_sent == 0,
        "centered target must not send: {}",
        stats.corrections_sent
    );

    let _ = pub_side.close().await;
    let _ = hub.close().await;
}

/// Телеметрия-тип из M0 остаётся совместимым (смоук сериализации).
#[test]
fn telemetry_smoke() {
    let t = TelemetrySample::minimal(1, 1.0, 2.0, 3.0, 4.0);
    let js = serde_json::to_string(&t).unwrap();
    assert!(js.contains("\"roll_deg\":1.0"));
}
