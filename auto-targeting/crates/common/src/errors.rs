//! Error types shared across the workspace.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CommonError>;

/// Top-level error type for the auto-targeting system.
///
/// Each module may have its own more specific error type, but they all
/// convert into `CommonError` for cross-module propagation.
#[derive(Debug, Error)]
pub enum CommonError {
    #[error("video capture error: {0}")]
    Video(String),

    #[error("inference error: {0}")]
    Inference(String),

    #[error("tracking error: {0}")]
    Tracking(String),

    #[error("flight controller error: {0}")]
    Fc(String),

    #[error("commander error: {0}")]
    Commander(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("toml deserialize error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("channel send error: {0}")]
    ChannelSend(String),

    #[error("watchdog timeout: {watchdog} exceeded {elapsed_ms}ms (limit: {limit_ms}ms)")]
    Watchdog {
        watchdog: &'static str,
        elapsed_ms: u64,
        limit_ms: u64,
    },

    #[error("invalid state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("safety violation: {0}")]
    Safety(String),

    #[error("other: {0}")]
    Other(String),
}

impl CommonError {
    /// Returns true if this error indicates a safety-critical condition
    /// that should trigger an ABORT state.
    pub fn is_safety_critical(&self) -> bool {
        matches!(
            self,
            Self::Watchdog { .. } | Self::Safety(_) | Self::InvalidStateTransition { .. }
        )
    }
}
