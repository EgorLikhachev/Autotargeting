//! MAVLink adapter for ArduPilot SITL (UDP transport).
//!
//! Delegates all logic to [`crate::mavlink_core::MavlinkCore`] (audit C1);
//! this wrapper only constructs the `udpin:` URL from the bare
//! `host:port` endpoint and sets SITL-friendly defaults.
//!
//! Usage:
//! 1. Start SITL: `docker compose -f sim/sitl/docker-compose.yml up -d`
//! 2. Configure: `[fc] adapter = "sitl-mavlink", endpoint = "127.0.0.1:14550"`
//! 3. Run: `cargo run -p auto-targeting-cli -- --config configs/sitl.toml`

use crate::mavlink_core::{MavlinkCore, MavlinkCoreConfig};
use crate::traits::{FcError, FlightControllerAdapter};
use async_trait::async_trait;
use common::{Attitude, FlightMode, GlobalPosition, HeartbeatStatus, PositionTargetNED, RoiTarget};

pub use crate::mavlink_core::TelemetryCache;

/// Configuration for the SITL MAVLink adapter.
#[derive(Debug, Clone)]
pub struct SittlConfig {
    /// UDP endpoint to listen on for SITL traffic, e.g. "127.0.0.1:14550".
    pub endpoint: String,
    pub system_id: u8,
    pub component_id: u8,
    pub target_system_id: u8,
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

    fn to_core(&self) -> MavlinkCoreConfig {
        MavlinkCoreConfig {
            // SITL-специфика: единственный различавшийся кусок копипасты.
            endpoint: format!("udpin:{}", self.endpoint),
            system_id: self.system_id,
            component_id: self.component_id,
            target_system_id: self.target_system_id,
            target_component_id: self.target_component_id,
            heartbeat_timeout_ms: 1000,
        }
    }
}

/// MAVLink adapter for ArduPilot SITL (UDP transport).
pub struct SittlMavlinkAdapter {
    core: MavlinkCore,
    #[allow(dead_code)]
    config: SittlConfig,
}

impl SittlMavlinkAdapter {
    pub fn new(config: SittlConfig) -> Self {
        let core = MavlinkCore::new(config.to_core());
        Self { core, config }
    }

    pub fn from_common(cfg: &common::FcConfig) -> Self {
        Self::new(SittlConfig::from_common(cfg))
    }
}

#[async_trait]
impl FlightControllerAdapter for SittlMavlinkAdapter {
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
        "SittlMavlinkAdapter"
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
    }

    #[test]
    fn core_url_gets_udpin_prefix() {
        let core_cfg = SittlConfig::default().to_core();
        assert_eq!(core_cfg.endpoint, "udpin:127.0.0.1:14550");
    }

    #[test]
    fn adapter_construction_does_not_connect() {
        let adapter = SittlMavlinkAdapter::new(SittlConfig::default());
        assert!(adapter.core.is_heartbeat_stale(1000));
    }
}
