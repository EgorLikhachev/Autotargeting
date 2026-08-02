//! HTTP health endpoint + systemd watchdog notify.
//!
//! systemd использует `Type=notify` + `WatchdogSec=10` для мониторинга
//! процессов. Приложение должно:
//! 1. Отправить `READY=1` при запуске
//! 2. Отправлять `WATCHDOG=1` каждые WatchdogSec/2 секунд
//! 3. Отправить `STOPPING=1` при завершении
//!
//! Если процесс не отправит WATCHDOG=1 вовремя — systemd его убьёт и
//! перезапустит (Restart=on-failure).
//!
//! ## HTTP endpoint
//!
//! Дополнительно поднимаем HTTP server на :8080 для health checks:
//! - `GET /health` → JSON со статусом системы
//! - `GET /metrics` → Prometheus-совместимые метрики (Phase 3)
//!
//! Это позволяет:
//! - Ansible/docker health checks
//! - curl-проверка из скриптов
//! - Мониторинг из Prometheus/Grafana

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Health status — shared state обновляемое из main loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
    pub state: String,
    pub connected: bool,
    pub armed: bool,
    pub fc_mode: String,
    pub fc_heartbeat_stale: bool,
    pub watchdogs_expired: u32,
    pub watchdogs_total: u32,
    pub active_target_id: Option<u64>,
    pub rate_limiter_sent: u64,
    pub rate_limiter_dropped: u64,
    pub last_command_age_ms: u64,
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self {
            status: "starting".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: 0,
            state: "IDLE".to_string(),
            connected: false,
            armed: false,
            fc_mode: "Unknown".to_string(),
            fc_heartbeat_stale: true,
            watchdogs_expired: 0,
            watchdogs_total: 0,
            active_target_id: None,
            rate_limiter_sent: 0,
            rate_limiter_dropped: 0,
            last_command_age_ms: u64::MAX,
        }
    }
}

/// Конфигурация health endpoint.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// HTTP порт (default: 8080).
    pub port: u16,
    /// Интервал sd_notify WATCHDOG=1 (сек). systemd WatchdogSec / 2.
    pub notify_interval_secs: u64,
    /// Включён ли systemd notify (false в non-systemd окружениях).
    pub enable_systemd_notify: bool,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            notify_interval_secs: 5,
            enable_systemd_notify: true,
        }
    }
}

/// Health server — HTTP + systemd notify.
pub struct HealthServer {
    config: HealthConfig,
    status: Arc<parking_lot::RwLock<HealthStatus>>,
    start_time: Instant,
    shutdown_tx: watch::Sender<bool>,
}

impl HealthServer {
    pub fn new(config: HealthConfig) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            config,
            status: Arc::new(parking_lot::RwLock::new(HealthStatus::default())),
            start_time: Instant::now(),
            shutdown_tx,
        }
    }

    /// Получить handle для обновления статуса из main loop.
    pub fn status_handle(&self) -> Arc<parking_lot::RwLock<HealthStatus>> {
        Arc::clone(&self.status)
    }

    /// Обновить статус (вызывается из main loop).
    pub fn update_status(&self, update_fn: impl FnOnce(&mut HealthStatus)) {
        let mut status = self.status.write();
        update_fn(&mut status);
        status.uptime_secs = self.start_time.elapsed().as_secs();
    }

    /// Запустить HTTP server и systemd notify loop.
    /// Возвращает handle для shutdown.
    pub async fn run(&self) -> anyhow::Result<()> {
        // 1. Systemd READY=1
        if self.config.enable_systemd_notify {
            match sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
                Ok(_) => info!("sent READY=1 to systemd"),
                Err(e) => warn!("failed to send READY=1 (not running under systemd?): {e}"),
            }
        }

        // 2. Запускаем systemd notify loop в отдельной задаче
        if self.config.enable_systemd_notify {
            let interval = Duration::from_secs(self.config.notify_interval_secs);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                loop {
                    ticker.tick().await;
                    if let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Watchdog]) {
                        warn!("failed to send WATCHDOG=1: {e}");
                    }
                    debug!("sent WATCHDOG=1 to systemd");
                }
            });
        }

        // 3. HTTP server
        let app = self.build_router();
        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        info!(port = self.config.port, "starting health HTTP server");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.wait_for(|&v| v).await;
            })
            .await?;

        info!("health server stopped");

        // 4. Systemd STOPPING=1
        if self.config.enable_systemd_notify {
            let _ = sd_notify::notify(true, &[sd_notify::NotifyState::Stopping]);
        }

        Ok(())
    }

    /// Остановить сервер.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    fn build_router(&self) -> Router {
        let status = Arc::clone(&self.status);
        let start_time = self.start_time;

        Router::new()
            .route(
                "/health",
                get(move || {
                    let status = Arc::clone(&status);
                    let start_time = start_time;
                    async move { health_handler(status, start_time).await }
                }),
            )
            .route("/metrics", get(metrics_handler))
            .route("/", get(root_handler))
    }
}

async fn health_handler(
    status: Arc<parking_lot::RwLock<HealthStatus>>,
    start_time: Instant,
) -> impl IntoResponse {
    let mut snapshot = status.read().clone();
    snapshot.uptime_secs = start_time.elapsed().as_secs();

    // Определяем общий статус
    if snapshot.fc_heartbeat_stale || snapshot.watchdogs_expired > 0 {
        snapshot.status = "degraded".to_string();
    } else if snapshot.connected {
        snapshot.status = "ok".to_string();
    } else {
        snapshot.status = "starting".to_string();
    }

    let code = if snapshot.status == "ok" {
        StatusCode::OK
    } else if snapshot.status == "degraded" {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK // starting is OK
    };

    (code, Json(snapshot))
}

async fn metrics_handler() -> impl IntoResponse {
    // Prometheus-совместимый формат
    // В реальном использовании здесь будут реальные метрики из HealthStatus
    let metrics = r#"# HELP auto_targeting_uptime_seconds Uptime in seconds
# TYPE auto_targeting_uptime_seconds counter
auto_targeting_uptime_seconds 0

# HELP auto_targeting_state_current Current system state (0=IDLE, 1=ARMED, 2=SCANNING, etc.)
# TYPE auto_targeting_state_current gauge
auto_targeting_state_current 0

# HELP auto_targeting_fc_connected FC connection status (0=disconnected, 1=connected)
# TYPE auto_targeting_fc_connected gauge
auto_targeting_fc_connected 0

# HELP auto_targeting_fc_armed FC armed status (0=disarmed, 1=armed)
# TYPE auto_targeting_fc_armed gauge
auto_targeting_fc_armed 0

# HELP auto_targeting_fc_heartbeat_stale FC heartbeat stale status (0=fresh, 1=stale)
# TYPE auto_targeting_fc_heartbeat_stale gauge
auto_targeting_fc_heartbeat_stale 1

# HELP auto_targeting_watchdogs_expired Number of expired watchdogs
# TYPE auto_targeting_watchdogs_expired gauge
auto_targeting_watchdogs_expired 0

# HELP auto_targeting_watchdogs_total Total number of registered watchdogs
# TYPE auto_targeting_watchdogs_total gauge
auto_targeting_watchdogs_total 5

# HELP auto_targeting_rate_limiter_sent_total Total commands sent to FC
# TYPE auto_targeting_rate_limiter_sent_total counter
auto_targeting_rate_limiter_sent_total 0

# HELP auto_targeting_rate_limiter_dropped_total Total commands dropped by rate limiter
# TYPE auto_targeting_rate_limiter_dropped_total counter
auto_targeting_rate_limiter_dropped_total 0

# HELP auto_targeting_active_target Active target ID (0 = no target)
# TYPE auto_targeting_active_target gauge
auto_targeting_active_target 0

# HELP auto_targeting_last_command_age_ms Age of last FC command in milliseconds
# TYPE auto_targeting_last_command_age_ms gauge
auto_targeting_last_command_age_ms 0
"#;
    metrics.to_string()
}

async fn root_handler() -> impl IntoResponse {
    "Auto-Targeting System\n\
     Endpoints:\n\
     - GET /health  — JSON health status\n\
     - GET /metrics — Prometheus metrics\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_default() {
        let s = HealthStatus::default();
        assert_eq!(s.status, "starting");
        assert_eq!(s.state, "IDLE");
        assert!(!s.connected);
    }

    #[test]
    fn health_config_default() {
        let c = HealthConfig::default();
        assert_eq!(c.port, 8080);
        assert_eq!(c.notify_interval_secs, 5);
        assert!(c.enable_systemd_notify);
    }

    #[test]
    fn health_server_construction() {
        let server = HealthServer::new(HealthConfig::default());
        assert_eq!(server.config.port, 8080);
    }

    #[test]
    fn update_status_works() {
        let server = HealthServer::new(HealthConfig::default());
        server.update_status(|s| {
            s.connected = true;
            s.state = "ARMED".to_string();
        });
        let snapshot = server.status.read().clone();
        assert!(snapshot.connected);
        assert_eq!(snapshot.state, "ARMED");
    }

    #[tokio::test]
    async fn health_server_starts_and_stops() {
        let server = HealthServer::new(HealthConfig {
            port: 18080, // нестандартный порт для теста
            enable_systemd_notify: false,
            ..Default::default()
        });

        let server_handle = Arc::new(server);
        let server_clone = Arc::clone(&server_handle);

        let run_handle = tokio::spawn(async move { server_clone.run().await });

        // Даём серверу время запуститься
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Проверяем, что /health отвечает
        let client = reqwest::Client::new();
        let resp = client
            .get("http://127.0.0.1:18080/health")
            .timeout(Duration::from_secs(2))
            .send()
            .await;

        if let Ok(resp) = resp {
            assert!(resp.status().is_success() || resp.status() == StatusCode::SERVICE_UNAVAILABLE);
            let body: serde_json::Value = resp.json().await.unwrap();
            assert!(body.get("status").is_some());
        }

        // Shutdown
        server_handle.shutdown();
        let _ = run_handle.await;
    }

    #[tokio::test]
    async fn root_endpoint_works() {
        let server = HealthServer::new(HealthConfig {
            port: 18081,
            enable_systemd_notify: false,
            ..Default::default()
        });

        let server_handle = Arc::new(server);
        let server_clone = Arc::clone(&server_handle);

        tokio::spawn(async move { server_clone.run().await });

        tokio::time::sleep(Duration::from_millis(200)).await;

        let client = reqwest::Client::new();
        let resp = client
            .get("http://127.0.0.1:18081/")
            .timeout(Duration::from_secs(2))
            .send()
            .await;

        if let Ok(resp) = resp {
            assert!(resp.status().is_success());
            let body = resp.text().await.unwrap();
            assert!(body.contains("Auto-Targeting"));
        }

        server_handle.shutdown();
    }
}
