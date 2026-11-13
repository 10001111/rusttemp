//! Command execution.

use std::io::{self, Write};
use std::sync::mpsc;

use crate::cli::{Config, Mode};
use crate::cpuinfo;
use crate::errors::{Error, Result};
use crate::monitoring::Monitor;
use crate::output;
use crate::perf;
use crate::sensors::SensorBackend;
use crate::sensors::platform::{self, sysfs::SysfsBackend};
use crate::terminal::{self, Style};
use crate::time::format_duration;

/// Measurement window before the first sample, so rates (power, load,
/// speed) cover a meaningful interval.
pub(crate) const PERF_WARMUP: std::time::Duration = std::time::Duration::from_millis(500);

/// Creates the monitor with a speed/load/power source and waits for the
/// warm-up window.
pub(crate) fn monitor_with_perf(
    backend: Box<dyn SensorBackend>,
    thresholds: crate::monitoring::Thresholds,
    sysfs_root: Option<&std::path::Path>,
) -> Monitor {
    monitor_with_sources(backend, thresholds, sysfs_root, None)
}

/// Like [`monitor_with_perf`], also attaching a GPU source. Every source is
/// created before the single warm-up wait, so all rates share the window.
pub(crate) fn monitor_with_sources(
    backend: Box<dyn SensorBackend>,
    thresholds: crate::monitoring::Thresholds,
    sysfs_root: Option<&std::path::Path>,
    gpu: Option<Box<dyn crate::sensors::gpu::GpuSource>>,
) -> Monitor {
    let m = Monitor::new(backend, thresholds)
        .with_perf(perf::default_source(sysfs_root))
        .with_gpu(gpu);
    if m.has_perf() || m.has_gpu() {
        std::thread::sleep(PERF_WARMUP);
    }
    m
}

/// Runs the command described by `config`.
pub fn run(config: Config) -> Result<()> {
    let backend: Box<dyn SensorBackend> = match &config.sysfs_root {
        Some(root) => Box::new(SysfsBackend::new(root)?),
        None => platform::default_backend()?,
    };
    let style = if config.json {
        Style::plain(config.zone)
    } else {
        Style::detect(config.no_color, config.zone)
    };
    let mut out = io::stdout().lock();
    let thresholds = config.thresholds;

    match config.mode {
        Mode::Sensors => {
            let text = if config.json {
                output::sensor_list_json(backend.name(), backend.sensors(), &backend.notes())?
                    + "\n"
            } else {
                terminal::render_sensor_list(
                    backend.name(),
                    backend.sensors(),
                    &backend.notes(),
                    style,
                )
            };
            emit(&mut out, &text)
        }
        Mode::Info => {
            let mut cpu = cpuinfo::detect(config.sysfs_root.as_deref());
            let mut monitor =
                monitor_with_perf(backend, config.thresholds, config.sysfs_root.as_deref());
            let snap = monitor.sample();
            // Clock values read from the CPU fill gaps left by CPUID/OS.
            if let Some(p) = snap.perf {
                if cpu.max_mhz.is_none() {
                    cpu.max_mhz = p.max_turbo_mhz.map(|m| m as u32);
                }
                if cpu.base_mhz.is_none() {
                    cpu.base_mhz = p.base_mhz.map(|m| m as u32);
                }
            }
            let text = if config.json {
                output::info_json(&cpu, &snap)? + "\n"
            } else {
                terminal::render_info(&cpu, &snap, style)
            };
            emit(&mut out, &text)
        }
        Mode::Once => {
            let cpu = cpuinfo::detect(config.sysfs_root.as_deref());
            let mut monitor =
                monitor_with_perf(backend, config.thresholds, config.sysfs_root.as_deref());
            let snap = monitor.sample();
            let text = if config.json {
                output::snapshot_json(&snap, &thresholds, Some(&cpu), true)? + "\n"
            } else {
                terminal::render_snapshot(&snap, &thresholds, style, None, Some(&cpu))
            };
            emit(&mut out, &text)
        }
        Mode::Monitor { interval, count } => {
            let (tx, rx) = mpsc::channel();
            ctrlc::set_handler(move || {
                let _ = tx.send(());
            })
            .map_err(|e| Error::Platform(format!("cannot install Ctrl+C handler: {e}")))?;

            let cpu = cpuinfo::detect(config.sysfs_root.as_deref());
            let interval_ms = interval.as_millis() as u64;
            let mut monitor =
                monitor_with_perf(backend, config.thresholds, config.sysfs_root.as_deref());
            let samples = monitor.run(interval, count, &rx, |snap| {
                let text = if config.json {
                    output::snapshot_json(snap, &thresholds, Some(&cpu), false)? + "\n"
                } else {
                    let frame = terminal::render_snapshot(
                        snap,
                        &thresholds,
                        style,
                        Some(interval_ms),
                        Some(&cpu),
                    );
                    if style.redraw {
                        terminal::redraw_frame(&frame)
                    } else {
                        frame + "\n"
                    }
                };
                emit(&mut out, &text)
            })?;

            if !config.json {
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
    }
}

pub(crate) fn emit(out: &mut impl Write, text: &str) -> Result<()> {
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(Error::Output)
}
