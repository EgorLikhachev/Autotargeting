//! MAVLink adapter for real ArduPilot flight controllers (serial/TCP/UDP).
//!
//! Production adapter. Delegates all logic to [`crate::mavlink_core::MavlinkCore`]
//! (audit C1: was a copy of the SITL adapter); this wrapper only sets
//! serial-by-default config and passes the endpoint URL through unchanged.
//!
//! Supported URLs: `serial:/dev/ttyACM0:115200`, `tcpout:host:port`,
//! `tcpin:addr:port`, `udpin:addr:port`, `udpout:host:port`.

use crate::mavlink_core::{MavlinkCore, MavlinkCoreConfig};
use crate::traits::{FcError, FlightControllerAdapter};
use async_trait::async_trait;
use common::{Attitude, FlightMode, GlobalPosition, HeartbeatStatus, PositionTargetNED, RoiTarget};

pub use crate::mavlink_core::TelemetryCache;

/// Configuration for the ArduPilot MAVLink adapter.
#[derive(Debug, Clone)]
pub struct ArduPilotConfig {
    /// MAVLink connection URL.
    pub endpoint: String,
    pub system_id: u8,
    pub component_id: u8,
    pub target_system_id: u8,
    pub target_component_id: u8,
    /// Heartbeat timeout in ms.
    pub heartbeat_timeout_ms: u64,
}

impl Default for ArduPilotConfig {
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

impl ArduPilotConfig {
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

    fn to_core(&self) -> MavlinkCoreConfig {
        MavlinkCoreConfig {
            endpoint: self.endpoint.clone(),
            system_id: self.system_id,
            component_id: self.component_id,
            target_system_id: self.target_system_id,
            target_component_id: self.target_component_id,
            heartbeat_timeout_ms: self.heartbeat_timeout_ms,
        }
    }
}

/// MAVLink adapter for real ArduPilot FCs (serial/TCP/UDP).
pub struct ArduPilotMavlinkAdapter {
    core: MavlinkCore,
    #[allow(dead_code)]
    config: ArduPilotConfig,
}

impl ArduPilotMavlinkAdapter {
    pub fn new(config: ArduPilotConfig) -> Self {
        let core = MavlinkCore::new(config.to_core());
        Self { core, config }
    }

    pub fn from_common(cfg: &common::FcConfig) -> Self {
        Self::new(ArduPilotConfig::from_common(cfg))
    }
}

#[async_trait]
impl FlightControllerAdapter for ArduPilotMavlinkAdapter {
    async fn connect(&mut self) -> Result<(), FcError> {
        self.core.connect().await
    }
    async fn disconnect(&mut self) -> Result<(), FcError> {
        self.core.disconnect().await
    }
    async fn set_roi(&mut self, roi: RoiTarget) -> Result<(), FcError> {
        self.core.set_roi(roi).await
    }
    async fn set_position_target_local_ned(
        &mut self,
        target: PositionTargetNED,
    ) -> Result<(), FcError> {
        self.core.set_position_target_local_ned(target).await
    }
    async fn set_mode(&mut self, mode: FlightMode) -> Result<(), FcError> {
        self.core.set_mode(mode).await
    }
    async fn arm(&mut self) -> Result<(), FcError> {
        self.core.arm().await
    }
    async fn disarm(&mut self) -> Result<(), FcError> {
        self.core.disarm().await
    }
    fn attitude(&self) -> Attitude {
        self.core.attitude()
    }
    fn global_position(&self) -> Option<GlobalPosition> {
        self.core.global_position()
    }
    fn heartbeat_status(&self) -> HeartbeatStatus {
        self.core.heartbeat_status()
    }
    fn name(&self) -> &'static str {
        "ArduPilotMavlinkAdapter"
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
        let cfg = ArduPilotConfig::from_common(&common_cfg);
        assert_eq!(cfg.endpoint, "serial:/dev/ttyUSB0:57600");
        assert_eq!(cfg.system_id, 200);
    }

    #[test]
    fn default_config_is_serial() {
        assert!(ArduPilotConfig::default().endpoint.starts_with("serial:"));
    }

    #[test]
    fn adapter_construction_does_not_connect() {
        let adapter = ArduPilotMavlinkAdapter::new(ArduPilotConfig::default());
        assert!(adapter.core.is_heartbeat_stale(1000));
    }

    /// Все transport-URL принимаются конструкцией (коннект — SITL-тесты).
    #[test]
    fn accepts_all_transport_urls() {
        for url in [
            "serial:/dev/ttyUSB0:115200",
            "serial:/dev/ttyACM0:57600",
            "tcpout:127.0.0.1:5760",
            "tcpin:0.0.0.0:14550",
            "udpin:0.0.0.0:14550",
            "udpout:127.0.0.1:14550",
        ] {
            let cfg = ArduPilotConfig {
                endpoint: url.to_string(),
                ..Default::default()
            };
            let _ = ArduPilotMavlinkAdapter::new(cfg);
        }
    }

    /// Плохой URL — graceful error.
    #[tokio::test]
    async fn connect_to_bad_url_returns_error() {
        let cfg = ArduPilotConfig {
            endpoint: "invalid-transport:foo".to_string(),
            ..Default::default()
        };
        let mut adapter = ArduPilotMavlinkAdapter::new(cfg);
        let result = adapter.connect().await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FcError::Connection(_)));
    }
}
