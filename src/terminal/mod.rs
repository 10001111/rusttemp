//! Human-readable terminal rendering.
//!
//! Rendering functions return plain `String`s so they can be unit tested; the
//! caller decides how to write them (redraw in place on a TTY, append when
//! piped). Warning levels are always spelled out, so color is never the only
//! carrier of information.

pub mod report;

use std::fmt::Write as _;
use std::io::IsTerminal;

use crate::cpuinfo::{CpuInfo, format_kib};
use crate::health::{self, Condition, HealthSummary};
use crate::monitoring::{Level, Snapshot, Thresholds};
use crate::output::{THRESHOLD_NOTICE, VERSION};
use crate::sensors::platform::WINDOWS_BACKEND;
use crate::sensors::{Reading, SensorInfo};
use crate::time::{Zone, format_clock, format_duration, zone_label};

/// Output styling decided once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// Emit ANSI colors.
    pub color: bool,
    /// Redraw in place (cursor home + clear) instead of appending frames.
    pub redraw: bool,
    /// Clock used for displayed times.
    pub zone: Zone,
}

impl Style {
    /// Plain output: no colors, no cursor movement.
    pub const fn plain(zone: Zone) -> Style {
        Style {
            color: false,
            redraw: false,
            zone,
        }
    }

    /// Detects capabilities of stdout. Colors are disabled by `--no-color`,
    /// by the `NO_COLOR` environment variable, or when stdout is not a terminal.
    pub fn detect(no_color: bool, zone: Zone) -> Style {
        if !std::io::stdout().is_terminal() || !enable_ansi() {
            return Style::plain(zone);
        }
        let env_no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Style {
            color: !no_color && !env_no_color,
            redraw: true,
            zone,
        }
    }
}

/// Enables ANSI escape processing on the Windows console (no-op elsewhere).
#[cfg(windows)]
fn enable_ansi() -> bool {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    // SAFETY: plain Win32 calls on the process's own stdout handle.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }
        mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
            || SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn enable_ansi() -> bool {
    true
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";

fn level_color(level: Level) -> &'static str {
    match level {
        Level::Normal => "\x1b[32m",        // green
        Level::Elevated => "\x1b[33m",      // yellow
        Level::High => "\x1b[1;31m",        // bold red
        Level::Critical => "\x1b[1;37;41m", // bold white on red
    }
}

fn paint(style: Style, code: &str, text: &str) -> String {
    if style.color {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn fmt_temp(c: Option<f64>) -> String {
    c.map_or_else(|| "--".to_string(), |c| format!("{c:.1} °C"))
}

fn label_width(sensors: &[SensorInfo]) -> usize {
    sensors
        .iter()
        .map(|s| s.label.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(6, 32)
}

fn chip_width(sensors: &[SensorInfo]) -> usize {
    sensors
        .iter()
        .map(|s| s.chip.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(4, 28)
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(width.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Explanation printed when no CPU-specific sensor was identified.
pub fn no_cpu_sensor_hint(backend: &str, any_sensors: bool) -> String {
    let mut s = String::new();
    if any_sensors {
        s.push_str(
            "No CPU-specific temperature sensor was identified. The sensors shown are \
             not confirmed to measure the CPU.\n",
        );
    } else {
        s.push_str("No temperature sensors were found.\n");
    }
    match backend {
        WINDOWS_BACKEND => s.push_str(
            "On Windows, CPU package and core temperatures require the PawnIO driver \
             (`winget install namazso.PawnIO`) and running RustTemp from an administrator \
             terminal. See the note above for the exact reason.\n",
        ),
        "linux-sysfs" => s.push_str(
            "Load the CPU sensor driver (Intel: `sudo modprobe coretemp`, AMD: \
             `sudo modprobe k10temp`). Virtual machines, containers and WSL usually expose \
             no CPU sensors.\n",
        ),
        _ => {}
    }
    s
}

/// Diagnostic notes, one per line.
pub fn render_notes(notes: &[String], style: Style) -> String {
    notes
        .iter()
        .map(|n| format!("{} {n}\n", paint(style, BOLD, "Note:")))
        .collect()
}

/// `--sensors` output.
pub fn render_sensor_list(
    backend: &str,
    sensors: &[SensorInfo],
    notes: &[String],
    style: Style,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} — {} sensor(s) discovered via {backend}\n",
        paint(style, BOLD, &format!("RustTemp {VERSION}")),
        sensors.len()
    );
    for (i, s) in sensors.iter().enumerate() {
        let cpu = if s.kind.is_cpu() { "yes" } else { "no" };
        let _ = writeln!(out, "[{}] {}", i + 1, paint(style, BOLD, &s.label));
        let _ = writeln!(out, "    id       {}", s.id);
        let _ = writeln!(out, "    chip     {}", s.chip);
        let _ = writeln!(
            out,
            "    kind     {} (CPU sensor: {cpu})",
            s.kind.describe()
        );
        let _ = writeln!(out, "    source   {}", s.source);
        if s.limits.max_c.is_some() || s.limits.crit_c.is_some() {
            let _ = writeln!(
                out,
                "    limits   max {} · crit {} (reported by hardware/driver)",
                fmt_temp(s.limits.max_c),
                fmt_temp(s.limits.crit_c)
            );
        }
        out.push('\n');
    }
    out.push_str(&render_notes(notes, style));
    if !sensors.iter().any(|s| s.kind.is_cpu()) {
        out.push_str(&no_cpu_sensor_hint(backend, !sensors.is_empty()));
    }
    out
}

/// One sample, as a table. `with_stats` adds min/avg/max columns and the
/// monitoring header (continuous mode).
pub fn render_snapshot(
    snap: &Snapshot<'_>,
    thresholds: &Thresholds,
    style: Style,
    with_stats: Option<u64>,
    cpu: Option<&CpuInfo>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  {}  updated {} {}",
        paint(style, BOLD, &format!("RustTemp {VERSION}")),
        paint(style, DIM, snap.backend),
        format_clock(snap.at, style.zone),
        zone_label(snap.at, style.zone)
    );
    if let Some(interval_ms) = with_stats {
        let _ = writeln!(
            out,
            "{}",
            paint(
                style,
                DIM,
                &format!(
                    "monitoring {} · {} samples · every {interval_ms} ms · Ctrl+C to stop",
                    format_duration(snap.elapsed),
                    snap.sample
                )
            )
        );
    }
    if let Some(cpu) = cpu.filter(|c| c.brand.is_some() || c.vendor.is_some()) {
        let _ = writeln!(
            out,
            "{}",
            paint(style, DIM, &format!("CPU: {}", cpu.summary()))
        );
    }
    out.push('\n');

    let lw = label_width(snap.sensors);
    let cw = chip_width(snap.sensors);
    let rows: Vec<usize> = (0..snap.sensors.len()).collect();
    let (cpu, other): (Vec<usize>, Vec<usize>) = rows
        .into_iter()
        .partition(|&i| snap.sensors[i].kind.is_cpu());

    for (title, group) in [
        ("CPU SENSORS", &cpu),
        ("OTHER SENSORS (not identified as CPU)", &other),
    ] {
        if group.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{}", paint(style, BOLD, title));
        let mut header = format!("  {:<lw$}  {:<cw$}  {:>9}", "SENSOR", "CHIP", "CURRENT");
        if with_stats.is_some() {
            let _ = write!(header, "  {:>9}  {:>9}  {:>9}", "MIN", "AVG", "MAX");
        }
        header.push_str("   UPDATED  STATUS");
        let _ = writeln!(out, "{}", paint(style, DIM, &header));

        for &i in group.iter() {
            let info = &snap.sensors[i];
            let stats = &snap.stats[i];
            let mut line = format!(
                "  {:<lw$}  {:<cw$}  ",
                truncate(&info.label, lw),
                truncate(&info.chip, cw)
            );
            let status = match &snap.readings[i] {
                Reading::Celsius(c) => {
                    let level = thresholds.classify(*c);
                    let _ = write!(
                        line,
                        "{}",
                        paint(
                            style,
                            level_color(level),
                            &format!("{:>9}", fmt_temp(Some(*c)))
                        )
                    );
                    paint(style, level_color(level), level.label())
                }
                Reading::Unavailable(why) => {
                    let _ = write!(line, "{:>9}", "--");
                    paint(style, DIM, &format!("UNAVAILABLE: {why}"))
                }
            };
            if with_stats.is_some() {
                let _ = write!(
                    line,
                    "  {:>9}  {:>9}  {:>9}",
                    fmt_temp(stats.min()),
                    fmt_temp(stats.average()),
                    fmt_temp(stats.max())
                );
            }
            let updated = stats
                .last_valid_at()
                .map_or_else(|| "never".to_string(), |t| format_clock(t, style.zone));
            let _ = writeln!(line, "  {updated:>8}  {status}");
            out.push_str(&line);
        }
        out.push('\n');
    }

    if let Some(p) = snap.perf.filter(|p| p.has_data()) {
        out.push_str(&render_perf(
            p,
            snap.perf_stats,
            style,
            with_stats.is_some(),
        ));
        out.push('\n');
    }
    if !cpu.is_empty() || snap.counters.is_some() {
        out.push_str(&render_health(
            &health::summarize(snap),
            style,
            with_stats.is_some(),
        ));
        out.push('\n');
    }
    if !snap.notes.is_empty() {
        out.push_str(&render_notes(snap.notes, style));
        if !cpu.is_empty() {
            out.push('\n');
        }
    }
    if cpu.is_empty() {
        out.push_str(&no_cpu_sensor_hint(snap.backend, !snap.sensors.is_empty()));
        out.push('\n');
    }

    let _ = writeln!(
        out,
        "{}",
        paint(
            style,
            DIM,
            &format!(
                "Thresholds: elevated ≥{} °C · high ≥{} °C · critical ≥{} °C ({THRESHOLD_NOTICE})",
                thresholds.elevated_c, thresholds.high_c, thresholds.critical_c
            )
        )
    );
    out
}

/// `no`, `YES (all CPU sensors)`, `YES (a, b)` or `YES (a, b, c, +N more)`.
fn condition_text(list: &[String], total: usize, style: Style, color: &str) -> String {
    let text = match list.len() {
        0 => return "no".to_string(),
        n if n == total && total > 1 => "YES (all CPU sensors)".to_string(),
        n if n <= 3 => format!("YES ({})", list.join(", ")),
        n => format!("YES ({}, +{} more)", list[..3].join(", "), n - 3),
    };
    paint(style, color, &text)
}

fn condition_row(c: &Condition, total: usize, style: Style, color: &str) -> String {
    format!(
        "now: {} · logged: {}",
        condition_text(&c.now, total, style, color),
        condition_text(&c.logged, total, style, color)
    )
}

/// Plain-language meaning of each health line (shown by `--info`).
pub const HEALTH_EXPLANATIONS: &[(&str, &str)] = &[
    (
        "Thermal throttling",
        "the CPU slowed itself down because it reached its throttle temperature. \
         Occasional events under heavy load are common on laptops; throttling during \
         light use suggests a cooling problem (dust, fan, thermal paste).",
    ),
    (
        "PROCHOT throttling",
        "another part of the system (e.g. the laptop's embedded controller or power \
         circuitry) told the CPU to slow down, usually because something else is hot.",
    ),
    (
        "Critical temperature",
        "the CPU reached shutdown-level temperature. This should never happen; if it \
         is logged, check the cooling immediately.",
    ),
    (
        "Power-limit throttling",
        "speed is held back by the configured power limit. Expected on laptops, \
         especially on battery or in quiet/balanced power modes; not a fault.",
    ),
    (
        "Headroom",
        "degrees left before the CPU's own limit (TjMax), where it throttles hard to \
         protect itself.",
    ),
];

/// The CPU HEALTH section. `session` adds whole-session observations.
pub fn render_health(h: &HealthSummary, style: Style, session: bool) -> String {
    let mut out = String::new();
    let mut row = |name: &str, value: String| {
        let _ = writeln!(out, "  {name:<24}{value}");
    };
    let red = level_color(Level::High);
    let yellow = level_color(Level::Elevated);
    if h.flags_available {
        row(
            "Thermal throttling",
            condition_row(&h.thermal_throttling, h.flag_sensors, style, red),
        );
        row(
            "PROCHOT throttling",
            condition_row(&h.prochot, h.flag_sensors, style, red),
        );
        row(
            "Critical temperature",
            condition_row(&h.critical_temperature, h.flag_sensors, style, red),
        );
        row(
            "Power-limit throttling",
            match &h.power_limit_throttling {
                Some(c) => condition_row(c, h.flag_sensors, style, yellow),
                None => "not reported by this CPU".to_string(),
            },
        );
    }
    if let Some(c) = h.throttle_counters {
        row(
            "Throttle events",
            format!(
                "since boot: package {} · core up to {} per CPU ({} of {} CPUs affected)",
                c.package_events, c.core_events_max, c.cpus_with_core_events, c.cpus_reporting
            ),
        );
    }
    if !h.flags_available && h.throttle_counters.is_none() {
        row(
            "Throttling status",
            "not available (Windows: requires PawnIO and an administrator terminal; \
             Linux: the kernel exposes no thermal_throttle counters)"
                .to_string(),
        );
    }
    row(
        "Hottest CPU sensor",
        match &h.hottest_cpu_sensor {
            Some(x) => match (x.headroom_c, x.limit_c) {
                (Some(hr), Some(limit)) => format!(
                    "{} at {:.1} °C · {:.1} °C below its {:.1} °C limit",
                    x.label, x.temperature_c, hr, limit
                ),
                _ => format!(
                    "{} at {:.1} °C (no hardware limit reported)",
                    x.label, x.temperature_c
                ),
            },
            None => "no CPU temperature available".to_string(),
        },
    );
    if session {
        let shares: Vec<String> = h
            .session
            .time_at_or_above
            .iter()
            .map(|s| format!("≥{} {:.0}%", s.level.label(), s.percent))
            .collect();
        row(
            "This session",
            format!(
                "thermal throttling in {} of {} samples · time {} (worst CPU sensor)",
                h.session.thermal_throttle_samples,
                h.session.samples,
                shares.join(" · ")
            ),
        );
    }
    let mut text = format!("{}\n", paint(style, BOLD, "CPU HEALTH"));
    text.push_str(&out);
    if h.flags_available {
        let _ = writeln!(
            text,
            "  {}",
            paint(
                style,
                DIM,
                "\"logged\" = flagged by the CPU since power-on, or since software last cleared it"
            )
        );
    }
    text
}

fn cache_line(g: &crate::cpuinfo::CacheGroup) -> String {
    let name = match g.kind {
        "unified" => format!("L{}", g.level),
        k => format!("L{} {k}", g.level),
    };
    let parts: Vec<String> = g
        .instances
        .iter()
        .map(|(size, n)| {
            if *n == 1 {
                format_kib(*size)
            } else {
                format!("{n} × {}", format_kib(*size))
            }
        })
        .collect();
    let multi = g.instances.len() > 1 || g.instances.first().is_some_and(|i| i.1 > 1);
    if multi {
        format!(
            "{name}: {} ({} total)",
            parts.join(" + "),
            format_kib(g.total_kib())
        )
    } else {
        format!("{name}: {}", parts.join(" + "))
    }
}

/// `--info` output: CPU identity, then health from one sample.
pub fn render_info(cpu: &CpuInfo, snap: &Snapshot<'_>, style: Style) -> String {
    let na = || "unavailable".to_string();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} — CPU information\n",
        paint(style, BOLD, &format!("RustTemp {VERSION}"))
    );
    let mut row = |name: &str, value: String| {
        let _ = writeln!(out, "  {name:<16}{value}");
    };
    row(
        "Manufacturer",
        match (&cpu.vendor, &cpu.vendor_id) {
            (Some(v), Some(id)) if v != id => format!("{v} ({id})"),
            (Some(v), _) => v.clone(),
            _ => na(),
        },
    );
    row("Model", cpu.brand.clone().unwrap_or_else(na));
    row(
        "Signature",
        match (cpu.family, cpu.model, cpu.stepping) {
            (Some(f), Some(m), Some(st)) => {
                format!("family {f} · model {m} (0x{m:X}) · stepping {st}")
            }
            _ => na(),
        },
    );
    row("Microcode", cpu.microcode.clone().unwrap_or_else(na));
    let mut cores = match cpu.physical_cores {
        Some(c) => match (cpu.performance_cores, cpu.efficiency_cores) {
            (Some(p), Some(e)) => format!("{c} physical ({p} performance + {e} efficiency)"),
            _ => format!("{c} physical"),
        },
        None => "physical: unavailable".to_string(),
    };
    if let Some(t) = cpu.logical_processors {
        let _ = write!(cores, " · {t} logical");
    }
    if let Some(p) = cpu.packages {
        let _ = write!(cores, " · {p} package{}", if p == 1 { "" } else { "s" });
    }
    row("Cores", cores);
    if cpu.caches.is_empty() {
        row("Cache", na());
    } else {
        for (i, g) in cpu.caches.iter().enumerate() {
            row(if i == 0 { "Cache" } else { "" }, cache_line(g));
        }
    }
    row(
        "Clock speed",
        match (cpu.base_mhz, cpu.max_mhz) {
            (None, None) => na(),
            (b, m) => format!(
                "base {} · max {}",
                b.map_or_else(na, |v| format!("{v} MHz")),
                m.map_or_else(na, |v| format!("{v} MHz"))
            ),
        },
    );
    let limit = snap
        .sensors
        .iter()
        .filter(|s| s.kind.is_cpu())
        .filter_map(|s| s.limits.crit_c.map(|c| (c, s.chip.as_str())))
        .fold(None, |best: Option<(f64, &str)>, x| match best {
            Some(b) if b.0 >= x.0 => Some(b),
            _ => Some(x),
        });
    row(
        "Thermal limit",
        match limit {
            Some((c, chip)) => format!("{c:.1} °C (reported by the CPU via {chip})"),
            None => "unavailable (no CPU temperature sensor accessible)".to_string(),
        },
    );
    if cpu.virtual_machine {
        row(
            "Environment",
            "virtual machine: cores, caches and microcode describe the virtual CPU, \
             which may differ from the physical processor"
                .to_string(),
        );
    }
    row(
        "Data sources",
        if cpu.sources.is_empty() {
            na()
        } else {
            cpu.sources.join(", ")
        },
    );
    out.push('\n');
    if let Some(p) = snap.perf.filter(|p| p.has_data()) {
        out.push_str(&render_perf(p, snap.perf_stats, style, false));
        out.push('\n');
    }
    let summary = health::summarize(snap);
    out.push_str(&render_health(&summary, style, false));
    if summary.flags_available || summary.hottest_cpu_sensor.is_some() {
        let _ = writeln!(out, "\n{}", paint(style, BOLD, "WHAT THESE MEAN"));
        for (name, meaning) in HEALTH_EXPLANATIONS {
            let _ = writeln!(out, "  {name:<24}{meaning}");
        }
    }
    if !snap.notes.is_empty() {
        out.push('\n');
        out.push_str(&render_notes(snap.notes, style));
    }
    out
}

fn fmt_mhz(v: Option<f64>) -> Option<String> {
    v.map(|m| format!("{m:.0} MHz"))
}

/// The CPU SPEED & POWER section. `session` adds whole-session statistics.
pub fn render_perf(
    p: &crate::perf::PerfSample,
    stats: &crate::perf::PerfStats,
    style: Style,
    session: bool,
) -> String {
    let mut out = format!("{}\n", paint(style, BOLD, "CPU SPEED & POWER"));
    let mut row = |name: &str, value: String| {
        let _ = writeln!(out, "  {name:<24}{value}");
    };
    let why = |r: &Option<String>| r.clone().unwrap_or_else(|| "not reported".into());
    row(
        "Package power",
        match p.package_w {
            Some(w) => match p.cores_w {
                Some(c) => format!("{w:.1} W (cores {c:.1} W)"),
                None => format!("{w:.1} W"),
            },
            None => format!("unavailable ({})", why(&p.power_unavailable)),
        },
    );
    if p.limits.any() {
        let lim = |name: &str, w: Option<f64>, en: Option<bool>| {
            w.map(|w| match en {
                Some(false) => format!("{name} {w:.1} W (disabled)"),
                _ => format!("{name} {w:.1} W"),
            })
        };
        let parts: Vec<String> = [
            lim("PL1", p.limits.pl1_w, p.limits.pl1_enabled),
            lim("PL2", p.limits.pl2_w, p.limits.pl2_enabled),
            p.limits.tdp_w.map(|w| format!("TDP {w:.1} W")),
        ]
        .into_iter()
        .flatten()
        .collect();
        row("Power limits", parts.join(" · "));
        row(
            "",
            paint(
                style,
                DIM,
                "values programmed in the CPU; firmware, the power mode or battery \
                 operation can enforce lower effective limits",
            ),
        );
    }
    let mut clock: Vec<String> = Vec::new();
    match fmt_mhz(p.avg_mhz) {
        Some(a) => clock.push(format!("avg {a}")),
        None => clock.push(format!("avg unavailable ({})", why(&p.speed_unavailable))),
    }
    if let Some(b) = fmt_mhz(p.base_mhz) {
        clock.push(format!("base {b}"));
    }
    if let Some(m) = fmt_mhz(p.max_turbo_mhz) {
        clock.push(format!("max turbo {m}"));
    }
    row("Clock", clock.join(" · "));
    if let Some(l) = p.total_load_pct {
        row("CPU load", format!("{l:.0}%"));
    }
    if session {
        let mut parts = Vec::new();
        if let (Some(avg), Some(max)) = (stats.package_w.avg, stats.package_w.max) {
            parts.push(format!("power avg {avg:.1} W · max {max:.1} W"));
        }
        if let (Some(m), Some(c)) = (stats.peak_mhz, &stats.peak_core) {
            parts.push(format!("peak clock {m:.0} MHz ({c})"));
        }
        if let Some(l) = stats.total_load_pct.avg {
            parts.push(format!("avg load {l:.0}%"));
        }
        if !parts.is_empty() {
            row("This session", parts.join(" · "));
        }
    }
    let sources: Vec<&str> = [p.speed_source, p.power_source]
        .into_iter()
        .flatten()
        .collect();
    if !p.cores.is_empty() {
        let lw = p
            .cores
            .iter()
            .map(|c| c.label.chars().count())
            .max()
            .unwrap_or(4)
            .max(4);
        let _ = writeln!(
            out,
            "{}",
            paint(
                style,
                DIM,
                &format!("  {:<lw$}  {:>9}  {:>5}", "CORE", "SPEED", "LOAD")
            )
        );
        for c in &p.cores {
            let speed = fmt_mhz(c.mhz).unwrap_or_else(|| {
                // Speed is measurable in general, but this core had no active
                // cycles in the window: it was asleep.
                if p.speed_source.is_some() && c.load_pct.is_some_and(|l| l < 1.0) {
                    "idle".into()
                } else {
                    "--".into()
                }
            });
            let load = c
                .load_pct
                .map_or_else(|| "--".into(), |l| format!("{l:.0}%"));
            let _ = writeln!(out, "  {:<lw$}  {speed:>9}  {load:>5}", c.label);
        }
    }
    if !sources.is_empty() {
        let _ = writeln!(
            out,
            "  {}",
            paint(style, DIM, &format!("sources: {}", sources.join(", ")))
        );
    }
    out
}

/// Wraps a frame for in-place redraw: cursor home, clear each line's tail,
/// clear below the frame.
pub fn redraw_frame(frame: &str) -> String {
    let mut out = String::with_capacity(frame.len() + 64);
    out.push_str("\x1b[H");
    for line in frame.lines() {
        out.push_str(line);
        out.push_str("\x1b[K\n");
    }
    out.push_str("\x1b[J");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::SensorStats;
    use crate::sensors::{HardwareLimits, SensorKind};
    use std::time::{Duration, UNIX_EPOCH};

    fn sensor(label: &str, chip: &str, kind: SensorKind) -> SensorInfo {
        SensorInfo {
            id: format!("x/{label}"),
            label: label.into(),
            chip: chip.into(),
            kind,
            source: "/dev/null".into(),
            limits: HardwareLimits::default(),
        }
    }

    fn render(
        sensors: &[SensorInfo],
        readings: &[Reading],
        stats: Option<u64>,
        style: Style,
    ) -> String {
        let at = UNIX_EPOCH + Duration::from_secs(1_794_476_040);
        let mut st = vec![SensorStats::default(); sensors.len()];
        for (s, r) in st.iter_mut().zip(readings) {
            s.record(r, at);
        }
        let snap = Snapshot {
            backend: "linux-sysfs",
            at,
            elapsed: Duration::from_secs(83),
            sample: 42,
            sensors,
            readings,
            stats: &st,
            notes: &[],
            flags: &[],
            counters: None,
            throttle_samples: 0,
            perf: None,
            perf_stats: &crate::perf::PerfStats::default(),
            gpu_devices: &[],
            gpu_samples: &[],
        };
        render_snapshot(&snap, &Thresholds::default(), style, stats, None)
    }

    #[test]
    fn groups_cpu_and_other_sensors_and_labels_levels() {
        let sensors = [
            sensor("Package id 0", "coretemp", SensorKind::CpuPackage),
            sensor("Core 0", "coretemp", SensorKind::CpuCore),
            sensor("Composite", "nvme", SensorKind::Other),
        ];
        let readings = [
            Reading::Celsius(96.0),
            Reading::Unavailable("read failed".into()),
            Reading::Celsius(38.0),
        ];
        let out = render(&sensors, &readings, None, Style::plain(Zone::Utc));
        assert!(out.contains("updated 09:34:00 UTC"));
        let cpu_at = out.find("CPU SENSORS").unwrap();
        let other_at = out.find("OTHER SENSORS").unwrap();
        assert!(cpu_at < other_at);
        assert!(out.contains("96.0 °C"));
        assert!(out.contains("CRITICAL"));
        assert!(out.contains("UNAVAILABLE: read failed"));
        assert!(out.contains("NORMAL"));
        assert!(out.contains("not processor-specific"));
        assert!(
            !out.contains("MIN"),
            "single readings have no statistics columns"
        );
        assert!(!out.contains('\x1b'), "plain style must not emit escapes");
        assert!(!out.contains("No CPU-specific"));
    }

    #[test]
    fn monitoring_view_has_statistics() {
        let sensors = [sensor("Core 0", "coretemp", SensorKind::CpuCore)];
        let out = render(
            &sensors,
            &[Reading::Celsius(71.0)],
            Some(2000),
            Style::plain(Zone::Utc),
        );
        assert!(out.contains("monitoring 00:01:23 · 42 samples · every 2000 ms"));
        assert!(out.contains("MIN"));
        assert!(out.contains("ELEVATED"));
    }

    #[test]
    fn colors_only_when_enabled() {
        let sensors = [sensor("Core 0", "coretemp", SensorKind::CpuCore)];
        let style = Style {
            color: true,
            redraw: true,
            zone: Zone::Utc,
        };
        let out = render(&sensors, &[Reading::Celsius(88.0)], None, style);
        assert!(out.contains("\x1b[1;31m"));
        assert!(out.contains("HIGH"));
    }

    #[test]
    fn no_cpu_sensor_explanation() {
        let sensors = [sensor(
            "\\_TZ.THRM",
            "ACPI thermal zone",
            SensorKind::AcpiThermalZone,
        )];
        let out = render(
            &sensors,
            &[Reading::Celsius(70.0)],
            None,
            Style::plain(Zone::Utc),
        );
        assert!(out.contains("No CPU-specific temperature sensor was identified"));
        assert!(!out.contains("CPU SENSORS\n"));
        let empty = render(&[], &[], None, Style::plain(Zone::Utc));
        assert!(empty.contains("No temperature sensors were found"));
    }

    #[test]
    fn sensor_list_shows_identity() {
        let mut s = sensor("Package id 0", "coretemp", SensorKind::CpuPackage);
        s.limits.crit_c = Some(100.0);
        let out = render_sensor_list("linux-sysfs", &[s], &[], Style::plain(Zone::Utc));
        assert!(out.contains("1 sensor(s) discovered via linux-sysfs"));
        assert!(out.contains("CPU package (CPU sensor: yes)"));
        assert!(out.contains("crit 100.0 °C"));
        assert!(out.contains("source   /dev/null"));
    }

    #[test]
    fn long_labels_are_truncated() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn condition_wording() {
        let st = Style::plain(Zone::Utc);
        let names: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(condition_text(&[], 5, st, ""), "no");
        assert_eq!(condition_text(&names, 5, st, ""), "YES (all CPU sensors)");
        assert_eq!(condition_text(&names[..2], 5, st, ""), "YES (a, b)");
        assert_eq!(
            condition_text(&names[..4], 5, st, ""),
            "YES (a, b, c, +1 more)"
        );
        assert_eq!(condition_text(&names[..1], 1, st, ""), "YES (a)");
    }

    #[test]
    fn redraw_wraps_lines() {
        assert_eq!(redraw_frame("a\nb\n"), "\x1b[Ha\x1b[K\nb\x1b[K\n\x1b[J");
    }
}
