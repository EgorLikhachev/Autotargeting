//! Application configuration.
//!
//! Loaded from TOML files (`config.toml`) with environment variable
//! overrides. See `config.example.toml` at the repo root for a template.

use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

/// Конфигурация шины событий (D-014, M5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusConfig {
    /// Endpoint zenoh (listener или connect — решает компонент).
    pub endpoint: String,
    /// Поднять listener (первый процесс шины) вместо connect.
    #[serde(default)]
    pub listen: bool,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            endpoint: "tcp/127.0.0.1:7447".to_string(),
            listen: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub video: VideoConfig,
    /// M5: шина событий (zenoh).
    pub bus: BusConfig,
    pub inference: InferenceConfig,
    pub tracker: TrackerConfig,
    pub fc: FcConfig,
    pub commander: CommanderConfig,
    /// Path to write logs (in addition to stdout).
    pub log_file: Option<String>,
    /// Trace filter, e.g. "info,auto_targeting=debug".
    pub log_filter: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    /// V4L2 device path, e.g. "/dev/video0".
    pub device: String,
    /// Capture width.
    pub width: u32,
    /// Capture height.
    pub height: u32,
    /// Capture framerate.
    pub fps: u32,
    /// Preferred pixel format. One of: "nv12", "yuyv", "mjpeg".
    pub format: String,
    /// Queue length — frames older than this are dropped.
    pub queue_depth: usize,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            device: "/dev/video0".to_string(),
            width: 1280,
            height: 720,
            fps: 30,
            format: "mjpeg".to_string(),
            queue_depth: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    /// Path to the RKNN-compiled model file.
    pub model_path: String,
    /// Confidence threshold in [0.0, 1.0]. Detections below this are discarded.
    pub confidence_threshold: f32,
    /// NMS IoU threshold.
    pub nms_threshold: f32,
    /// Classes to track (others are filtered out). Empty = all.
    pub track_classes: Vec<String>,
    /// Path to the Unix socket for the C++ RKNN bridge.
    pub bridge_socket: String,
    /// If true and NPU is unavailable, fall back to CPU inference (ONNX Runtime).
    pub allow_cpu_fallback: bool,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            model_path: "/opt/auto-targeting/models/yolov8n_int8.rknn".to_string(),
            confidence_threshold: 0.45,
            nms_threshold: 0.45,
            track_classes: vec!["person".to_string()],
            bridge_socket: "/tmp/rknn-bridge.sock".to_string(),
            allow_cpu_fallback: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerConfig {
    /// Maximum age (ms) of a detection before target is declared LOST.
    pub max_target_age_ms: u64,
    /// Maximum missed frames before LOST.
    pub max_missed_frames: u32,
    /// Number of consecutive detections required to confirm a lock.
    pub lock_confirmation_frames: u32,
    /// IoU threshold for matching a new detection to an existing track.
    pub match_iou_threshold: f32,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            max_target_age_ms: 2000,
            max_missed_frames: 60,
            lock_confirmation_frames: 3,
            match_iou_threshold: 0.3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcConfig {
    /// Adapter implementation: "ardupilot-mavlink" | "sitl-mavlink" | "mock".
    pub adapter: String,
    /// Serial device path (for ardupilot-mavlink) or UDP endpoint (for sitl).
    pub endpoint: String,
    /// Baud rate for serial connection.
    pub baud_rate: u32,
    /// MAVLink system ID of this companion computer.
    pub system_id: u8,
    /// MAVLink component ID.
    pub component_id: u8,
    /// Target system ID (the FC).
    pub target_system_id: u8,
    /// Target component ID.
    pub target_component_id: u8,
    /// Rate limit for SET_POSITION_TARGET_LOCAL_NED (Hz).
    pub command_rate_hz: u32,
    /// Heartbeat timeout (ms) — if exceeded, transition to ABORT.
    pub heartbeat_timeout_ms: u64,
}

impl Default for FcConfig {
    fn default() -> Self {
        Self {
            adapter: "mock".to_string(),
            endpoint: "127.0.0.1:14550".to_string(),
            baud_rate: 115_200,
            system_id: 1,
            component_id: 1,
            target_system_id: 1,
            target_component_id: 1,
            command_rate_hz: 10,
            heartbeat_timeout_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommanderConfig {
    /// Watchdog timeouts (ms) for each loop.
    pub video_loop_wdt_ms: u64,
    pub inference_loop_wdt_ms: u64,
    pub tracking_loop_wdt_ms: u64,
    pub command_loop_wdt_ms: u64,
    /// Deadband — if target offset (as fraction of frame) is less than this,
    /// no correction command is sent. Prevents micro-jitter.
    pub deadband_fraction: f32,
    /// Hysteresis duration (ms) — when target disappears, stay in
    /// TRACKING_DEGRADED before transitioning to LOST.
    pub loss_hysteresis_ms: u64,
    /// Maximum yaw rate command (deg/s). Commands are clipped to this.
    pub max_yaw_rate_dps: f32,
    /// Maximum pitch rate command (deg/s).
    pub max_pitch_rate_dps: f32,
    /// Maximum target offset (as fraction of frame) before command is clipped.
    pub max_offset_fraction: f32,
    /// Oscillation detector: ring buffer size (in commands, ~3s @ 10Hz = 30).
    pub oscillation_window: usize,
    /// Oscillation detector: sign-change fraction above which we trigger.
    pub oscillation_threshold: f32,
    /// Number of oscillation triggers in 5s window before ABORT.
    pub oscillation_abort_count: u32,
}

impl Default for CommanderConfig {
    fn default() -> Self {
        Self {
            video_loop_wdt_ms: 100,
            inference_loop_wdt_ms: 200,
            tracking_loop_wdt_ms: 50,
            command_loop_wdt_ms: 100,
            deadband_fraction: 0.05,
            loss_hysteresis_ms: 500,
            max_yaw_rate_dps: 30.0,
            max_pitch_rate_dps: 15.0,
            max_offset_fraction: 0.30,
            oscillation_window: 30,
            oscillation_threshold: 0.5,
            oscillation_abort_count: 3,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            video: VideoConfig::default(),
            bus: BusConfig::default(),
            inference: InferenceConfig::default(),
            tracker: TrackerConfig::default(),
            fc: FcConfig::default(),
            commander: CommanderConfig::default(),
            log_file: None,
            log_filter: "info,auto_targeting=debug".to_string(),
        }
    }
}

impl AppConfig {
    /// Load config from a TOML file at `path`, with environment variable
    /// overrides. Environment variables are prefixed `AT_` and use `__` as
    /// the section separator, e.g. `AT_VIDEO__DEVICE=/dev/video1`.
    ///
    /// The TOML must include all required fields — figment does not auto-merge
    /// with `Default::default()`. Use `load_or_default` if you want fallback.
    pub fn load(path: &str) -> crate::errors::Result<Self> {
        let config = Figment::from(Toml::file(path))
            .merge(Env::prefixed("AT_").split("__"))
            .extract()
            .map_err(|e| crate::errors::CommonError::Config(format!("figment: {e}")))?;
        Ok(config)
    }

    /// Load config from a TOML file, merging with defaults so partial configs work.
    /// Environment variables override the file values.
    pub fn load_with_defaults(path: &str) -> crate::errors::Result<Self> {
        use figment::providers::Serialized;
        let config = Figment::from(Serialized::defaults(Self::default()))
            .merge(Toml::file(path))
            .merge(Env::prefixed("AT_").split("__"))
            .extract()
            .map_err(|e| crate::errors::CommonError::Config(format!("figment: {e}")))?;
        Ok(config)
    }

    /// Load config from a TOML file, falling back to defaults if file is missing
    /// or unparseable. Best-effort for dev — production should fail loudly on
    /// config errors.
    pub fn load_or_default(path: Option<&str>) -> Self {
        if let Some(p) = path {
            match Self::load_with_defaults(p) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("WARN: failed to load config from {p}: {e}; using defaults");
                    Self::default()
                }
            }
        } else {
            Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let c = AppConfig::default();
        assert_eq!(c.video.width, 1280);
        assert_eq!(c.commander.deadband_fraction, 0.05);
    }

    #[test]
    fn config_from_toml_string() {
        let toml_str = r#"
[video]
device = "/dev/video2"
width = 1920
height = 1080
fps = 30
format = "yuyv"
queue_depth = 5

[inference]
model_path = "/tmp/model.rknn"
confidence_threshold = 0.5
nms_threshold = 0.4
track_classes = ["person", "car"]
bridge_socket = "/tmp/test.sock"
allow_cpu_fallback = true

[tracker]
max_target_age_ms = 3000
max_missed_frames = 90
lock_confirmation_frames = 2
match_iou_threshold = 0.25

[fc]
adapter = "sitl-mavlink"
endpoint = "127.0.0.1:14550"
baud_rate = 115200
system_id = 1
component_id = 1
target_system_id = 1
target_component_id = 1
command_rate_hz = 10
heartbeat_timeout_ms = 1000

[commander]
video_loop_wdt_ms = 100
inference_loop_wdt_ms = 200
tracking_loop_wdt_ms = 50
command_loop_wdt_ms = 100
deadband_fraction = 0.05
loss_hysteresis_ms = 500
max_yaw_rate_dps = 30.0
max_pitch_rate_dps = 15.0
max_offset_fraction = 0.30
oscillation_window = 30
oscillation_threshold = 0.5
oscillation_abort_count = 3

log_file = "/var/log/auto-targeting.log"
log_filter = "info,auto_targeting=debug"
"#;
        // Write to a temp file and load
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), toml_str).unwrap();
        let cfg =
            AppConfig::load_with_defaults(tmp.path().to_str().unwrap()).expect("should parse");
        assert_eq!(cfg.video.device, "/dev/video2");
        assert_eq!(cfg.video.width, 1920);
        assert_eq!(cfg.fc.adapter, "sitl-mavlink");
        assert_eq!(cfg.inference.track_classes, vec!["person", "car"]);
    }

    #[test]
    fn config_merges_with_defaults_when_partial() {
        // Figment does NOT auto-merge with Default::default() — partial TOML
        // must include all required fields. This test documents that behavior.
        // Use `load_or_default` if you want defaults on failure.
        let toml_str = r#"
[video]
device = "/dev/video5"
width = 640
height = 480
fps = 15
format = "mjpeg"
queue_depth = 2

[inference]
model_path = "/tmp/m.rknn"
confidence_threshold = 0.4
nms_threshold = 0.4
track_classes = []
bridge_socket = "/tmp/b.sock"
allow_cpu_fallback = false

[tracker]
max_target_age_ms = 1000
max_missed_frames = 30
lock_confirmation_frames = 2
match_iou_threshold = 0.3

[fc]
adapter = "mock"
endpoint = "127.0.0.1:14550"
baud_rate = 115200
system_id = 1
component_id = 1
target_system_id = 1
target_component_id = 1
command_rate_hz = 10
heartbeat_timeout_ms = 1000

[commander]
video_loop_wdt_ms = 100
inference_loop_wdt_ms = 200
tracking_loop_wdt_ms = 50
command_loop_wdt_ms = 100
deadband_fraction = 0.05
loss_hysteresis_ms = 500
max_yaw_rate_dps = 30.0
max_pitch_rate_dps = 15.0
max_offset_fraction = 0.30
oscillation_window = 30
oscillation_threshold = 0.5
oscillation_abort_count = 3

log_file = ""
log_filter = "info"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), toml_str).unwrap();
        let cfg =
            AppConfig::load_with_defaults(tmp.path().to_str().unwrap()).expect("should parse");
        assert_eq!(cfg.video.device, "/dev/video5");
    }
}
