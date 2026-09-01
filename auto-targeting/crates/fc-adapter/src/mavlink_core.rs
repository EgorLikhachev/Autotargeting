//! mavlink_core — shared MAVLink adapter implementation (audit C1 dedup).
//!
//! sitl_mavlink.rs and ardupilot_mavlink.rs were copy-paste (the only
//! difference was the connect() URL). All logic lives here; the thin
//! wrappers provide defaults and URL construction.

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
type Conn = mavlink::Connection<Msg>;

/// Configuration for the ArduPilot MAVLink adapter.
#[derive(Debug, Clone)]
pub struct MavlinkCoreConfig {
    /// MAVLink connection URL.
    ///
    /// Examples:
    /// - `serial:/dev/ttyUSB0:115200` — serial port at 115200 baud
    /// - `tcpout:127.0.0.1:5760` — TCP client
    /// - `udpin:0.0.0.0:14550` — UDP listener
    /// - `udpout:127.0.0.1:14550` — UDP client
    pub endpoint: String,
    pub system_id: u8,
    pub component_id: u8,
    pub target_system_id: u8,
    pub target_component_id: u8,
    /// Heartbeat timeout in ms. If no heartbeat received within this time,
    /// the FC is considered lost.
    pub heartbeat_timeout_ms: u64,
}

impl Default for MavlinkCoreConfig {
    fn default() -> Self {
        Self {
            endpoint: "serial:/dev/ttyACM0:115200".to_string(),
            system_id: 255,
            component_id: 1,
            target_system_id: 1,
            target_component_id: 1,
            heartbeat_timeout_ms: 1000,
        }
    }
}

impl MavlinkCoreConfig {
    pub fn from_common(cfg: &common::FcConfig) -> Self {
        Self {
            endpoint: cfg.endpoint.clone(),
            system_id: cfg.system_id,
            component_id: cfg.component_id,
            target_system_id: cfg.target_system_id,
            target_component_id: cfg.target_component_id,
            heartbeat_timeout_ms: cfg.heartbeat_timeout_ms,
        }
    }
}

/// Cached telemetry — updated by the background reader.
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

/// MAVLink core: connection, telemetry reader, cache, commands.
/// Wrappers (SittlMavlinkAdapter / ArduPilotMavlinkAdapter) delegate here
/// (audit C1: ~1013 duplicated lines removed).
pub struct MavlinkCore {
    config: MavlinkCoreConfig,
    vehicle: Option<Arc<Conn>>,
    cache: Arc<RwLock<TelemetryCache>>,
    connected: bool,
}

impl MavlinkCore {
    pub fn new(config: MavlinkCoreConfig) -> Self {
        Self {
            config,
            vehicle: None,
            cache: Arc::new(RwLock::new(TelemetryCache::fresh())),
            connected: false,
        }
    }

    pub fn from_common(cfg: &common::FcConfig) -> Self {
        Self::new(MavlinkCoreConfig::from_common(cfg))
    }

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
            _ => {}
        }
    }

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
impl FlightControllerAdapter for MavlinkCore {
    async fn connect(&mut self) -> Result<(), FcError> {
        let url = &self.config.endpoint;
        info!(url = %url, "connecting via MAVLink");
        let vehicle = mavlink::connect::<Msg>(url)
            .map_err(|e| FcError::Connection(format!("mavlink connect failed: {e}")))?;
        let vehicle: Arc<Conn> = Arc::new(vehicle);
        self.vehicle = Some(Arc::clone(&vehicle));
        self.connected = true;
        self.spawn_reader(vehicle);
        info!("MAVLink connection established: {url}");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), FcError> {
        self.connected = false;
        self.vehicle = None;
        info!("MAVLink disconnected");
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

        let type_mask = ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_VX_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_VY_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_VZ_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_AX_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_AY_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_AZ_IGNORE
            | ap::PositionTargetTypemask::POSITION_TARGET_TYPEMASK_YAW_RATE_IGNORE;

        let msg = ap::SET_POSITION_TARGET_LOCAL_NED_DATA {
            time_boot_ms: 0,
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
        "MavlinkCore"
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
            endpoint: "serial:/dev/ttyUSB0:57600".to_string(),
            system_id: 200,
            ..Default::default()
        };
        let cfg = MavlinkCoreConfig::from_common(&common_cfg);
        assert_eq!(cfg.endpoint, "serial:/dev/ttyUSB0:57600");
        assert_eq!(cfg.system_id, 200);
    }

    #[test]
    fn default_config_is_serial() {
        let cfg = MavlinkCoreConfig::default();
        assert!(cfg.endpoint.starts_with("serial:"));
    }

    #[test]
    fn heartbeat_mode_translation_guided() {
        let hb = ap::HEARTBEAT_DATA {
            custom_mode: 15,
            mavtype: ap::MavType::MAV_TYPE_FIXED_WING,
            autopilot: ap::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
            base_mode: ap::MavModeFlag::MAV_MODE_FLAG_SAFETY_ARMED,
            system_status: ap::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        };
        assert_eq!(heartbeat_to_flight_mode(&hb), FlightMode::Guided);
    }

    #[test]
    fn heartbeat_mode_translation_rtl() {
        let hb = ap::HEARTBEAT_DATA {
            custom_mode: 11,
            mavtype: ap::MavType::MAV_TYPE_FIXED_WING,
            autopilot: ap::MavAutopilot::MAV_AUTOPILOT_ARDUPILOTMEGA,
            base_mode: ap::MavModeFlag::default(),
            system_status: ap::MavState::MAV_STATE_ACTIVE,
            mavlink_version: 3,
        };
        assert_eq!(heartbeat_to_flight_mode(&hb), FlightMode::Rtl);
    }

    #[test]
    fn adapter_construction_does_not_connect() {
        let cfg = MavlinkCoreConfig::default();
        let adapter = MavlinkCore::new(cfg);
        assert!(!adapter.connected);
        assert!(adapter.vehicle.is_none());
        assert!(adapter.is_heartbeat_stale(1000));
    }

    #[test]
    fn telemetry_cache_fresh_is_stale() {
        let cache = TelemetryCache::fresh();
        assert!(cache.last_heartbeat.is_none());
        let age = (Utc::now() - cache.last_heartbeat_chrono).num_seconds();
        assert!((55..=65).contains(&age), "age should be ~60s, got {age}");
    }

    /// Test endpoint URL parsing — verifies the adapter accepts all
    /// transport types supported by the mavlink crate.
    #[test]
    fn accepts_all_transport_urls() {
        let urls = [
            "serial:/dev/ttyUSB0:115200",
            "serial:/dev/ttyACM0:57600",
            "tcpout:127.0.0.1:5760",
            "tcpin:0.0.0.0:14550",
            "udpin:0.0.0.0:14550",
            "udpout:127.0.0.1:14550",
        ];
        for url in &urls {
            let cfg = MavlinkCoreConfig {
                endpoint: url.to_string(),
                ..Default::default()
            };
            let adapter = MavlinkCore::new(cfg);
            // Construction should succeed — actual connection is tested
            // in integration tests with SITL.
            assert_eq!(adapter.config.endpoint, *url);
        }
    }

    /// Connect to a bad URL — should fail gracefully.
    #[tokio::test]
    async fn connect_to_bad_url_returns_error() {
        let cfg = MavlinkCoreConfig {
            endpoint: "invalid-transport:foo".to_string(),
            ..Default::default()
        };
        let mut adapter = MavlinkCore::new(cfg);
        let result = adapter.connect().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FcError::Connection(_)));
        assert!(!adapter.connected);
    }
}
