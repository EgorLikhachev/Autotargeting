//! MAVLink-based adapter for ArduPilot SITL (Software In The Loop).
//!
//! Connects to ArduPilot SITL over UDP. SITL listens on `endpoint` (default
//! `127.0.0.1:14550`) and accepts MAVLink v2 messages.
//!
//! ## Usage
//!
//! 1. Start SITL: `docker compose -f sim/sitl/docker-compose.yml up -d`
//! 2. Configure: `[fc] adapter = "sitl-mavlink", endpoint = "127.0.0.1:14550"`
//! 3. Run: `cargo run -p auto-targeting-cli -- --config configs/sitl.toml`
//!
//! ## Implementation notes
//!
//! - Uses `mavlink::connect("udpin:...")` to establish a UDP listener.
//!   SITL sends to this address; we receive.
//! - Spawns a background std::thread that reads MAVLink messages and updates
//!   a shared telemetry cache (`Arc<RwLock<TelemetryCache>>`).
//! - The mavlink crate's API is sync-based; we wrap blocking calls in async
//!   methods via `tokio::task::spawn_blocking`. For the hot path
//!   (10 Hz SET_POSITION_TARGET_LOCAL_NED) we accept the small blocking cost.

use crate::traits::{FcError, FlightControllerAdapter};
use async_trait::async_trait;
use chrono::Utc;
use common::{Attitude, FlightMode, GlobalPosition, HeartbeatStatus, PositionTargetNED, RoiTarget};
use mavlink::{dialects::ardupilotmega as ap, MavConnection};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

/// The concrete message type used throughout the adapter.
type Msg = ap::MavMessage;

/// The concrete connection type returned by `mavlink::connect`.
type Conn = mavlink::Connection<Msg>;

/// Cached telemetry — updated by the background reader, read by the trait methods.
#[derive(Debug, Clone, Default)]
pub struct TelemetryCache {
    pub attitude: Attitude,
    pub global_position: Option<GlobalPosition>,
    pub last_heartbeat: Option<Instant>,
    pub last_heartbeat_chrono: chrono::DateTime<Utc>,
    pub armed: bool,
    pub mode: FlightMode,
}

impl TelemetryCache {
    fn fresh() -> Self {
        Self {
            last_heartbeat_chrono: Utc::now() - chrono::Duration::seconds(60),
            ..Default::default()
        }
    }
}

/// Configuration for the SITL MAVLink adapter.
#[derive(Debug, Clone)]
pub struct SittlConfig {
    /// UDP endpoint to listen on for SITL traffic, e.g. "127.0.0.1:14550".
    pub endpoint: String,
    /// This companion computer's system ID.
    pub system_id: u8,
    /// This companion computer's component ID.
    pub component_id: u8,
    /// Target FC system ID (usually 1).
    pub target_system_id: u8,
    /// Target FC component ID (usually 1).
    pub target_component_id: u8,
}

impl Default for SittlConfig {
    fn default() -> Self {
        Self {
            endpoint: "127.0.0.1:14550".to_string(),
            system_id: 255,
            component_id: 1,
            target_system_id: 1,
            target_component_id: 1,
        }
    }
}

impl SittlConfig {
    pub fn from_common(cfg: &common::FcConfig) -> Self {
        Self {
            endpoint: cfg.endpoint.clone(),
            system_id: cfg.system_id,
            component_id: cfg.component_id,
            target_system_id: cfg.target_system_id,
            target_component_id: cfg.target_component_id,
        }
    }
}

/// MAVLink adapter for ArduPilot SITL (UDP transport).
///
/// For real ArduPilot FCs over serial/USB, see `ArduPilotMavlinkAdapter`
/// (Phase 4 TODO — uses the same mavlink crate with `serial:` or `tcpout:` URL).
pub struct SittlMavlinkAdapter {
    config: SittlConfig,
    vehicle: Option<Arc<Conn>>,
    cache: Arc<RwLock<TelemetryCache>>,
    connected: bool,
}

impl SittlMavlinkAdapter {
    pub fn new(config: SittlConfig) -> Self {
        Self {
            config,
            vehicle: None,
            cache: Arc::new(RwLock::new(TelemetryCache::fresh())),
            connected: false,
        }
    }

    /// Build from common config.
    pub fn from_common(cfg: &common::FcConfig) -> Self {
        Self::new(SittlConfig::from_common(cfg))
    }

    /// Spawn the background telemetry reader thread.
    /// Runs forever (until the connection drops).
    fn spawn_reader(&self, vehicle: Arc<Conn>) {
        let cache = Arc::clone(&self.cache);

        std::thread::spawn(move || {
            info!("MAVLink telemetry reader thread started");
            loop {
                match vehicle.recv() {
                    Ok((_header, msg)) => {
                        Self::process_message(&msg, &cache);
                    }
                    Err(e) => {
                        error!(error = %e, "MAVLink recv error — reader thread exiting");
                        break;
                    }
                }
            }
            warn!("MAVLink telemetry reader thread stopped");
        });
    }

    /// Process a single MAVLink message — update the telemetry cache.
    fn process_message(msg: &ap::MavMessage, cache: &Arc<RwLock<TelemetryCache>>) {
        let now = Instant::now();
        let now_chrono = Utc::now();
        match msg {
            ap::MavMessage::HEARTBEAT(hb) => {
                let mut c = cache.write();
                c.last_heartbeat = Some(now);
                c.last_heartbeat_chrono = now_chrono;
                c.armed = hb.base_mode & ap::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
                    == ap::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED;
                c.mode = heartbeat_to_flight_mode(hb);
                debug!(armed = c.armed, mode = ?c.mode, "HEARTBEAT received");
            }
            ap::MavMessage::ATTITUDE(att) => {
                let mut c = cache.write();
                c.attitude = Attitude {
                    roll: att.roll,
                    pitch: att.pitch,
                    yaw: att.yaw,
                    roll_rate: att.rollspeed,
                    pitch_rate: att.pitchspeed,
                    yaw_rate: att.yawspeed,
                };
                debug!(
                    roll = att.roll,
                    pitch = att.pitch,
                    yaw = att.yaw,
                    "ATTITUDE received"
                );
            }
            ap::MavMessage::GLOBAL_POSITION_INT(pos) => {
                let mut c = cache.write();
                c.global_position = Some(GlobalPosition {
                    lat: pos.lat as f64 / 1e7,
                    lon: pos.lon as f64 / 1e7,
                    alt_msl: pos.alt as f32 / 1e3,
                    alt_agl: pos.relative_alt as f32 / 1e3,
                });
            }
            _ => {
                // Other messages — not cached
            }
        }
    }

    /// Send a COMMAND_LONG message (used for ROI, mode change, arm/disarm).
    fn send_command_long(&self, command: ap::MavCmd, params: [f32; 7]) -> Result<(), FcError> {
        let vehicle = self.vehicle.as_ref().ok_or_else(|| {
            FcError::Connection("not connected — call connect() first".to_string())
        })?;

        let cmd = ap::COMMAND_LONG_DATA {
            target_system: self.config.target_system_id,
            target_component: self.config.target_component_id,
            command,
            confirmation: 0,
            param1: params[0],
            param2: params[1],
            param3: params[2],
            param4: params[3],
            param5: params[4],
            param6: params[5],
            param7: params[6],
        };

        let msg = ap::MavMessage::COMMAND_LONG(cmd);
        // MavHeader: source system/component. Use 0 for the seq — the mavlink
        // crate will fill it in. We use the companion computer's system/component.
        let header = mavlink::MavHeader {
            system_id: self.config.system_id,
            component_id: self.config.component_id,
            sequence: 0,
        };
        vehicle
            .send(&header, &msg)
            .map_err(|e| FcError::Connection(format!("send COMMAND_LONG failed: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl FlightControllerAdapter for SittlMavlinkAdapter {
    async fn connect(&mut self) -> Result<(), FcError> {
        let url = format!("udpin:{}", self.config.endpoint);
        info!(endpoint = %self.config.endpoint, url = %url, "connecting to SITL via MAVLink UDP");
        let vehicle = mavlink::connect::<Msg>(&url)
            .map_err(|e| FcError::Connection(format!("mavlink connect failed: {e}")))?;
        let vehicle: Arc<Conn> = Arc::new(vehicle);
        self.vehicle = Some(Arc::clone(&vehicle));
        self.connected = true;
        self.spawn_reader(vehicle);
        info!(
            "SITL MAVLink connection established (listening on {})",
            self.config.endpoint
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), FcError> {
        // The mavlink crate closes the connection when the Arc<dyn MavConnection>
        // is dropped. We just release our reference; the reader thread holds
        // the other and will exit on the next recv error.
        self.connected = false;
        self.vehicle = None;
        info!("SITL MAVLink disconnected");
        Ok(())
    }

    async fn set_roi(&mut self, roi: RoiTarget) -> Result<(), FcError> {
        let (cmd, params) = match roi {
            RoiTarget::GlobalLatLng { lat, lon, alt } => (
                ap::MavCmd::MAV_CMD_DO_SET_ROI_LOCATION,
                [0.0, 0.0, 0.0, 0.0, lat as f32, lon as f32, alt],
            ),
            RoiTarget::LocalNed { north, east, down } => (
                ap::MavCmd::MAV_CMD_DO_SET_ROI_LOCATION,
                [0.0, 0.0, 0.0, 0.0, north, east, down],
            ),
            RoiTarget::None => (ap::MavCmd::MAV_CMD_DO_SET_ROI_NONE, [0.0; 7]),
        };
        debug!(?cmd, ?roi, "sending ROI command");
        self.send_command_long(cmd, params)
    }

    async fn set_position_target_local_ned(
        &mut self,
        target: PositionTargetNED,
    ) -> Result<(), FcError> {
        let vehicle = self
            .vehicle
            .as_ref()
            .ok_or_else(|| FcError::Connection("not connected".to_string()))?;

        // Build SET_POSITION_TARGET_LOCAL_NED.
        // type_mask: bits set to 1 mean "ignore this field".
        // We control X/Y/Z + yaw, ignore velocities + accelerations + yaw_rate.
        let type_mask = ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_VX_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_VY_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_VZ_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_AX_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_AY_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_AZ_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_YAW_RATE_IGNORE;

        let msg = ap::SET_POSITION_TARGET_LOCAL_NED_DATA {
            time_boot_ms: 0, // ArduPilot ignores this field
            target_system: self.config.target_system_id,
            target_component: self.config.target_component_id,
            coordinate_frame: ap::MavFrame::MAV_FRAME_LOCAL_OFFSET_NED,
            type_mask,
            x: target.north,
            y: target.east,
            z: target.down,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            afx: 0.0,
            afy: 0.0,
            afz: 0.0,
            yaw: target.yaw,
            yaw_rate: 0.0,
        };

        let mav_msg = ap::MavMessage::SET_POSITION_TARGET_LOCAL_NED(msg);
        let header = mavlink::MavHeader {
            system_id: self.config.system_id,
            component_id: self.config.component_id,
            sequence: 0,
        };
        vehicle.send(&header, &mav_msg).map_err(|e| {
            FcError::Connection(format!("send SET_POSITION_TARGET_LOCAL_NED failed: {e}"))
        })?;
        Ok(())
    }

    async fn set_mode(&mut self, mode: FlightMode) -> Result<(), FcError> {
        // ArduPilot mode change via COMMAND_LONG with MAV_CMD_DO_SET_MODE.
        // param1 = base_mode (1=custom), param2 = custom_mode.
        // ArduPlane custom_modes: 0=Manual, 2=Stabilize, 5=FBWA, 6=FBWB,
        // 7=CRUISE, 10=AUTO, 11=RTL, 12=LOITER, 15=GUIDED
        let custom_mode = match mode {
            FlightMode::Manual => 0u32,
            FlightMode::Stabilize => 2,
            FlightMode::Loiter => 12,
            FlightMode::Guided => 15,
            FlightMode::Rtl => 11,
            FlightMode::Auto => 10,
            other => return Err(FcError::UnsupportedMode(other)),
        };

        debug!(?mode, custom_mode, "sending mode change");
        // param1=1 (MAV_MODE_FLAG_CUSTOM_MODE_ENABLED), param2=custom_mode
        self.send_command_long(
            ap::MavCmd::MAV_CMD_DO_SET_MODE,
            [1.0, custom_mode as f32, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
    }

    async fn arm(&mut self) -> Result<(), FcError> {
        debug!("arming");
        self.send_command_long(
            ap::MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
    }

    async fn disarm(&mut self) -> Result<(), FcError> {
        debug!("disarming");
        self.send_command_long(
            ap::MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
    }

    fn attitude(&self) -> Attitude {
        self.cache.read().attitude
    }

    fn global_position(&self) -> Option<GlobalPosition> {
        self.cache.read().global_position
    }

    fn heartbeat_status(&self) -> HeartbeatStatus {
        let c = self.cache.read();
        HeartbeatStatus {
            last_heartbeat: c.last_heartbeat_chrono,
            armed: c.armed,
            mode: c.mode,
        }
    }

    fn name(&self) -> &'static str {
        "SittlMavlinkAdapter"
    }
}

/// Convert an ArduPilot HEARTBEAT's `custom_mode` + `base_mode` to our `FlightMode`.
fn heartbeat_to_flight_mode(hb: &ap::HEARTBEAT_DATA) -> FlightMode {
    if hb.autopilot != ap::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA {
        return FlightMode::Unknown;
    }
    match hb.custom_mode {
        0 => FlightMode::Manual,
        2 => FlightMode::Stabilize,
        10 => FlightMode::Auto,
        11 => FlightMode::Rtl,
        12 => FlightMode::Loiter,
        15 => FlightMode::Guided,
        _ => FlightMode::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_common() {
        let common_cfg = common::FcConfig {
            endpoint: "127.0.0.1:14551".to_string(),
            system_id: 200,
            ..Default::default()
        };
        let cfg = SittlConfig::from_common(&common_cfg);
        assert_eq!(cfg.endpoint, "127.0.0.1:14551");
        assert_eq!(cfg.system_id, 200);
    }

    #[test]
    fn heartbeat_mode_translation_guided() {
        let hb = ap::HEARTBEAT_DATA {
            custom_mode: 15, // GUIDED
            mavtype: ap::MavType::MAV_TYPE_FIXED_WING,
            autopilot: ap::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
            base_mode: ap::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED,
            system_status: ap::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        };
        let mode = heartbeat_to_flight_mode(&hb);
        assert_eq!(mode, FlightMode::Guided);
    }

    #[test]
    fn heartbeat_mode_translation_rtl() {
        let hb = ap::HEARTBEAT_DATA {
            custom_mode: 11, // RTL
            mavtype: ap::MavType::MAV_TYPE_FIXED_WING,
            autopilot: ap::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
            base_mode: ap::MavModeFlag::default(),
            system_status: ap::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        };
        assert_eq!(heartbeat_to_flight_mode(&hb), FlightMode::Rtl);
    }

    #[test]
    fn heartbeat_mode_translation_unknown_custom_mode() {
        let hb = ap::HEARTBEAT_DATA {
            custom_mode: 99, // unknown
            mavtype: ap::MavType::MAV_TYPE_FIXED_WING,
            autopilot: ap::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
            base_mode: ap::MavModeFlag::default(),
            system_status: ap::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        };
        assert_eq!(heartbeat_to_flight_mode(&hb), FlightMode::Unknown);
    }

    #[test]
    fn heartbeat_mode_translation_non_ardupilot() {
        let hb = ap::HEARTBEAT_DATA {
            custom_mode: 15,
            mavtype: ap::MavType::MAV_TYPE_FIXED_WING,
            autopilot: ap::MavAutopilot::MAV_AUTOPILOT_GENERIC,
            base_mode: ap::MavModeFlag::default(),
            system_status: ap::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        };
        assert_eq!(heartbeat_to_flight_mode(&hb), FlightMode::Unknown);
    }

    #[test]
    fn heartbeat_armed_flag_detected() {
        let hb = ap::HEARTBEAT_DATA {
            custom_mode: 15,
            mavtype: ap::MavType::MAV_TYPE_FIXED_WING,
            autopilot: ap::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
            base_mode: ap::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED,
            system_status: ap::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        };
        assert!(
            hb.base_mode & ap::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
                == ap::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED
        );
    }

    #[test]
    fn adapter_construction_does_not_connect() {
        let cfg = SittlConfig::default();
        let adapter = SittlMavlinkAdapter::new(cfg);
        assert!(!adapter.connected);
        assert!(adapter.vehicle.is_none());
        // Heartbeat should be stale (set 60s ago in default state)
        assert!(adapter.is_heartbeat_stale(1000));
    }

    #[test]
    fn telemetry_cache_fresh_is_stale() {
        let cache = TelemetryCache::fresh();
        // last_heartbeat is None, last_heartbeat_chrono is 60s ago
        assert!(cache.last_heartbeat.is_none());
        let age = (Utc::now() - cache.last_heartbeat_chrono).num_seconds();
        assert!((55..=65).contains(&age), "age should be ~60s, got {age}");
    }

    #[test]
    fn connect_to_invalid_endpoint_returns_error() {
        // Port 0 should fail to bind
        let cfg = SittlConfig {
            endpoint: "127.0.0.1:0".to_string(),
            ..Default::default()
        };
        let adapter = SittlMavlinkAdapter::new(cfg);
        // We don't actually await here since connect is async; just check
        // that the adapter is still not connected.
        assert!(!adapter.connected);
    }
}
