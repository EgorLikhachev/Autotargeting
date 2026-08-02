//! Safety monitors — geofencing + battery monitoring.
//!
//! Эти модули проверяют телеметрию FC и автоматически триггерят RTH,
//! если дрон:
//! - Вышел за geofence (максимальное расстояние от home)
//! - У него низкий заряд батареи
//! - Потеряна связь с GCS (опционально)
//!
//! ## Использование
//!
//! ```ignore
//! use commander::safety::{SafetyMonitor, SafetyConfig};
//!
//! let mut monitor = SafetyMonitor::new(SafetyConfig::default());
//! monitor.update_position(lat, lon, alt);
//! monitor.update_battery(voltage, remaining_pct);
//!
//! if let Some(reason) = monitor.check_violations() {
//!     commander.abort().await?; // auto-RTH
//! }
//! ```

use common::GlobalPosition;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// ============================================================
// GEOFENCING
// ============================================================

/// Конфигурация geofence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeofenceConfig {
    /// Максимальное расстояние от home точки (метры).
    pub max_distance_m: f32,
    /// Максимальная высота над home (метры).
    pub max_altitude_m: f32,
    /// Минимальная высота над home (метры). Отрицательная = ниже home.
    pub min_altitude_m: f32,
}

impl Default for GeofenceConfig {
    fn default() -> Self {
        Self {
            max_distance_m: 500.0, // 500m radius
            max_altitude_m: 120.0, // 120m AGL (legal limit in many countries)
            min_altitude_m: -10.0, // не ниже 10m от home
        }
    }
}

/// Тип нарушения geofence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeofenceViolation {
    DistanceExceeded,
    AltitudeMaxExceeded,
    AltitudeMinExceeded,
}

impl std::fmt::Display for GeofenceViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DistanceExceeded => write!(f, "distance exceeded"),
            Self::AltitudeMaxExceeded => write!(f, "max altitude exceeded"),
            Self::AltitudeMinExceeded => write!(f, "min altitude exceeded"),
        }
    }
}

/// Geofence monitor — проверяет позицию дрона.
pub struct Geofence {
    config: GeofenceConfig,
    /// Home позиция (устанавливается при arm).
    home: Option<(f64, f64, f32)>, // (lat, lon, alt_msl)
    /// Последняя известная позиция.
    last_position: Option<GlobalPosition>,
}

impl Geofence {
    pub fn new(config: GeofenceConfig) -> Self {
        Self {
            config,
            home: None,
            last_position: None,
        }
    }

    /// Установить home позицию (при arm или первой фиксации GPS).
    pub fn set_home(&mut self, lat: f64, lon: f64, alt_msl: f32) {
        self.home = Some((lat, lon, alt_msl));
        info!(lat, lon, alt_msl, "geofence home set");
    }

    /// Обновить текущую позицию.
    pub fn update_position(&mut self, pos: GlobalPosition) {
        self.last_position = Some(pos);
    }

    /// Проверить нарушения geofence.
    /// Возвращает Some(violation) если есть нарушение, иначе None.
    pub fn check(&self) -> Option<GeofenceViolation> {
        let (home_lat, home_lon, home_alt) = self.home?;
        let pos = self.last_position?;

        // Расстояние от home (haversine formula)
        let distance = haversine_distance(home_lat, home_lon, pos.lat, pos.lon);
        if distance > self.config.max_distance_m {
            warn!(
                distance,
                max = self.config.max_distance_m,
                "geofence violation: distance exceeded"
            );
            return Some(GeofenceViolation::DistanceExceeded);
        }

        // Высота
        let alt_agl = pos.alt_msl - home_alt;
        if alt_agl > self.config.max_altitude_m {
            warn!(
                alt_agl,
                max = self.config.max_altitude_m,
                "geofence violation: max altitude exceeded"
            );
            return Some(GeofenceViolation::AltitudeMaxExceeded);
        }
        if alt_agl < self.config.min_altitude_m {
            warn!(
                alt_agl,
                min = self.config.min_altitude_m,
                "geofence violation: min altitude exceeded"
            );
            return Some(GeofenceViolation::AltitudeMinExceeded);
        }

        None
    }

    /// Получить текущее расстояние от home (метры).
    pub fn distance_from_home(&self) -> Option<f32> {
        let (home_lat, home_lon, _) = self.home?;
        let pos = self.last_position?;
        Some(haversine_distance(home_lat, home_lon, pos.lat, pos.lon))
    }

    /// Home установлен?
    pub fn has_home(&self) -> bool {
        self.home.is_some()
    }
}

/// Haversine формула для расстояния между двумя GPS точками (метры).
fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f32 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;

    let lat1_rad = lat1.to_radians();
    let lat2_rad = lat2.to_radians();
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();

    let a =
        (dlat / 2.0).sin().powi(2) + lat1_rad.cos() * lat2_rad.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();

    (EARTH_RADIUS_M * c) as f32
}

// ============================================================
// BATTERY MONITORING
// ============================================================

/// Конфигурация battery monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryConfig {
    /// Порог RTH — если заряд ниже, триггерим RTL.
    pub rth_threshold_pct: f32,
    /// Порог LAND — если заряд ниже, триггерим LAND (критический).
    pub land_threshold_pct: f32,
    /// Минимальное напряжение батареи (V) — для redundancy.
    pub min_voltage: f32,
}

impl Default for BatteryConfig {
    fn default() -> Self {
        Self {
            rth_threshold_pct: 30.0,  // 30% → RTH
            land_threshold_pct: 15.0, // 15% → LAND
            min_voltage: 10.5,        // 3S LiPo: 3.5V/cell
        }
    }
}

/// Тип нарушения батареи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryViolation {
    LowBatteryRth,
    CriticalBatteryLand,
    LowVoltage,
}

impl std::fmt::Display for BatteryViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LowBatteryRth => write!(f, "low battery — RTH triggered"),
            Self::CriticalBatteryLand => write!(f, "critical battery — LAND triggered"),
            Self::LowVoltage => write!(f, "low voltage"),
        }
    }
}

/// Battery monitor.
pub struct BatteryMonitor {
    config: BatteryConfig,
    /// Последнее известное состояние батареи.
    last_battery: Option<BatteryState>,
}

#[derive(Debug, Clone, Copy)]
pub struct BatteryState {
    pub voltage: f32,
    pub remaining_pct: f32,
}

impl BatteryMonitor {
    pub fn new(config: BatteryConfig) -> Self {
        Self {
            config,
            last_battery: None,
        }
    }

    /// Обновить состояние батареи.
    pub fn update(&mut self, voltage: f32, remaining_pct: f32) {
        self.last_battery = Some(BatteryState {
            voltage,
            remaining_pct,
        });
    }

    /// Проверить нарушения.
    pub fn check(&self) -> Option<BatteryViolation> {
        let bat = self.last_battery?;

        if bat.remaining_pct <= self.config.land_threshold_pct {
            warn!(
                remaining_pct = bat.remaining_pct,
                threshold = self.config.land_threshold_pct,
                "critical battery — LAND"
            );
            return Some(BatteryViolation::CriticalBatteryLand);
        }

        if bat.remaining_pct <= self.config.rth_threshold_pct {
            warn!(
                remaining_pct = bat.remaining_pct,
                threshold = self.config.rth_threshold_pct,
                "low battery — RTH"
            );
            return Some(BatteryViolation::LowBatteryRth);
        }

        if bat.voltage > 0.0 && bat.voltage <= self.config.min_voltage {
            warn!(
                voltage = bat.voltage,
                min = self.config.min_voltage,
                "low voltage"
            );
            return Some(BatteryViolation::LowVoltage);
        }

        None
    }

    /// Получить текущее состояние батареи.
    pub fn state(&self) -> Option<BatteryState> {
        self.last_battery
    }
}

// ============================================================
// SAFETY MONITOR (комбинированный)
// ============================================================

/// Конфигурация safety monitor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SafetyConfig {
    pub geofence: GeofenceConfig,
    pub battery: BatteryConfig,
    /// Включён ли geofence.
    pub enable_geofence: bool,
    /// Включён ли battery monitor.
    pub enable_battery: bool,
}

/// Safety monitor — объединяет geofence + battery.
pub struct SafetyMonitor {
    config: SafetyConfig,
    geofence: Geofence,
    battery: BatteryMonitor,
}

/// Тип safety violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyViolation {
    Geofence(GeofenceViolation),
    Battery(BatteryViolation),
}

impl std::fmt::Display for SafetyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Geofence(v) => write!(f, "geofence: {v}"),
            Self::Battery(v) => write!(f, "battery: {v}"),
        }
    }
}

/// Действие, которое нужно выполнить при violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyAction {
    /// Перейти в RTL (Return To Home).
    Rth,
    /// Перейти в LAND (немедленная посадка).
    Land,
    /// Только предупредить, без действия.
    Warn,
}

impl SafetyMonitor {
    pub fn new(config: SafetyConfig) -> Self {
        Self {
            geofence: Geofence::new(config.geofence.clone()),
            battery: BatteryMonitor::new(config.battery.clone()),
            config,
        }
    }

    pub fn set_home(&mut self, lat: f64, lon: f64, alt_msl: f32) {
        self.geofence.set_home(lat, lon, alt_msl);
    }

    pub fn update_position(&mut self, pos: GlobalPosition) {
        if self.config.enable_geofence {
            self.geofence.update_position(pos);
        }
    }

    pub fn update_battery(&mut self, voltage: f32, remaining_pct: f32) {
        if self.config.enable_battery {
            self.battery.update(voltage, remaining_pct);
        }
    }

    /// Проверить все safety conditions.
    /// Возвращает (violation, action) если есть нарушение.
    pub fn check(&self) -> Option<(SafetyViolation, SafetyAction)> {
        if self.config.enable_battery {
            if let Some(v) = self.battery.check() {
                let action = match v {
                    BatteryViolation::CriticalBatteryLand => SafetyAction::Land,
                    BatteryViolation::LowBatteryRth | BatteryViolation::LowVoltage => {
                        SafetyAction::Rth
                    }
                };
                return Some((SafetyViolation::Battery(v), action));
            }
        }

        if self.config.enable_geofence && self.geofence.has_home() {
            if let Some(v) = self.geofence.check() {
                return Some((SafetyViolation::Geofence(v), SafetyAction::Rth));
            }
        }

        None
    }

    /// Получить расстояние от home.
    pub fn distance_from_home(&self) -> Option<f32> {
        self.geofence.distance_from_home()
    }

    /// Получить состояние батареи.
    pub fn battery_state(&self) -> Option<BatteryState> {
        self.battery.state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Geofence tests ===

    #[test]
    fn geofence_no_home_no_violation() {
        let gf = Geofence::new(GeofenceConfig::default());
        assert!(gf.check().is_none());
        assert!(!gf.has_home());
    }

    #[test]
    fn geofence_home_set_no_position_no_violation() {
        let mut gf = Geofence::new(GeofenceConfig::default());
        gf.set_home(55.0, 37.0, 100.0);
        assert!(gf.has_home());
        assert!(gf.check().is_none());
    }

    #[test]
    fn geofence_within_bounds_no_violation() {
        let mut gf = Geofence::new(GeofenceConfig::default());
        gf.set_home(55.0, 37.0, 100.0);
        gf.update_position(GlobalPosition {
            lat: 55.0001, // ~11 meters away
            lon: 37.0001,
            alt_msl: 110.0,
            alt_agl: 10.0,
        });
        assert!(gf.check().is_none());
    }

    #[test]
    fn geofence_distance_exceeded() {
        let mut gf = Geofence::new(GeofenceConfig {
            max_distance_m: 100.0,
            ..Default::default()
        });
        gf.set_home(55.0, 37.0, 100.0);
        gf.update_position(GlobalPosition {
            lat: 55.001, // ~111 meters away (0.001° ≈ 111m)
            lon: 37.0,
            alt_msl: 110.0,
            alt_agl: 10.0,
        });
        let v = gf.check().unwrap();
        assert_eq!(v, GeofenceViolation::DistanceExceeded);
    }

    #[test]
    fn geofence_max_altitude_exceeded() {
        let mut gf = Geofence::new(GeofenceConfig {
            max_altitude_m: 50.0,
            ..Default::default()
        });
        gf.set_home(55.0, 37.0, 100.0);
        gf.update_position(GlobalPosition {
            lat: 55.0,
            lon: 37.0,
            alt_msl: 200.0, // 100m AGL
            alt_agl: 100.0,
        });
        let v = gf.check().unwrap();
        assert_eq!(v, GeofenceViolation::AltitudeMaxExceeded);
    }

    #[test]
    fn geofence_min_altitude_exceeded() {
        let mut gf = Geofence::new(GeofenceConfig {
            min_altitude_m: -5.0,
            ..Default::default()
        });
        gf.set_home(55.0, 37.0, 100.0);
        gf.update_position(GlobalPosition {
            lat: 55.0,
            lon: 37.0,
            alt_msl: 90.0, // -10m AGL (below home)
            alt_agl: -10.0,
        });
        let v = gf.check().unwrap();
        assert_eq!(v, GeofenceViolation::AltitudeMinExceeded);
    }

    #[test]
    fn haversine_distance_zero() {
        let d = haversine_distance(55.0, 37.0, 55.0, 37.0);
        assert!(d < 0.1);
    }

    #[test]
    fn haversine_distance_known() {
        // Москва → Санкт-Петербург: ~634 km
        let d = haversine_distance(55.7558, 37.6173, 59.9343, 30.3351);
        assert!((d - 634_000.0).abs() < 5_000.0, "got {d}"); // ±5km tolerance
    }

    #[test]
    fn geofence_distance_from_home() {
        let mut gf = Geofence::new(GeofenceConfig::default());
        gf.set_home(55.0, 37.0, 100.0);
        gf.update_position(GlobalPosition {
            lat: 55.001,
            lon: 37.0,
            alt_msl: 110.0,
            alt_agl: 10.0,
        });
        let d = gf.distance_from_home().unwrap();
        assert!(d > 100.0 && d < 120.0, "got {d}");
    }

    // === Battery tests ===

    #[test]
    fn battery_no_data_no_violation() {
        let bm = BatteryMonitor::new(BatteryConfig::default());
        assert!(bm.check().is_none());
    }

    #[test]
    fn battery_full_no_violation() {
        let mut bm = BatteryMonitor::new(BatteryConfig::default());
        bm.update(12.6, 95.0);
        assert!(bm.check().is_none());
    }

    #[test]
    fn battery_low_triggers_rth() {
        let mut bm = BatteryMonitor::new(BatteryConfig::default());
        bm.update(11.5, 25.0); // 25% < 30% threshold
        let v = bm.check().unwrap();
        assert_eq!(v, BatteryViolation::LowBatteryRth);
    }

    #[test]
    fn battery_critical_triggers_land() {
        let mut bm = BatteryMonitor::new(BatteryConfig::default());
        bm.update(10.0, 10.0); // 10% < 15% threshold
        let v = bm.check().unwrap();
        assert_eq!(v, BatteryViolation::CriticalBatteryLand);
    }

    #[test]
    fn battery_low_voltage() {
        let mut bm = BatteryMonitor::new(BatteryConfig {
            min_voltage: 11.0,
            rth_threshold_pct: 0.0, // disable % check
            land_threshold_pct: 0.0,
        });
        bm.update(10.5, 50.0); // voltage low, % OK
        let v = bm.check().unwrap();
        assert_eq!(v, BatteryViolation::LowVoltage);
    }

    #[test]
    fn battery_land_takes_priority_over_rth() {
        let mut bm = BatteryMonitor::new(BatteryConfig::default());
        bm.update(9.0, 5.0); // both critical % and low voltage
        let v = bm.check().unwrap();
        // LAND takes priority
        assert_eq!(v, BatteryViolation::CriticalBatteryLand);
    }

    // === SafetyMonitor tests ===

    #[test]
    fn safety_monitor_no_violations() {
        let mut sm = SafetyMonitor::new(SafetyConfig {
            enable_geofence: true,
            enable_battery: true,
            ..Default::default()
        });
        sm.set_home(55.0, 37.0, 100.0);
        sm.update_position(GlobalPosition {
            lat: 55.0001,
            lon: 37.0,
            alt_msl: 110.0,
            alt_agl: 10.0,
        });
        sm.update_battery(12.6, 95.0);
        assert!(sm.check().is_none());
    }

    #[test]
    fn safety_monitor_battery_violation() {
        let mut sm = SafetyMonitor::new(SafetyConfig {
            enable_geofence: true,
            enable_battery: true,
            ..Default::default()
        });
        sm.set_home(55.0, 37.0, 100.0);
        sm.update_position(GlobalPosition {
            lat: 55.0,
            lon: 37.0,
            alt_msl: 100.0,
            alt_agl: 0.0,
        });
        sm.update_battery(11.5, 25.0); // low battery

        let (v, action) = sm.check().unwrap();
        assert!(matches!(v, SafetyViolation::Battery(_)));
        assert_eq!(action, SafetyAction::Rth);
    }

    #[test]
    fn safety_monitor_geofence_violation() {
        let mut sm = SafetyMonitor::new(SafetyConfig {
            enable_geofence: true,
            enable_battery: true,
            ..Default::default()
        });
        sm.set_home(55.0, 37.0, 100.0);
        sm.update_position(GlobalPosition {
            lat: 55.01, // ~1.1 km away, > 500m default
            lon: 37.0,
            alt_msl: 100.0,
            alt_agl: 0.0,
        });
        sm.update_battery(12.6, 95.0);

        let (v, action) = sm.check().unwrap();
        assert!(matches!(v, SafetyViolation::Geofence(_)));
        assert_eq!(action, SafetyAction::Rth);
    }

    #[test]
    fn safety_monitor_land_action_for_critical_battery() {
        let mut sm = SafetyMonitor::new(SafetyConfig {
            enable_geofence: true,
            enable_battery: true,
            ..Default::default()
        });
        sm.set_home(55.0, 37.0, 100.0);
        sm.update_position(GlobalPosition {
            lat: 55.0,
            lon: 37.0,
            alt_msl: 100.0,
            alt_agl: 0.0,
        });
        sm.update_battery(9.0, 10.0); // critical

        let (v, action) = sm.check().unwrap();
        assert!(matches!(v, SafetyViolation::Battery(_)));
        assert_eq!(action, SafetyAction::Land);
    }

    #[test]
    fn safety_monitor_disabled_features() {
        let mut sm = SafetyMonitor::new(SafetyConfig {
            enable_geofence: false,
            enable_battery: false,
            ..Default::default()
        });
        sm.set_home(55.0, 37.0, 100.0);
        sm.update_position(GlobalPosition {
            lat: 55.01,
            lon: 37.0,
            alt_msl: 100.0,
            alt_agl: 0.0,
        });
        sm.update_battery(9.0, 10.0); // critical, but disabled
        assert!(sm.check().is_none());
    }

    #[test]
    fn safety_monitor_battery_takes_priority_over_geofence() {
        let mut sm = SafetyMonitor::new(SafetyConfig {
            enable_geofence: true,
            enable_battery: true,
            ..Default::default()
        });
        sm.set_home(55.0, 37.0, 100.0);
        sm.update_position(GlobalPosition {
            lat: 55.01, // geofence violation
            lon: 37.0,
            alt_msl: 100.0,
            alt_agl: 0.0,
        });
        sm.update_battery(9.0, 10.0); // critical battery

        let (v, action) = sm.check().unwrap();
        // Battery (LAND) takes priority over geofence (RTH)
        assert!(matches!(v, SafetyViolation::Battery(_)));
        assert_eq!(action, SafetyAction::Land);
    }

    #[test]
    fn safety_violation_display() {
        let v = SafetyViolation::Geofence(GeofenceViolation::DistanceExceeded);
        let s = format!("{v}");
        assert!(s.contains("geofence"));
        assert!(s.contains("distance exceeded"));
    }
}
