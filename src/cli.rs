//! Command-line interface.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::errors::Result;
use crate::monitoring::Thresholds;
use crate::monitoring::thresholds::{DEFAULT_CRITICAL_C, DEFAULT_ELEVATED_C, DEFAULT_HIGH_C};
use crate::time::Zone;

/// Smallest accepted refresh interval in milliseconds.
pub const MIN_INTERVAL_MS: u64 = 100;
/// Largest accepted refresh interval in milliseconds (1 hour).
pub const MAX_INTERVAL_MS: u64 = 3_600_000;

/// Terminal-only CPU temperature monitor.
///
/// Without options, RustTemp continuously displays every discovered
/// temperature sensor with current, minimum, average and maximum values until
/// Ctrl+C is pressed.
#[derive(Debug, Parser)]
#[command(
    name = "rusttemp",
    version,
    about,
    long_about,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// `rusttemp cpu` / `rusttemp gpu`: the same reports as the `cpu` and
    /// `gpu` commands.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Take a single reading and exit.
    #[arg(long, conflicts_with_all = ["sensors", "count"])]
    pub once: bool,

    /// Show CPU information (manufacturer, model, cores, caches, clocks) and
    /// CPU health (throttling, thermal headroom), then exit.
    #[arg(long, conflicts_with_all = ["once", "sensors", "count"])]
    pub info: bool,

    /// List discovered sensors and their identifying information, then exit.
    #[arg(long, conflicts_with = "count")]
    pub sensors: bool,

    /// Refresh interval in milliseconds for continuous monitoring.
    #[arg(
        long,
        value_name = "MS",
        default_value_t = 1000,
        value_parser = clap::value_parser!(u64).range(MIN_INTERVAL_MS..=MAX_INTERVAL_MS),
        conflicts_with_all = ["once", "sensors", "info"]
    )]
    pub interval: u64,

    /// Stop continuous monitoring after N samples.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub count: Option<u64>,

    /// Print JSON: one document for --once / --sensors, one compact document
    /// per line (NDJSON) while monitoring.
    #[arg(long)]
    pub json: bool,

    /// Lower bound of the ELEVATED warning level in °C.
    #[arg(long, value_name = "CELSIUS", default_value_t = DEFAULT_ELEVATED_C, allow_negative_numbers = true)]
    pub elevated: f64,

    /// Lower bound of the HIGH warning level in °C.
    #[arg(long, value_name = "CELSIUS", default_value_t = DEFAULT_HIGH_C, allow_negative_numbers = true)]
    pub high: f64,

    /// Lower bound of the CRITICAL warning level in °C.
    #[arg(long, value_name = "CELSIUS", default_value_t = DEFAULT_CRITICAL_C, allow_negative_numbers = true)]
    pub critical: f64,

    /// Disable colored output (the NO_COLOR environment variable is also honored).
    #[arg(long)]
    pub no_color: bool,

    /// Show times in UTC instead of the local time zone.
    #[arg(long)]
    pub utc: bool,

    /// Read sensors from a Linux-style sysfs tree rooted at PATH instead of the
    /// platform default. Intended for diagnostics and testing.
    #[arg(long, value_name = "PATH")]
    pub sysfs_root: Option<PathBuf>,
}

/// Subcommands (identical to the standalone `cpu` and `gpu` executables).
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// CPU report: identification, temperatures, frequency, load, power,
    /// voltage, throttling and integrated GPU (same as `cpu`).
    Cpu(crate::commands::cpu::CpuArgs),
    /// Dedicated GPU report, each GPU separately (same as `gpu`).
    Gpu(crate::commands::gpu::GpuArgs),
}

/// What to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Continuous monitoring.
    Monitor {
        interval: Duration,
        count: Option<u64>,
    },
    /// One reading.
    Once,
    /// Sensor inventory.
    Sensors,
    /// CPU information and health.
    Info,
}

/// Validated configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub mode: Mode,
    pub json: bool,
    pub thresholds: Thresholds,
    pub no_color: bool,
    pub zone: Zone,
    pub sysfs_root: Option<PathBuf>,
}

impl Cli {
    /// Performs validation clap cannot express and builds a [`Config`].
    pub fn into_config(self) -> Result<Config> {
        let thresholds = Thresholds::new(self.elevated, self.high, self.critical)?;
        let mode = if self.info {
            Mode::Info
        } else if self.sensors {
            Mode::Sensors
        } else if self.once {
            Mode::Once
        } else {
            Mode::Monitor {
                interval: Duration::from_millis(self.interval),
                count: self.count,
            }
        };
        Ok(Config {
            mode,
            json: self.json,
            thresholds,
            no_color: self.no_color,
            zone: if self.utc { Zone::Utc } else { Zone::Local },
            sysfs_root: self.sysfs_root,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("rusttemp").chain(args.iter().copied()))
    }

    fn config(args: &[&str]) -> Config {
        parse(args).unwrap().into_config().unwrap()
    }

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn default_is_continuous_monitoring() {
        let c = config(&[]);
        assert_eq!(
            c.mode,
            Mode::Monitor {
                interval: Duration::from_millis(1000),
                count: None
            }
        );
        assert!(!c.json);
        assert_eq!(c.thresholds, Thresholds::default());
        assert_eq!(c.zone, Zone::Local);
        assert_eq!(config(&["--utc"]).zone, Zone::Utc);
    }

    #[test]
    fn documented_invocations() {
        assert_eq!(config(&["--once"]).mode, Mode::Once);
        assert_eq!(config(&["--sensors"]).mode, Mode::Sensors);
        assert_eq!(config(&["--info"]).mode, Mode::Info);
        assert!(config(&["--info", "--json"]).json);
        assert_eq!(
            config(&["--interval", "2000"]).mode,
            Mode::Monitor {
                interval: Duration::from_millis(2000),
                count: None
            }
        );
        assert!(config(&["--json"]).json);
        assert!(config(&["--once", "--json"]).json);
        assert_eq!(
            config(&["--count", "3"]).mode,
            Mode::Monitor {
                interval: Duration::from_millis(1000),
                count: Some(3)
            }
        );
    }

    #[test]
    fn cpu_and_gpu_subcommands() {
        assert!(matches!(
            parse(&["cpu"]).unwrap().command,
            Some(Command::Cpu(_))
        ));
        let c = parse(&["cpu", "--watch", "--interval", "2"]).unwrap();
        assert!(matches!(c.command, Some(Command::Cpu(a)) if a.watch && a.interval == 2.0));
        assert!(
            matches!(parse(&["gpu", "--all"]).unwrap().command, Some(Command::Gpu(a)) if a.all)
        );
        // Top-level rusttemp options and a subcommand do not mix.
        assert!(parse(&["--once", "cpu"]).is_err());
        assert!(parse(&[]).unwrap().command.is_none());
    }

    #[test]
    fn help_and_version() {
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["--version"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn invalid_arguments_are_rejected() {
        assert_eq!(
            parse(&["--interval", "abc"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse(&["--interval", "50"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse(&["--interval", "0"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse(&["--interval", "-5"]).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            parse(&["--interval"]).unwrap_err().kind(),
            ErrorKind::InvalidValue
        );
        assert_eq!(
            parse(&["--count", "0"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse(&["--bogus"]).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            parse(&["--once", "--sensors"]).unwrap_err().kind(),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            parse(&["--once", "--interval", "500"]).unwrap_err().kind(),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            parse(&["--once", "--count", "2"]).unwrap_err().kind(),
            ErrorKind::ArgumentConflict
        );
        for args in [
            &["--info", "--once"][..],
            &["--info", "--sensors"],
            &["--info", "--interval", "500"],
        ] {
            assert_eq!(
                parse(args).unwrap_err().kind(),
                ErrorKind::ArgumentConflict,
                "{args:?}"
            );
        }
    }

    #[test]
    fn interval_bounds() {
        assert!(parse(&["--interval", "100"]).is_ok());
        assert!(parse(&["--interval", "3600000"]).is_ok());
        assert!(parse(&["--interval", "3600001"]).is_err());
    }

    #[test]
    fn threshold_options() {
        let c = config(&["--elevated", "60", "--high", "75.5", "--critical", "90"]);
        assert_eq!(c.thresholds, Thresholds::new(60.0, 75.5, 90.0).unwrap());
        let c = config(&["--elevated", "-10", "--high", "0", "--critical", "10"]);
        assert_eq!(c.thresholds.elevated_c, -10.0);
        let err = parse(&["--elevated", "90"])
            .unwrap()
            .into_config()
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("elevated < high < critical"));
        let err = parse(&["--high", "NaN"])
            .unwrap()
            .into_config()
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }
}
