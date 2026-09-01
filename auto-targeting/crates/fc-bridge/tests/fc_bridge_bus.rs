//! Интеграционный тест M3 (все ОС, без железа): мост с MockFcAdapter +
//! реальная шина zenoh → телеметрия на at/telemetry, статус на
//! at/status/fc, команда по шине доходит до диспетчера (счётчик растёт).

use std::time::Duration;

use event_bus::{topics, BusConfig, CommandMsg, EventBus, TelemetrySample, CONTRACT_VERSION};
use fc_bridge::{BridgeConfig, FcBridge, FcBridgeStatus};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fc_bridge_publishes_telemetry_status_and_handles_commands() {
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17451".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();

    let bus2 = EventBus::connect("tcp/127.0.0.1:17451").await.unwrap();
    let mut adapter = fc_adapter::build_adapter(&common::FcConfig {
        adapter: "mock".into(),
        ..Default::default()
    });
    adapter.connect().await.unwrap();

    let tele_sub = hub.subscribe_telemetry().await.unwrap();
    let status_sub = hub
        .subscriber::<FcBridgeStatus>(&topics::status("fc"))
        .await
        .unwrap();
    let cmd_pub = bus2.publish_commands().await.unwrap();

    // Мост в отдельной задаче; команду публикуем ПОСЛЕ его старта
    // (zenoh best-effort: put без готового подписчика теряется).
    let bridge = FcBridge::new(BridgeConfig {
        telemetry_hz: 50,
        max_duration: Some(Duration::from_secs(2)),
        ..BridgeConfig::default()
    });
    let hub_clone = EventBus::connect("tcp/127.0.0.1:17451").await.unwrap();
    let bridge_task =
        tokio::spawn(async move { bridge.run(adapter.as_mut(), &hub_clone).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(600)).await;

    let cmd = CommandMsg {
        v: CONTRACT_VERSION,
        target: "fc".into(),
        cmd: "arm".into(),
        args: serde_json::json!({}),
        source: "test".into(),
        id: 1,
    };
    cmd_pub.publish(&cmd).await.unwrap();

    let stats = bridge_task.await.unwrap();
    assert!(stats.telemetry_published >= 5, "telemetry not flowing");

    // Телеметрия: контракт.
    let sample = tele_sub.recv_timeout(Duration::from_secs(5)).await.unwrap();
    assert!(sample.t_ms > 0);
    assert!(sample.roll_deg.is_finite());
    // Мок-адаптер по умолчанию Stabilize(2); проверяем валидный код карты.
    assert!(matches!(sample.mode, 0 | 2 | 10 | 11 | 12 | 15 | 17 | 255));

    // Статус: контракт.
    let st = status_sub
        .recv_timeout(Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(st.v, CONTRACT_VERSION);
    assert!(st.telemetry_hz_actual > 0.0);

    // Команды дошли (счётчик обработанных).
    assert!(
        stats.commands_handled >= 1,
        "command not dispatched (handled={}, errors={})",
        stats.commands_handled,
        stats.command_errors
    );

    let _ = bus2.close().await;
    let _ = hub.close().await;
}

/// Диспетчер: неизвестная команда → ошибка (счётчик ошибок), мост жив.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fc_bridge_rejects_unknown_command() {
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17452".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();
    let bus2 = EventBus::connect("tcp/127.0.0.1:17452").await.unwrap();
    let mut adapter = fc_adapter::build_adapter(&common::FcConfig {
        adapter: "mock".into(),
        ..Default::default()
    });
    adapter.connect().await.unwrap();
    let cmd_pub = bus2.publish_commands().await.unwrap();

    let bad = CommandMsg {
        v: CONTRACT_VERSION,
        target: "fc".into(),
        cmd: "self_destruct".into(),
        args: serde_json::json!({}),
        source: "test".into(),
        id: 2,
    };
    let good = CommandMsg {
        v: CONTRACT_VERSION,
        target: "not-fc".into(),
        cmd: "noop".into(),
        args: serde_json::json!({}),
        source: "test".into(),
        id: 3,
    };
    let bridge = FcBridge::new(BridgeConfig {
        telemetry_hz: 10,
        max_duration: Some(Duration::from_secs(2)),
        ..BridgeConfig::default()
    });
    let hub_clone = EventBus::connect("tcp/127.0.0.1:17452").await.unwrap();
    let bridge_task =
        tokio::spawn(async move { bridge.run(adapter.as_mut(), &hub_clone).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(600)).await;

    cmd_pub.publish(&bad).await.unwrap();
    cmd_pub.publish(&good).await.unwrap();

    let stats = bridge_task.await.unwrap();

    assert_eq!(stats.commands_handled, 0);
    assert!(stats.command_errors >= 2, "both commands must error");

    let _ = bus2.close().await;
    let _ = hub.close().await;
}

#[test]
fn telemetry_sample_is_copyable_payload() {
    // Контракт-совместимость типа (компиляция + serde).
    let t = TelemetrySample::minimal(1, 0.0, 0.0, 0.0, 0.0);
    let js = serde_json::to_string(&t).unwrap();
    assert!(js.contains("\"mode\":255"));
}
