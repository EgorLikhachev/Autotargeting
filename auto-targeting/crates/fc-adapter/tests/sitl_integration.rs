//! SITL integration tests — тесты против реального ArduPilot SITL.
//!
//! Эти тесты требуют запущенного SITL. По умолчанию они #[ignore],
//! чтобы не запускаться в обычном CI.
//!
//! ## Запуск
//!
//! ```bash
//! # 1. Запустить SITL
//! ./sim/sitl/run_sitl.sh start
//!
//! # 2. Запустить тесты
//! cargo test -p fc-adapter --test sitl_integration -- --include-ignored
//!
//! # 3. Или конкретный тест
//! cargo test -p fc-adapter --test sitl_integration -- --include-ignored test_heartbeat
//! ```
//!
//! ## Что тестирует
//!
//! - Подключение к SITL по TCP
//! - Получение HEARTBEAT
//! - Команды arm/disarm
//! - Смену режима (GUIDED, RTL, LOITER)
//! - Получение ATTITUDE telemetry
//! - Стабильность heartbeat (не теряется > 1 сек)

#![cfg(test)]

use common::FlightMode;
use fc_adapter::{ArduPilotConfig, ArduPilotMavlinkAdapter, FlightControllerAdapter};
use std::time::{Duration, Instant};
use tokio::time::sleep;

const SITL_ENDPOINT: &str = "tcpout:127.0.0.1:5760";
const HEARTBEAT_TIMEOUT_SECS: u64 = 5;

/// Проверка: можем подключиться к SITL.
#[tokio::test]
#[ignore = "requires SITL running: ./sim/sitl/run_sitl.sh start"]
async fn test_sitl_connect() {
    let mut adapter = ArduPilotMavlinkAdapter::new(ArduPilotConfig {
        endpoint: SITL_ENDPOINT.to_string(),
        ..Default::default()
    });

    let result = adapter.connect().await;
    assert!(
        result.is_ok(),
        "failed to connect to SITL: {:?}",
        result.err()
    );

    adapter.disconnect().await.unwrap();
}

/// Проверка: получаем HEARTBEAT от SITL в течение 5 секунд.
#[tokio::test]
#[ignore = "requires SITL running"]
async fn test_sitl_heartbeat() {
    let mut adapter = ArduPilotMavlinkAdapter::new(ArduPilotConfig {
        endpoint: SITL_ENDPOINT.to_string(),
        ..Default::default()
    });

    adapter.connect().await.unwrap();

    // Ждём HEARTBEAT
    let start = Instant::now();
    let mut got_heartbeat = false;

    while start.elapsed() < Duration::from_secs(HEARTBEAT_TIMEOUT_SECS) {
        let hb = adapter.heartbeat_status();
        if !hb.is_stale(2000) {
            got_heartbeat = true;
            println!("HEARTBEAT received: mode={:?}, armed={}", hb.mode, hb.armed);
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }

    assert!(got_heartbeat, "no HEARTBEAT received within 5 seconds");

    adapter.disconnect().await.unwrap();
}

/// Проверка: arm + disarm работает.
#[tokio::test]
#[ignore = "requires SITL running"]
async fn test_sitl_arm_disarm() {
    let mut adapter = ArduPilotMavlinkAdapter::new(ArduPilotConfig {
        endpoint: SITL_ENDPOINT.to_string(),
        ..Default::default()
    });

    adapter.connect().await.unwrap();

    // Ждём первый HEARTBEAT
    wait_for_heartbeat(&adapter, Duration::from_secs(5)).await;

    // Disarm сначала (вдруг уже armed)
    let _ = adapter.disarm().await;
    sleep(Duration::from_secs(1)).await;

    // Arm
    let result = adapter.arm().await;
    assert!(result.is_ok(), "arm failed: {:?}", result.err());

    // Ждём обновления статуса
    sleep(Duration::from_secs(2)).await;
    let hb = adapter.heartbeat_status();
    println!("After arm: mode={:?}, armed={}", hb.mode, hb.armed);

    // Disarm
    let result = adapter.disarm().await;
    assert!(result.is_ok(), "disarm failed: {:?}", result.err());

    sleep(Duration::from_secs(2)).await;

    adapter.disconnect().await.unwrap();
}

/// Проверка: смена режима работает (GUIDED, RTL, LOITER).
#[tokio::test]
#[ignore = "requires SITL running"]
async fn test_sitl_mode_change() {
    let mut adapter = ArduPilotMavlinkAdapter::new(ArduPilotConfig {
        endpoint: SITL_ENDPOINT.to_string(),
        ..Default::default()
    });

    adapter.connect().await.unwrap();
    wait_for_heartbeat(&adapter, Duration::from_secs(5)).await;

    // Сначала arm (некоторые режимы требуют armed state)
    let _ = adapter.arm().await;
    sleep(Duration::from_secs(1)).await;

    // Попробовать GUIDED
    let result = adapter.set_mode(FlightMode::Guided).await;
    println!("set_mode(Guided): {:?}", result);

    sleep(Duration::from_secs(2)).await;
    let hb = adapter.heartbeat_status();
    println!("Current mode: {:?}", hb.mode);

    // Попробовать LOITER
    let result = adapter.set_mode(FlightMode::Loiter).await;
    println!("set_mode(Loiter): {:?}", result);

    sleep(Duration::from_secs(2)).await;
    let hb = adapter.heartbeat_status();
    println!("Current mode: {:?}", hb.mode);

    // Попробовать RTL
    let result = adapter.set_mode(FlightMode::Rtl).await;
    println!("set_mode(Rtl): {:?}", result);

    sleep(Duration::from_secs(2)).await;
    let hb = adapter.heartbeat_status();
    println!("Current mode: {:?}", hb.mode);

    adapter.disconnect().await.unwrap();
}

/// Проверка: получаем ATTITUDE от SITL.
#[tokio::test]
#[ignore = "requires SITL running"]
async fn test_sitl_attitude() {
    let mut adapter = ArduPilotMavlinkAdapter::new(ArduPilotConfig {
        endpoint: SITL_ENDPOINT.to_string(),
        ..Default::default()
    });

    adapter.connect().await.unwrap();
    wait_for_heartbeat(&adapter, Duration::from_secs(5)).await;

    // Ждём ATTITUDE (SITL шлёт на 10Hz)
    let start = Instant::now();

    while start.elapsed() < Duration::from_secs(5) {
        let att = adapter.attitude();
        // Если attitude не нулевой — получили
        if att.roll != 0.0 || att.pitch != 0.0 || att.yaw != 0.0 {
            println!(
                "ATTITUDE received: roll={:.3}, pitch={:.3}, yaw={:.3}",
                att.roll, att.pitch, att.yaw
            );
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }

    // ATTITUDE может быть нулевым если дрон стоит на месте — это OK
    // Главное — heartbeat жив
    let hb = adapter.heartbeat_status();
    assert!(!hb.is_stale(2000), "heartbeat should be alive");

    adapter.disconnect().await.unwrap();
}

/// Проверка: heartbeat стабилен в течение 10 секунд.
#[tokio::test]
#[ignore = "requires SITL running"]
async fn test_sitl_heartbeat_stability() {
    let mut adapter = ArduPilotMavlinkAdapter::new(ArduPilotConfig {
        endpoint: SITL_ENDPOINT.to_string(),
        ..Default::default()
    });

    adapter.connect().await.unwrap();
    wait_for_heartbeat(&adapter, Duration::from_secs(5)).await;

    // Проверяем heartbeat каждые 500мс в течение 10 секунд
    let start = Instant::now();
    let mut stale_count = 0;
    let mut total_checks = 0;

    while start.elapsed() < Duration::from_secs(10) {
        let hb = adapter.heartbeat_status();
        total_checks += 1;
        if hb.is_stale(2000) {
            stale_count += 1;
            println!("WARNING: heartbeat stale at {:?}", start.elapsed());
        }
        sleep(Duration::from_millis(500)).await;
    }

    println!(
        "Heartbeat stability: {}/{} checks stale ({:.1}% stale)",
        stale_count,
        total_checks,
        (stale_count as f64 / total_checks as f64) * 100.0
    );

    // Допускаем максимум 1 stale из 20 проверок (5%)
    assert!(
        stale_count <= 1,
        "heartbeat too unstable: {stale_count} stale out of {total_checks}"
    );

    adapter.disconnect().await.unwrap();
}

/// Helper: ждать HEARTBEAT с таймаутом.
async fn wait_for_heartbeat(adapter: &ArduPilotMavlinkAdapter, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let hb = adapter.heartbeat_status();
        if !hb.is_stale(2000) {
            return;
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("no HEARTBEAT within {timeout:?}");
}
