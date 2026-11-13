//! `gpu`: dedicated GPU report (each GPU separately).

use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use clap::Parser;

use super::parse_interval;
use crate::app::{PERF_WARMUP, emit};
use crate::errors::{Error, Result};
use crate::monitoring::thresholds::{DEFAULT_CRITICAL_C, DEFAULT_ELEVATED_C, DEFAULT_HIGH_C};
use crate::monitoring::{Monitor, Thresholds};
use crate::output;
use crate::perf;
use crate::report;
use crate::sensors::{Reading, SensorBackend, SensorInfo};
use crate::terminal::{self, Style};
use crate::time::{Zone, format_duration};

/// Show the dedicated (discrete) GPU report: identification, utilization,
/// clocks, temperatures, power, VRAM, fans and throttling, for each GPU
/// separately.
///
/// Integrated graphics are reported by `cpu`. Devices that cannot be
/// confirmed as integrated or dedicated are shown in their own section.
#[derive(Debug, Parser)]
#[command(name = "gpu", version, about, long_about)]
pub struct GpuArgs {
    /// Keep refreshing the report until Ctrl+C.
    #[arg(long)]
    pub watch: bool,

    /// Seconds between refreshes in --watch mode (0.25–3600, decimals allowed).
    #[arg(long, value_name = "SECONDS", default_value_t = 1.0, value_parser = parse_interval, requires = "watch")]
    pub interval: f64,

    /// Stop --watch after N refreshes.
    #[arg(long, value_name = "N", requires = "watch", value_parser = clap::value_parser!(u64).range(1..))]
    pub count: Option<u64>,

    /// Also show integrated GPUs and software renderers.
    #[arg(long)]
    pub all: bool,

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

    /// Read a Linux-style sysfs tree under PATH (diagnostics and testing).
    #[arg(long, value_name = "PATH", hide = true)]
    pub sysfs_root: Option<PathBuf>,
}

/// The `gpu` command reads no CPU temperature sensors.
struct NoTemperatureSensors;

impl SensorBackend for NoTemperatureSensors {
    fn name(&self) -> &'static str {
        "gpu"
    }
    fn sensors(&self) -> &[SensorInfo] {
        &[]
    }
    fn read(&mut self) -> Vec<Reading> {
        Vec::new()
    }
}

/// Entry point of the `gpu` executable.
pub fn main() -> ! {
    super::finish("gpu", run(GpuArgs::parse()))
}

/// Runs the `gpu` command.
pub fn run(args: GpuArgs) -> Result<()> {
    let thresholds = Thresholds::new(args.elevated, args.high, args.critical)?;
    let zone = if args.utc { Zone::Utc } else { Zone::Local };
    let style = if args.json {
        Style::plain(zone)
    } else {
        Style::detect(args.no_color, zone)
    };
    let root = args.sysfs_root.as_deref();
    let (gpu, gpu_error) = match crate::sensors::gpu::default_source(root) {
        Ok(g) => (Some(g), None),
        Err(e) => (None, Some(e)),
    };
    // CPU power data is only needed for integrated-GPU power (shown with --all).
    let perf = if args.all {
        perf::default_source(root)
    } else {
        None
    };
    let mut monitor = Monitor::new(Box::new(NoTemperatureSensors), thresholds)
        .with_perf(perf)
        .with_gpu(gpu);
    if monitor.has_gpu() || monitor.has_perf() {
        std::thread::sleep(PERF_WARMUP);
    }
    let mut out = io::stdout().lock();

    if !args.watch {
        let snap = monitor.sample();
        let r = report::gpu::gpu_command_report(&snap, args.all, gpu_error.as_deref());
        let text = if args.json {
            output::gpu_report_json(&r, snap.at, snap.sample, true)? + "\n"
        } else {
            terminal::report::render_gpu_report(&r, snap.at, &thresholds, style, None)
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
        let r = report::gpu::gpu_command_report(snap, args.all, gpu_error.as_deref());
        let text = if args.json {
            output::gpu_report_json(&r, snap.at, snap.sample, false)? + "\n"
        } else {
            let frame = terminal::report::render_gpu_report(
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

    fn parse(args: &[&str]) -> std::result::Result<GpuArgs, clap::Error> {
        GpuArgs::try_parse_from(std::iter::once("gpu").chain(args.iter().copied()))
    }

    #[test]
    fn clap_definition_is_consistent() {
        GpuArgs::command().debug_assert();
    }

    #[test]
    fn options() {
        assert!(parse(&["--all"]).unwrap().all);
        assert_eq!(
            parse(&["--watch", "--interval", "2"]).unwrap().interval,
            2.0
        );
        assert_eq!(
            parse(&["--interval", "2"]).unwrap_err().kind(),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            parse(&["--bogus"]).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
    }
}
