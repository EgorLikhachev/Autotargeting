//! CLI argument parsing.

use clap::{ArgAction, Parser};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "auto-targeting",
    version,
    about = "Auto-Targeting System companion computer",
    long_about = "Runs the full auto-targeting pipeline. Use --mock-fc / --mock-all for testing without hardware."
)]
pub struct CliArgs {
    /// Path to the configuration TOML file.
    /// If not provided, defaults are used.
    #[arg(short, long, env = "AT_CONFIG_PATH")]
    pub config: Option<PathBuf>,

    /// Use a mock Flight Controller (in-memory). No real FC connection.
    /// Useful for testing without hardware.
    #[arg(long, action = ArgAction::SetTrue)]
    pub mock_fc: bool,

    /// Use mock everything (synthetic video, mock inference, mock FC).
    /// The Phase 0 smoke test.
    #[arg(long, action = ArgAction::SetTrue)]
    pub mock_all: bool,

    /// Print health status and exit. Used by systemd / healthcheck scripts.
    #[arg(long, action = ArgAction::SetTrue)]
    pub health_check: bool,

    /// Increase verbosity. Can be repeated: -v, -vv, -vvv.
    #[arg(short, long, action = ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Full,
    MockFc,
    MockAll,
    HealthCheck,
}

impl CliArgs {
    pub fn mode(&self) -> RunMode {
        if self.health_check {
            RunMode::HealthCheck
        } else if self.mock_all {
            RunMode::MockAll
        } else if self.mock_fc {
            RunMode::MockFc
        } else {
            RunMode::Full
        }
    }
}
