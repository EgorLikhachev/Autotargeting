//! CLI argument parsing.

use clap::{ArgAction, Args, Parser, Subcommand};
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
    #[arg(short, long, env = "AT_CONFIG_PATH", global = true)]
    pub config: Option<PathBuf>,

    /// Use a mock Flight Controller (in-memory). No real FC connection.
    /// Useful for testing without hardware.
    #[arg(long, action = ArgAction::SetTrue, global = true)]
    pub mock_fc: bool,

    /// Use mock everything (synthetic video, mock inference, mock FC).
    /// The Phase 0 smoke test.
    #[arg(long, action = ArgAction::SetTrue, global = true)]
    pub mock_all: bool,

    /// Print health status and exit. Used by systemd / healthcheck scripts.
    #[arg(long, action = ArgAction::SetTrue, global = true)]
    pub health_check: bool,

    /// Start the interactive REPL (operator console).
    /// Requires --mock-fc or --mock-all (real FC not yet supported in REPL).
    #[arg(long, action = ArgAction::SetTrue, global = true)]
    pub repl: bool,

    /// Increase verbosity. Can be repeated: -v, -vv, -vvv.
    #[arg(short, long, action = ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Subcommand (scenario runner, etc.).
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Run a SITL test scenario from a JSON file.
    Scenario {
        #[command(flatten)]
        scenario_args: ScenarioArgs,
    },
    /// M5: монитор шины (подписка at/**, pretty-print).
    BusMon {
        /// Маска тем zenoh.
        #[arg(long, default_value = "at/**")]
        topics: String,
        /// Свернуть payload до N символов.
        #[arg(long, default_value_t = 400)]
        max_len: usize,
    },
    /// M5: операторский REPL через шину.
    ReplBus,
    /// M5: конфиг-сервис — отвечать на запросы at/config.
    ConfigSvc,
    /// M5: получить конфиг (at/config) и напечатать.
    ConfigGet,
}

#[derive(Debug, Clone, Args)]
pub struct ScenarioArgs {
    /// Path to the scenario JSON file, OR --all to run all scenarios in a directory.
    #[arg(group = "target")]
    pub file: Option<PathBuf>,

    /// Run all scenarios in the given directory.
    #[arg(long, group = "target")]
    pub all: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Full,
    MockFc,
    MockAll,
    HealthCheck,
    Repl,
    Scenario,
    /// M5: монитор шины at/**.
    BusMon,
    /// M5: REPL через шину (команды FC + живые данные).
    ReplBus,
    /// M5: конфиг-сервис (queryable at/config).
    ConfigSvc,
    /// M5: запрос конфига (get at/config).
    ConfigGet,
}

impl CliArgs {
    pub fn mode(&self) -> RunMode {
        match &self.command {
            Some(Command::BusMon { .. }) => return RunMode::BusMon,
            Some(Command::ReplBus) => return RunMode::ReplBus,
            Some(Command::ConfigSvc) => return RunMode::ConfigSvc,
            Some(Command::ConfigGet) => return RunMode::ConfigGet,
            _ => {}
        }
        if self.command.is_some() {
            RunMode::Scenario
        } else if self.repl {
            RunMode::Repl
        } else if self.health_check {
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
