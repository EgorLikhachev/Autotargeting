//! # fc-bridge — мост FC ↔ шина (M3, BUS_MIGRATION_PLAN)
//!
//! Направления:
//! - **Телеметрия → шина**: опрос кэша адаптера (attitude/GPS/heartbeat)
//!   с фиксированной частотой → `at/telemetry` (`TelemetrySample`), рёбра
//!   событий (mode/armed/heartbeat-alive) → `at/fc_events` (`FcEvent`).
//! - **Шина → FC**: подписка `at/commands` (`CommandMsg`, target="fc") →
//!   методы `FlightControllerAdapter` (set_roi / set_pos_ned / set_mode /
//!   arm / disarm). Команды fire-and-forget, как и адаптер.
//! - Статус `at/status/fc` раз в секунду.
//!
//! Адаптер выбирается трейтом `build_adapter` (mock | sitl-mavlink |
//! ardupilot-mavlink) — мост не знает конкретной реализации.
//!
//! ## Контракт команд (target="fc")
//!
//! ```json
//! {"cmd":"set_roi","args":{"lat":55.75,"lon":37.62,"alt":100.0}}
//! {"cmd":"set_roi","args":{"none":true}}
//! {"cmd":"set_pos_ned","args":{"north":1.0,"east":0.0,"down":0.0,"yaw":0.0}}
//! {"cmd":"set_mode","args":{"mode":"guided"}}
//! {"cmd":"arm"} / {"cmd":"disarm"}
//! ```

use std::time::{Duration, Instant};

use common::{FlightMode, GlobalPosition};
use event_bus::{topics, CommandMsg, EventBus, FcEvent, TelemetrySample, CONTRACT_VERSION};
use fc_adapter::FlightControllerAdapter;

/// Ошибки моста.
#[derive(thiserror::Error, Debug)]
pub enum BridgeError {
    #[error("bus: {0}")]
    Bus(#[from] event_bus::BusError),
    #[error("fc: {0}")]
    Fc(#[from] fc_adapter::FcError),
    #[error("bad command: {0}")]
    BadCommand(String),
}

/// Конфигурация моста.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Частота публикации телеметрии.
    pub telemetry_hz: u32,
    /// Таймаут heartbeat для событий «связь потеряна/восстановлена».
    pub heartbeat_timeout_ms: u64,
    /// Период статуса.
    pub status_interval: Duration,
    /// Прекратить после N секунд (None — пока жив процесс).
    pub max_duration: Option<Duration>,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            telemetry_hz: 10,
            heartbeat_timeout_ms: 2000,
            status_interval: Duration::from_secs(1),
            max_duration: None,
        }
    }
}

/// Статус моста на шине (at/status/fc).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FcBridgeStatus {
    pub v: u8,
    pub adapter: String,
    pub heartbeat_alive: bool,
    pub mode: String,
    pub armed: bool,
    pub telemetry_published: u64,
    pub commands_handled: u64,
    pub command_errors: u64,
    pub telemetry_hz_actual: f32,
}

/// Метрики моста.
#[derive(Debug, Clone, Copy, Default)]
pub struct BridgeStats {
    pub telemetry_published: u64,
    pub commands_handled: u64,
    pub command_errors: u64,
}

/// FlightMode → стабильный u8-код для TelemetrySample.mode
/// (обратный к карте custom_mode в fc-adapter).
#[must_use]
pub fn mode_to_u8(m: FlightMode) -> u8 {
    match m {
        FlightMode::Unknown => 255,
        FlightMode::Manual => 0,
        FlightMode::Stabilize => 2,
        FlightMode::Auto => 10,
        FlightMode::Rtl => 11,
        FlightMode::Loiter => 12,
        FlightMode::Guided => 15,
        FlightMode::AltHold => 17,
    }
}

/// Мост FC ↔ шина.
pub struct FcBridge {
    cfg: BridgeConfig,
}

impl FcBridge {
    #[must_use]
    pub fn new(cfg: BridgeConfig) -> Self {
        Self { cfg }
    }

    /// Основной цикл. Адаптер уже подключен (connect() вызван снаружи).
    pub async fn run(
        &self,
        adapter: &mut dyn FlightControllerAdapter,
        bus: &EventBus,
    ) -> Result<BridgeStats, BridgeError> {
        let tele_pub = bus.publish_telemetry().await?;
        let fc_pub = bus.publisher::<FcEvent>(topics::FC_EVENTS).await?;
        let status_pub = bus
            .publisher::<FcBridgeStatus>(&topics::status("fc"))
            .await?;
        let cmd_sub = bus.subscribe_commands().await?;
        // Declare-распространение между peer-ами.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut stats = BridgeStats::default();
        let telemetry_period =
            Duration::from_millis(u64::from(1000 / self.cfg.telemetry_hz.max(1)));
        let mut last_tele = Instant::now() - telemetry_period;
        let mut last_status = Instant::now();
        let started = Instant::now();

        // Рёбра событий FC (dedup-состояние).
        let mut prev_alive: Option<bool> = None;
        let mut prev_armed: Option<bool> = None;
        let mut prev_mode: Option<FlightMode> = None;

        // Окно FPS телеметрии.
        let mut win_frames: u64 = 0;
        let mut win_start = Instant::now();

        loop {
            if let Some(max) = self.cfg.max_duration {
                if started.elapsed() >= max {
                    break;
                }
            }

            // Команды (неблокирующе, в цикле телеметрии).
            while let Ok(cmd) = cmd_sub.recv_timeout(Duration::from_millis(0)).await {
                match self.dispatch(adapter, &cmd).await {
                    Ok(()) => stats.commands_handled += 1,
                    Err(e) => {
                        stats.command_errors += 1;
                        tracing::warn!(cmd = %cmd.cmd, error = %e, "command failed");
                    }
                }
            }

            // Телеметрия.
            if last_tele.elapsed() >= telemetry_period {
                last_tele = Instant::now();
                let att = adapter.attitude();
                let pos: Option<GlobalPosition> = adapter.global_position();
                let hb = adapter.heartbeat_status();
                let alive = !adapter.is_heartbeat_stale(self.cfg.heartbeat_timeout_ms);

                let sample = TelemetrySample {
                    t_ms: chrono::Utc::now().timestamp_millis(),
                    roll_deg: att.roll.to_degrees(),
                    pitch_deg: att.pitch.to_degrees(),
                    yaw_deg: att.yaw.to_degrees(),
                    alt_m: pos.as_ref().map_or(0.0, |p| p.alt_agl),
                    lat_deg: pos.as_ref().map_or(0.0, |p| p.lat),
                    lon_deg: pos.as_ref().map_or(0.0, |p| p.lon),
                    battery_v: 0.0, // не парсится текущими адаптерами
                    mode: mode_to_u8(hb.mode),
                };
                if tele_pub.publish(&sample).await.is_ok() {
                    stats.telemetry_published += 1;
                    win_frames += 1;
                }

                // Рёбра: alive / armed / mode.
                if prev_alive != Some(alive) {
                    let ev = FcEvent {
                        v: CONTRACT_VERSION,
                        kind: if alive { "link_up" } else { "link_down" }.into(),
                        detail: serde_json::json!({}),
                        at: chrono::Utc::now(),
                    };
                    // best-effort: потеря события телеметрии допустима (C6)
                    let _ = fc_pub.publish(&ev).await;
                    prev_alive = Some(alive);
                }
                if prev_armed != Some(hb.armed) {
                    let ev = FcEvent {
                        v: CONTRACT_VERSION,
                        kind: "armed".into(),
                        detail: serde_json::json!({"armed": hb.armed}),
                        at: chrono::Utc::now(),
                    };
                    let _ = fc_pub.publish(&ev).await;
                    prev_armed = Some(hb.armed);
                }
                if prev_mode.as_ref() != Some(&hb.mode) {
                    let ev = FcEvent {
                        v: CONTRACT_VERSION,
                        kind: "mode_change".into(),
                        detail: serde_json::json!({"mode": format!("{:?}", hb.mode)}),
                        at: chrono::Utc::now(),
                    };
                    let _ = fc_pub.publish(&ev).await;
                    prev_mode = Some(hb.mode);
                }
            }

            // Статус.
            if last_status.elapsed() >= self.cfg.status_interval {
                let secs = win_start.elapsed().as_secs_f32();
                let fps = if secs > 0.0 {
                    win_frames as f32 / secs
                } else {
                    0.0
                };
                let hb = adapter.heartbeat_status();
                let st = FcBridgeStatus {
                    v: CONTRACT_VERSION,
                    adapter: adapter.name().to_string(),
                    heartbeat_alive: !adapter.is_heartbeat_stale(self.cfg.heartbeat_timeout_ms),
                    mode: format!("{:?}", hb.mode),
                    armed: hb.armed,
                    telemetry_published: stats.telemetry_published,
                    commands_handled: stats.commands_handled,
                    command_errors: stats.command_errors,
                    telemetry_hz_actual: fps,
                };
                let _ = status_pub.publish(&st).await;
                win_frames = 0;
                win_start = Instant::now();
                last_status = Instant::now();
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(stats)
    }

    /// Диспетчер команд target="fc".
    async fn dispatch(
        &self,
        adapter: &mut dyn FlightControllerAdapter,
        cmd: &CommandMsg,
    ) -> Result<(), BridgeError> {
        if cmd.target != "fc" {
            return Err(BridgeError::BadCommand(format!(
                "target '{}' != 'fc'",
                cmd.target
            )));
        }
        let args = &cmd.args;
        match cmd.cmd.as_str() {
            "set_roi" => {
                if args.get("none").and_then(serde_json::Value::as_bool) == Some(true) {
                    adapter.set_roi(common::RoiTarget::None).await?;
                } else {
                    let lat = args["lat"].as_f64().ok_or_else(|| Self::need("lat"))?;
                    let lon = args["lon"].as_f64().ok_or_else(|| Self::need("lon"))?;
                    let alt = args
                        .get("alt")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0) as f32;
                    adapter
                        .set_roi(common::RoiTarget::GlobalLatLng { lat, lon, alt })
                        .await?;
                }
                Ok(())
            }
            "set_pos_ned" | "set_position_target_local_ned" => {
                let g = |k: &str| -> Result<f32, BridgeError> {
                    args[k]
                        .as_f64()
                        .map(|v| v as f32)
                        .ok_or_else(|| Self::need(k))
                };
                let t = common::PositionTargetNED {
                    north: g("north")?,
                    east: g("east")?,
                    down: g("down")?,
                    yaw: g("yaw")?,
                };
                adapter
                    .set_position_target_local_ned(t)
                    .await
                    .map_err(BridgeError::Fc)
            }
            "set_mode" => {
                let name = args["mode"].as_str().ok_or_else(|| Self::need("mode"))?;
                let mode = match name.to_ascii_lowercase().as_str() {
                    "manual" => FlightMode::Manual,
                    "stabilize" | "stabilised" => FlightMode::Stabilize,
                    "althold" | "alt_hold" => FlightMode::AltHold,
                    "loiter" => FlightMode::Loiter,
                    "guided" => FlightMode::Guided,
                    "rtl" | "return" => FlightMode::Rtl,
                    "auto" => FlightMode::Auto,
                    other => {
                        return Err(BridgeError::BadCommand(format!("mode '{other}'")));
                    }
                };
                adapter.set_mode(mode).await.map_err(BridgeError::Fc)
            }
            "arm" => adapter.arm().await.map_err(BridgeError::Fc),
            "disarm" => adapter.disarm().await.map_err(BridgeError::Fc),
            other => Err(BridgeError::BadCommand(format!("cmd '{other}'"))),
        }
    }

    fn need(field: &str) -> BridgeError {
        BridgeError::BadCommand(format!("missing arg '{field}'"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_map_roundtrip_keys() {
        assert_eq!(mode_to_u8(FlightMode::Guided), 15);
        assert_eq!(mode_to_u8(FlightMode::Unknown), 255);
        assert_eq!(mode_to_u8(FlightMode::Manual), 0);
    }
}
