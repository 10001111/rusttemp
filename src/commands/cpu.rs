//! `cpu`: CPU report (identification, temperatures, frequency, load,
//! power, voltage, throttling).

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use clap::Parser;

use super::parse_interval;
use crate::app::{emit, monitor_with_sources};
use crate::cpuinfo;
use crate::errors::{Error, Result};
use crate::monitoring::Thresholds;
use crate::monitoring::thresholds::{DEFAULT_CRITICAL_C, DEFAULT_ELEVATED_C, DEFAULT_HIGH_C};
use crate::output;
use crate::report;
use crate::sensors::SensorBackend;
use crate::sensors::platform::{self, sysfs::SysfsBackend};
use crate::terminal::{self, Style};
use crate::time::{Zone, format_duration};

/// Show a CPU report: identification, temperatures, frequency, load, power,
/// voltage and throttling.
///
/// Values that need administrator rights (temperatures, power, voltage and
/// throttling flags on Windows, read through the PawnIO driver) are shown as
/// unavailable, with the reason, when run without them.
#[derive(Debug, Parser)]
#[command(name = "cpu", version, about, long_about)]
pub struct CpuArgs {
    /// Keep refreshing the report until Ctrl+C.
    #[arg(long)]
    pub watch: bool,

    /// Seconds between refreshes in --watch mode (0.25–3600, decimals allowed).
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0, value_parser = parse_interval, requires = "watch")]
    pub interval: f64,

    /// Stop --watch after N refreshes.
    #[arg(long, value_name = "N", requires = "watch", value_parser = clap::value_parser!(u64).range(1..))]
    pub count: Option<u64>,

    /// Print JSON (one document; one compact document per line with --watch).
    #[arg(long)]
    pub json: bool,

    /// Disable colours (the NO_COLOR environment variable is also honoured).
    #[arg(long)]
    pub no_color: bool,

    /// Show times in UTC instead of local time.
    #[arg(long)]
    pub utc: bool,

    /// Lower bound of the ELEVATED temperature level in °C.
    #[arg(long, value_name = "CELSIUS", default_value_t = DEFAULT_ELEVATED_C, allow_negative_numbers = true)]
    pub elevated: f64,

    /// Lower bound of the HIGH temperature level in °C.
    #[arg(long, value_name = "CELSIUS", default_value_t = DEFAULT_HIGH_C, allow_negative_numbers = true)]
    pub high: f64,

    /// Lower bound of the CRITICAL temperature level in °C.
    #[arg(long, value_name = "CELSIUS", default_value_t = DEFAULT_CRITICAL_C, allow_negative_numbers = true)]
    pub critical: f64,

    /// Read a Linux-style sensor tree under PATH (diagnostics and testing).
    #[arg(long, value_name = "PATH", hide = true)]
    pub sysfs_root: Option<PathBuf>,
}

/// Entry point of the `cpu` executable.
pub fn main() -> ! {
    super::finish("cpu", run(CpuArgs::parse()))
}

/// Runs the `cpu` command.
pub fn run(args: CpuArgs) -> Result<()> {
    let thresholds = Thresholds::new(args.elevated, args.high, args.critical)?;
    let zone = if args.utc { Zone::Utc } else { Zone::Local };
    let style = if args.json {
        Style::plain(zone)
    } else {
        Style::detect(args.no_color, zone)
    };
    let root = args.sysfs_root.as_deref();
    let backend: Box<dyn SensorBackend> = match root {
        Some(r) => Box::new(SysfsBackend::new(r)?),
        None => platform::default_backend()?,
    };
    let cpu = cpuinfo::detect(root);
    let mut out = io::stdout().lock();
    let (gpu, gpu_error) = match crate::sensors::gpu::default_source(root) {
        Ok(g) => (Some(g), None),
        Err(e) => (None, Some(e)),
    };
    let mut monitor = monitor_with_sources(backend, thresholds, root, gpu);

    if !args.watch {
        let snap = monitor.sample();
        let r = report::cpu::build(&cpu, &snap, gpu_error.as_deref());
        let text = if args.json {
            output::cpu_report_json(&r, snap.at, snap.sample, &thresholds, true)? + "\n"
        } else {
            terminal::report::render_cpu_report(&r, snap.at, &thresholds, style, None)
        };
        return emit(&mut out, &text);
    }

    let (tx, rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .map_err(|e| Error::Platform(format!("cannot install Ctrl+C handler: {e}")))?;
    let interval = Duration::from_secs_f64(args.interval);
    let samples = monitor.run(interval, args.count, &rx, |snap| {
        let r = report::cpu::build(&cpu, snap, gpu_error.as_deref());
        let text = if args.json {
            output::cpu_report_json(&r, snap.at, snap.sample, &thresholds, false)? + "\n"
        } else {
            let frame = terminal::report::render_cpu_report(
                &r,
                snap.at,
                &thresholds,
                style,
                Some((args.interval, snap.sample)),
            );
            if style.redraw {
                terminal::redraw_frame(&frame)
            } else {
                frame + "\n"
            }
        };
        emit(&mut out, &text)
    })?;
    if !args.json {
        let elapsed = monitor.snapshot().elapsed;
        emit(
            &mut out,
            &format!(
                "\nMonitoring stopped after {} ({samples} samples).\n",
                format_duration(elapsed)
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> std::result::Result<CpuArgs, clap::Error> {
        CpuArgs::try_parse_from(std::iter::once("cpu").chain(args.iter().copied()))
    }

    #[test]
    fn clap_definition_is_consistent() {
        CpuArgs::command().debug_assert();
    }

    #[test]
    fn options() {
        let a = parse(&[]).unwrap();
        assert!(!a.watch && !a.json);
        assert_eq!(a.interval, 1.0);
        let a = parse(&["--watch", "--interval", "2"]).unwrap();
        assert!(a.watch);
        assert_eq!(a.interval, 2.0);
        assert_eq!(
            parse(&["--watch", "--interval", "0.5", "--count", "3"])
                .unwrap()
                .count,
            Some(3)
        );
        assert!(parse(&["--json"]).unwrap().json);
    }

    #[test]
    fn invalid_options() {
        // --interval and --count only make sense with --watch.
        assert_eq!(
            parse(&["--interval", "2"]).unwrap_err().kind(),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            parse(&["--count", "2"]).unwrap_err().kind(),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            parse(&["--watch", "--interval", "0"]).unwrap_err().kind(),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse(&["--bogus"]).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
    }
}
