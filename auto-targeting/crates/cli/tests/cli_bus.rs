//! Интеграционные тесты M5 (все ОС): операторская консоль на шине.
//!
//! 1. REPL-команда доходит: `send_fc_command("arm")` → fc-bridge (mock FC)
//!    диспетчеризует (счётчик команд растёт).
//! 2. Конфиг-сервис: queryable at/config отвечает текущим AppConfig.

use std::time::Duration;

use auto_targeting_cli::bus_console;
use common::AppConfig;
use event_bus::{BusConfig, EventBus};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repl_command_reaches_fc_bridge() {
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17461".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();

    // fc-bridge с mock FC (диспетчер команд).
    let mut adapter = fc_adapter::build_adapter(&common::FcConfig {
        adapter: "mock".into(),
        ..Default::default()
    });
    adapter.connect().await.unwrap();
    let bridge_bus = EventBus::connect("tcp/127.0.0.1:17461").await.unwrap();
    let fb = fc_bridge::FcBridge::new(fc_bridge::BridgeConfig {
        telemetry_hz: 5,
        max_duration: Some(Duration::from_secs(6)),
        ..Default::default()
    });
    let fc_task = tokio::spawn(async move { fb.run(adapter.as_mut(), &bridge_bus).await });

    // Оператор (как repl-bus): подключение и команда arm.
    let op = EventBus::connect("tcp/127.0.0.1:17461").await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await; // declare-распространение
    bus_console::send_fc_command(&op, "arm", serde_json::json!({}))
        .await
        .unwrap();
    bus_console::send_fc_command(&op, "set_mode", serde_json::json!({"mode": "guided"}))
        .await
        .unwrap();

    let stats = fc_task.await.unwrap().unwrap();
    assert!(
        stats.commands_handled >= 2,
        "repl commands must reach fc-bridge: handled={} errors={}",
        stats.commands_handled,
        stats.command_errors
    );
    let _ = op.close().await;
    let _ = hub.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_service_answers_query() {
    let hub = EventBus::listen(BusConfig {
        endpoint: "tcp/127.0.0.1:17462".into(),
        ..BusConfig::default()
    })
    .await
    .unwrap();

    let mut cfg = AppConfig::default();
    cfg.bus.endpoint = "tcp/127.0.0.1:17462".into();
    cfg.video.fps = 42; // маркер для проверки roundtrip

    // Сервис (в фоновой задаче).
    let svc_bus = EventBus::connect("tcp/127.0.0.1:17462").await.unwrap();
    let svc_cfg = cfg.clone();
    let svc = tokio::spawn(async move { bus_console::run_config_service(&svc_bus, svc_cfg).await });
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Клиент: запрос через pub/sub-ack.
    let client = EventBus::connect("tcp/127.0.0.1:17462").await.unwrap();
    // Прямой roundtrip для ассертов (config_get печатает, здесь — данные):
    let ack = client
        .subscriber::<serde_json::Value>(event_bus::topics::CONFIG_ACK)
        .await
        .unwrap();
    let req = client
        .publisher::<event_bus::CommandMsg>(bus_console::CONFIG_GET_TOPIC)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    req.publish(&event_bus::CommandMsg {
        v: event_bus::CONTRACT_VERSION,
        target: "config-svc".into(),
        cmd: "get".into(),
        args: serde_json::json!({}),
        source: "test".into(),
        id: 1,
    })
    .await
    .unwrap();
    let v = ack.recv_timeout(Duration::from_secs(5)).await.unwrap();
    let got: AppConfig = serde_json::from_value(v).unwrap();
    assert_eq!(got.video.fps, 42);
    assert_eq!(got.bus.endpoint, "tcp/127.0.0.1:17462");

    svc.abort();
    let _ = client.close().await;
    let _ = hub.close().await;
}
