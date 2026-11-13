//! Terminal layout for the `cpu` report.
//!
//! Layout: a banner, then titled sections of `Label: value  [source]` lines.
//! A measured value shows its source dimmed; an unavailable value shows
//! `Unavailable — <reason>`. Temperatures are coloured by warning level and
//! utilization by load, but every value is also written in plain text.

use std::fmt::Write as _;

use super::{BOLD, DIM, Style, level_color, paint, render_notes};
use crate::hardware::gpu::GpuKind;
use crate::measurement::Measurement;
use crate::monitoring::{Level, Thresholds};
use crate::output::VERSION;
use crate::report::cpu::{CpuReport, ThrottleState};
use crate::report::gpu::{GpuCommandReport, GpuReport};
use crate::time::{format_clock, zone_label};

/// Width of banners and section rules.
pub const WIDTH: usize = 64;
const LABEL_WIDTH: usize = 22;

/// Colour for a utilization percentage.
pub fn utilization_color(pct: f64) -> &'static str {
    if pct >= 85.0 {
        level_color(Level::High)
    } else if pct >= 60.0 {
        level_color(Level::Elevated)
    } else {
        level_color(Level::Normal)
    }
}

/// Banner with a centred title.
pub fn banner(title: &str, style: Style) -> String {
    let rule = "=".repeat(WIDTH);
    let pad = WIDTH.saturating_sub(title.chars().count()) / 2;
    format!(
        "{rule}\n{}{}\n{rule}\n",
        " ".repeat(pad),
        paint(style, BOLD, title)
    )
}

/// Section title with a rule.
pub fn section(title: &str, style: Style) -> String {
    format!("\n{}\n{}\n", paint(style, BOLD, title), "-".repeat(WIDTH))
}

/// Text for a measurement: the formatted (optionally coloured) value plus
/// its source, or the reason it is missing.
pub fn show<T>(
    m: &Measurement<T>,
    style: Style,
    fmt: impl Fn(&T) -> String,
    color: impl Fn(&T) -> Option<&'static str>,
) -> String {
    match m {
        Measurement::Measured { value, source, .. } => {
            let text = fmt(value);
            let text = match color(value) {
                Some(c) => paint(style, c, &text),
                None => text,
            };
            if source.is_empty() {
                text
            } else {
                format!("{text}  {}", paint(style, DIM, &format!("[{source}]")))
            }
        }
        Measurement::Unavailable { reason } => {
            paint(style, DIM, &format!("Unavailable — {reason}"))
        }
        Measurement::NotApplicable { reason } => {
            paint(style, DIM, &format!("Not applicable — {reason}"))
        }
    }
}

fn no_color<T>(_: &T) -> Option<&'static str> {
    None
}

/// `Label:  value` line.
pub fn line(out: &mut String, label: &str, value: String) {
    // An empty label continues the previous line (no colon).
    let label = if label.is_empty() {
        String::new()
    } else {
        format!("{label}:")
    };
    let _ = writeln!(out, "{label:<LABEL_WIDTH$}{value}");
}

fn mhz(v: &f64) -> String {
    format!("{v:.0} MHz")
}

fn celsius(v: &f64) -> String {
    format!("{v:.1} °C")
}

fn watts(v: &f64) -> String {
    format!("{v:.1} W")
}

fn percent(v: &f64) -> String {
    format!("{v:.0}%")
}

/// Short cell text for tables: value or `--`/`idle`.
fn cell<T>(m: &Measurement<T>, fmt: impl Fn(&T) -> String) -> String {
    match m {
        Measurement::Measured { value, .. } => fmt(value),
        Measurement::Unavailable { reason } if reason.contains("idle") => "idle".into(),
        _ => "--".into(),
    }
}

/// Eight-level sparkline of `values`, scaled between their min and max.
pub fn sparkline(values: &[f64]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let (min, max) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    values
        .iter()
        .map(|&v| {
            let i = if max > min {
                (((v - min) / (max - min)) * 7.0).round() as usize
            } else {
                3
            };
            BARS[i.min(7)]
        })
        .collect()
}

fn throttle_text(t: &ThrottleState) -> String {
    let now = match t.now {
        Some(true) => "YES, right now".to_string(),
        Some(false) => "No".to_string(),
        None => "Unknown (no live flag)".to_string(),
    };
    match t.since_boot {
        Some(true) => format!("{now} (has occurred since boot)"),
        Some(false) => format!("{now} (not since boot)"),
        None => now,
    }
}

fn throttle_color(t: &ThrottleState) -> Option<&'static str> {
    (t.now == Some(true)).then(|| level_color(Level::High))
}

/// Renders the CPU report. `watch` is the refresh interval in seconds when
/// running in `--watch` mode.
pub fn render_cpu_report(
    r: &CpuReport,
    at: std::time::SystemTime,
    thresholds: &Thresholds,
    style: Style,
    watch: Option<(f64, u64)>,
) -> String {
    let temp_color = |c: &f64| Some(level_color(thresholds.classify(*c)));
    let util_color = |p: &f64| Some(utilization_color(*p));
    let mut out = banner("RUST HARDWARE MONITOR — CPU", style);
    let mut header = format!(
        "updated {} {} · RustTemp {VERSION}",
        format_clock(at, style.zone),
        zone_label(at, style.zone)
    );
    if let Some((secs, samples)) = watch {
        let _ = write!(
            header,
            " · refresh {secs} s · {samples} samples · Ctrl+C to stop"
        );
    }
    let _ = writeln!(out, "{}", paint(style, DIM, &header));

    // Identification.
    let id = &r.identification;
    out.push_str(&section("CPU INFORMATION", style));
    let text = |s: &String| s.clone();
    line(
        &mut out,
        "Manufacturer",
        show(&id.manufacturer, style, text, no_color),
    );
    line(
        &mut out,
        "Processor",
        show(&id.model, style, text, no_color),
    );
    line(
        &mut out,
        "Architecture",
        show(&id.architecture, style, text, no_color),
    );
    line(
        &mut out,
        "Microarchitecture",
        show(&id.microarchitecture, style, text, no_color),
    );
    let count = |n: &usize| n.to_string();
    let split = id.performance_cores.get().zip(id.efficiency_cores.get());
    let cores = show(
        &id.physical_cores,
        style,
        |n| match split {
            Some((p, e)) => format!("{n} ({p} performance + {e} efficiency)"),
            None => n.to_string(),
        },
        no_color,
    );
    line(&mut out, "Physical Cores", cores);
    line(
        &mut out,
        "Logical Processors",
        show(&id.logical_processors, style, count, no_color),
    );
    line(
        &mut out,
        "Base Frequency",
        show(&id.base_frequency_mhz, style, mhz, no_color),
    );
    line(
        &mut out,
        "Max Boost Frequency",
        show(&id.max_boost_frequency_mhz, style, mhz, no_color),
    );
    let a = &r.activity;
    line(
        &mut out,
        "Current Frequency",
        show(&a.current_frequency_mhz, style, mhz, no_color),
    );
    line(
        &mut out,
        "CPU Utilization",
        show(&a.utilization_pct, style, percent, util_color),
    );

    // Temperature.
    let t = &r.temperature;
    out.push_str(&section("TEMPERATURE", style));
    line(
        &mut out,
        "CPU Temperature",
        show(&t.overall_c, style, celsius, temp_color),
    );
    line(
        &mut out,
        "Package",
        show(&t.package_c, style, celsius, temp_color),
    );
    line(
        &mut out,
        "Session Maximum",
        show(&t.session_max_c, style, celsius, temp_color),
    );
    line(
        &mut out,
        "Thermal Limit",
        show(&t.limit_c, style, celsius, no_color),
    );
    line(&mut out, "Sensors", show(&t.sensors, style, text, no_color));

    // Per-core table.
    if !a.cores.is_empty() {
        out.push_str(&section("PER-CORE", style));
        let lw = a
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
                &format!(
                    "{:<lw$}  {:>10}  {:>6}  {:>9}",
                    "CORE", "FREQUENCY", "LOAD", "TEMP"
                )
            )
        );
        for c in &a.cores {
            let freq = format!("{:>10}", cell(&c.frequency_mhz, mhz));
            let load = format!("{:>6}", cell(&c.utilization_pct, percent));
            let load = match c.utilization_pct.get() {
                Some(p) => paint(style, utilization_color(p), &load),
                None => load,
            };
            let temp = format!("{:>9}", cell(&c.temperature_c, celsius));
            let temp = match c.temperature_c.get() {
                Some(v) => paint(style, level_color(thresholds.classify(v)), &temp),
                None => temp,
            };
            let _ = writeln!(out, "{:<lw$}  {freq}  {load}  {temp}", c.label);
        }
        let src = |pick: &dyn Fn(&crate::report::cpu::CoreLine) -> Option<String>| {
            a.cores.iter().find_map(pick)
        };
        let sources: Vec<String> = [
            src(&|c| c.frequency_mhz.source().map(|s| format!("frequency: {s}"))),
            src(&|c| c.utilization_pct.source().map(|s| format!("load: {s}"))),
            src(&|c| {
                c.temperature_c
                    .source()
                    .map(|s| format!("temperature: {s}"))
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !sources.is_empty() {
            let _ = writeln!(
                out,
                "{}",
                paint(style, DIM, &format!("[{}]", sources.join(" · ")))
            );
        }
    }

    // Power and voltage.
    let p = &r.power;
    out.push_str(&section("POWER & VOLTAGE", style));
    line(
        &mut out,
        "Package Power",
        show(&p.package_w, style, watts, no_color),
    );
    line(
        &mut out,
        "Cores Power",
        show(&p.cores_w, style, watts, no_color),
    );
    line(
        &mut out,
        "Core Voltage",
        show(&p.voltage_v, style, |v| format!("{v:.3} V"), no_color),
    );
    let limits: Vec<String> = [("PL1", &p.pl1_w), ("PL2", &p.pl2_w), ("TDP", &p.tdp_w)]
        .iter()
        .filter_map(|(n, m)| m.get().map(|w| format!("{n} {w:.1} W")))
        .collect();
    if limits.is_empty() {
        line(
            &mut out,
            "Power Limits",
            show(&p.pl1_w, style, watts, no_color),
        );
    } else {
        let src = p.pl1_w.source().or(p.tdp_w.source()).unwrap_or_default();
        line(
            &mut out,
            "Power Limits",
            format!(
                "{}  {}",
                limits.join(" · "),
                paint(style, DIM, &format!("[{src}]"))
            ),
        );
        line(
            &mut out,
            "",
            paint(
                style,
                DIM,
                "values programmed in the CPU; firmware, power mode or battery may enforce lower limits",
            ),
        );
    }
    let history = if p.history_w.len() >= 2 {
        format!(
            "{}  min {:.1} · avg {:.1} · max {:.1} W ({} readings)",
            sparkline(&p.history_w),
            p.session.min.unwrap_or_default(),
            p.session.avg.unwrap_or_default(),
            p.session.max.unwrap_or_default(),
            p.session.samples
        )
    } else if watch.is_some() {
        paint(style, DIM, "collecting…")
    } else if p.package_w.is_measured() {
        paint(style, DIM, "run `cpu --watch` to record power over time")
    } else {
        paint(style, DIM, "Unavailable — package power is not readable")
    };
    line(&mut out, "Power History", history);

    // Throttling.
    let th = &r.throttling;
    out.push_str(&section("THROTTLING", style));
    line(
        &mut out,
        "Thermal Throttling",
        show(&th.thermal, style, throttle_text, throttle_color),
    );
    line(
        &mut out,
        "Power Throttling",
        show(&th.power, style, throttle_text, throttle_color),
    );
    line(
        &mut out,
        "PROCHOT Throttling",
        show(&th.prochot, style, throttle_text, throttle_color),
    );

    // Integrated GPU.
    out.push_str(&section("INTEGRATED GPU", style));
    let ig = &r.integrated_gpu;
    if let Some(e) = &ig.detection_error {
        line(
            &mut out,
            "Detection",
            paint(style, DIM, &format!("Unavailable — {e}")),
        );
    } else if !ig.detected {
        out.push_str("Integrated GPU: Not detected.\n");
    } else {
        for (i, d) in ig.devices.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            line(&mut out, "Detection", "Detected".to_string());
            render_gpu_fields(&mut out, d, thresholds, style);
        }
    }
    if ig.unclassified > 0 {
        line(
            &mut out,
            "",
            paint(
                style,
                DIM,
                &format!(
                    "{} graphics device(s) could not be confirmed as integrated or dedicated; see `gpu`",
                    ig.unclassified
                ),
            ),
        );
    }

    if !r.notes.is_empty() {
        out.push('\n');
        out.push_str(&render_notes(&r.notes, style));
    }
    let _ = writeln!(
        out,
        "\n{}",
        paint(
            style,
            DIM,
            &format!(
                "Temperature colours: elevated ≥{} °C · high ≥{} °C · critical ≥{} °C \
                 (configurable; not hardware limits)",
                thresholds.elevated_c, thresholds.high_c, thresholds.critical_c
            )
        )
    );
    out.push_str(&"=".repeat(WIDTH));
    out.push('\n');
    out
}

/// Renders the `gpu` report. `watch` is `(interval seconds, sample)`.
pub fn render_gpu_report(
    r: &GpuCommandReport,
    at: std::time::SystemTime,
    thresholds: &Thresholds,
    style: Style,
    watch: Option<(f64, u64)>,
) -> String {
    let mut out = banner("RUST HARDWARE MONITOR — GPU", style);
    let mut header = format!(
        "updated {} {} · RustTemp {VERSION}",
        format_clock(at, style.zone),
        zone_label(at, style.zone)
    );
    if let Some((secs, samples)) = watch {
        let _ = write!(
            header,
            " · refresh {secs} s · {samples} samples · Ctrl+C to stop"
        );
    }
    let _ = writeln!(out, "{}", paint(style, DIM, &header));

    if let Some(e) = &r.detection_error {
        out.push_str(&section("DEDICATED GPU", style));
        line(
            &mut out,
            "Detection",
            paint(style, DIM, &format!("Unavailable — {e}")),
        );
    } else if r.dedicated.is_empty() {
        out.push_str(&section("DEDICATED GPU", style));
        out.push_str("Dedicated GPU: Not detected.\n");
        if r.integrated_count > 0 && r.integrated.is_empty() {
            line(
                &mut out,
                "",
                paint(
                    style,
                    DIM,
                    "this computer's integrated GPU is reported by `cpu` (or `gpu --all`)",
                ),
            );
        }
    } else {
        let n = r.dedicated.len();
        for (i, g) in r.dedicated.iter().enumerate() {
            let title = if n > 1 {
                format!("DEDICATED GPU {} OF {n}", i + 1)
            } else {
                "DEDICATED GPU".to_string()
            };
            out.push_str(&section(&title, style));
            render_gpu_fields(&mut out, g, thresholds, style);
        }
    }
    for (i, g) in r.unclassified.iter().enumerate() {
        out.push_str(&section(&format!("UNCLASSIFIED GPU {}", i + 1), style));
        line(
            &mut out,
            "",
            paint(
                style,
                DIM,
                "could not be confirmed as integrated or dedicated; shown separately, not as a dedicated GPU",
            ),
        );
        render_gpu_fields(&mut out, g, thresholds, style);
    }
    for (i, g) in r.integrated.iter().enumerate() {
        out.push_str(&section(
            &format!("INTEGRATED GPU {} (shown with --all)", i + 1),
            style,
        ));
        render_gpu_fields(&mut out, g, thresholds, style);
    }
    for g in &r.software {
        out.push_str(&section(
            "SOFTWARE RENDERER (shown with --all; not a GPU)",
            style,
        ));
        line(
            &mut out,
            "Model",
            show(&g.model, style, |s| s.clone(), no_color),
        );
    }
    if !r.notes.is_empty() {
        out.push('\n');
        out.push_str(&render_notes(&r.notes, style));
    }
    let _ = writeln!(
        out,
        "\n{}",
        paint(
            style,
            DIM,
            &format!(
                "Temperature colours: elevated ≥{} °C · high ≥{} °C · critical ≥{} °C \
                 (configurable; not hardware limits)",
                thresholds.elevated_c, thresholds.high_c, thresholds.critical_c
            )
        )
    );
    out.push_str(&"=".repeat(WIDTH));
    out.push('\n');
    out
}

/// Formats a byte count as B / KiB / MiB / GiB.
pub fn fmt_bytes(b: u64) -> String {
    const KIB: f64 = 1024.0;
    let v = b as f64;
    if v >= KIB * KIB * KIB {
        format!("{:.2} GiB", v / (KIB * KIB * KIB))
    } else if v >= KIB * KIB {
        format!("{:.0} MiB", v / (KIB * KIB))
    } else if v >= KIB {
        format!("{:.0} KiB", v / KIB)
    } else {
        format!("{b} B")
    }
}

/// "X used of Y" from separate used / total measurements.
pub fn memory_text(used: &Measurement<u64>, total: &Measurement<u64>, style: Style) -> String {
    match (used, total) {
        (
            Measurement::Measured {
                value: u, source, ..
            },
            Measurement::Measured { value: t, .. },
        ) => {
            let avail = t.saturating_sub(*u);
            format!(
                "{} used of {} ({} free)  {}",
                fmt_bytes(*u),
                fmt_bytes(*t),
                fmt_bytes(avail),
                paint(style, DIM, &format!("[{source}]"))
            )
        }
        (
            Measurement::Unavailable { reason },
            Measurement::Measured {
                value: t, source, ..
            },
        ) => format!(
            "{} total  {} {}",
            fmt_bytes(*t),
            paint(style, DIM, &format!("[{source}]")),
            paint(style, DIM, &format!("· usage unavailable — {reason}"))
        ),
        (Measurement::Measured { .. }, _) => show(used, style, |b| fmt_bytes(*b), no_color),
        _ => show(total, style, |b| fmt_bytes(*b), no_color),
    }
}

/// Field lines for one GPU (shared by `cpu`'s iGPU section and `gpu`).
pub fn render_gpu_fields(out: &mut String, r: &GpuReport, thresholds: &Thresholds, style: Style) {
    let text = |s: &String| s.clone();
    let temp_color = |c: &f64| Some(level_color(thresholds.classify(*c)));
    let util_color = |p: &f64| Some(utilization_color(*p));
    let g = &r.sample;
    line(
        out,
        "Manufacturer",
        show(&r.manufacturer, style, text, no_color),
    );
    line(out, "Model", show(&r.model, style, text, no_color));
    line(
        out,
        "Architecture",
        show(&r.architecture, style, text, no_color),
    );
    if g.execution_units.is_measured() {
        line(
            out,
            "Execution Units",
            show(&g.execution_units, style, |n| n.to_string(), no_color),
        );
    }
    let ids = [
        r.pci_vendor_id.value().map(|v| format!("vendor {v}")),
        r.pci_device_id.value().map(|v| format!("device {v}")),
        r.pci_address.value().map(|v| format!("at {v}")),
    ];
    let ids: Vec<String> = ids.into_iter().flatten().collect();
    if !ids.is_empty() {
        line(out, "PCI Device", ids.join(" · "));
    }
    line(
        out,
        "Driver Version",
        show(&r.driver_version, style, text, no_color),
    );
    line(
        out,
        "GPU Utilization",
        show(&g.utilization_pct, style, percent, util_color),
    );
    line(
        out,
        "GPU Temperature",
        show(&g.temperature_c, style, celsius, temp_color),
    );
    line(
        out,
        "Core Frequency",
        show(&g.core_clock_mhz, style, mhz, no_color),
    );
    line(
        out,
        "Memory Frequency",
        show(&g.memory_clock_mhz, style, mhz, no_color),
    );
    line(
        out,
        "GPU Power",
        show(&g.power_w, style, |w| format!("{w:.2} W"), no_color),
    );
    if r.kind != GpuKind::Integrated || g.power_limit_w.is_measured() {
        line(
            out,
            "Power Limit",
            show(&g.power_limit_w, style, watts, no_color),
        );
    }
    line(
        out,
        "GPU Voltage",
        show(&g.voltage_v, style, |v| format!("{v:.3} V"), no_color),
    );
    if r.kind == GpuKind::Integrated {
        line(
            out,
            "Shared Memory",
            memory_text(
                &g.shared_memory_used_bytes,
                &g.shared_memory_total_bytes,
                style,
            ),
        );
        line(
            out,
            "Dedicated Memory",
            memory_text(
                &g.dedicated_memory_used_bytes,
                &g.dedicated_memory_total_bytes,
                style,
            ),
        );
    } else {
        line(
            out,
            "VRAM",
            memory_text(
                &g.dedicated_memory_used_bytes,
                &g.dedicated_memory_total_bytes,
                style,
            ),
        );
        line(
            out,
            "Shared Memory",
            memory_text(
                &g.shared_memory_used_bytes,
                &g.shared_memory_total_bytes,
                style,
            ),
        );
        line(
            out,
            "Memory Temperature",
            show(&g.memory_temperature_c, style, celsius, temp_color),
        );
        line(
            out,
            "Fan Speed",
            show(&g.fan_speed_rpm, style, |r| format!("{r:.0} RPM"), no_color),
        );
        line(
            out,
            "Fan Speed (%)",
            show(&g.fan_speed_pct, style, percent, no_color),
        );
        line(
            out,
            "Temperature Limit",
            show(&g.temperature_limit_c, style, celsius, no_color),
        );
    }
    line(
        out,
        "Memory Utilization",
        show(&r.memory_utilization_pct, style, percent, util_color),
    );
    let yes_no = |b: &bool| {
        if *b {
            "YES".to_string()
        } else {
            "No".to_string()
        }
    };
    let red = |b: &bool| b.then(|| level_color(Level::High));
    line(
        out,
        "Thermal Throttling",
        show(&g.thermal_throttling, style, yes_no, red),
    );
    line(
        out,
        "Power Throttling",
        show(&g.power_throttling, style, yes_no, red),
    );
    if !r.classification.is_empty() {
        let reasons: Vec<&str> = r.classification.iter().map(|e| e.signal.as_str()).collect();
        line(
            out,
            "Classified As",
            format!(
                "{}  {}",
                r.kind.describe(),
                paint(style, DIM, &format!("[{}]", reasons.join("; ")))
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Zone;

    #[test]
    fn bytes() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(128 << 20), "128 MiB");
        assert_eq!(fmt_bytes(3 << 30), "3.00 GiB");
    }

    #[test]
    fn memory_wording() {
        let st = Style::plain(Zone::Utc);
        let used = Measurement::measured(1u64 << 30, "B", "PDH");
        let total = Measurement::measured(4u64 << 30, "B", "D3DKMT");
        assert_eq!(
            memory_text(&used, &total, st),
            "1.00 GiB used of 4.00 GiB (3.00 GiB free)  [PDH]"
        );
        let none: Measurement<u64> = Measurement::unavailable("no counters");
        assert!(memory_text(&none, &total, st).starts_with("4.00 GiB total"));
        let na: Measurement<u64> = Measurement::not_applicable("software renderer");
        assert!(memory_text(&na, &na, st).starts_with("Not applicable"));
    }

    #[test]
    fn sparkline_scales() {
        assert_eq!(sparkline(&[0.0, 7.0]), "▁█");
        assert_eq!(sparkline(&[5.0, 5.0, 5.0]), "▄▄▄");
        assert_eq!(sparkline(&[1.0, 2.0, 3.0]).chars().count(), 3);
    }

    #[test]
    fn measurement_text() {
        let st = Style::plain(Zone::Utc);
        let m = Measurement::measured(45.25, "W", "RAPL");
        assert_eq!(show(&m, st, watts, no_color), "45.2 W  [RAPL]");
        let u: Measurement<f64> = Measurement::unavailable("needs administrator");
        assert_eq!(
            show(&u, st, watts, no_color),
            "Unavailable — needs administrator"
        );
        let n: Measurement<f64> = Measurement::not_applicable("uses system memory");
        assert_eq!(
            show(&n, st, watts, no_color),
            "Not applicable — uses system memory"
        );
        let zero = Measurement::measured(0.0, "%", "PDH");
        assert_eq!(show(&zero, st, percent, no_color), "0%  [PDH]");
    }

    #[test]
    fn continuation_line_has_no_colon() {
        let mut out = String::new();
        line(&mut out, "Power Limits", "PL1 82.0 W".into());
        line(&mut out, "", "caveat".into());
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("Power Limits:"));
        assert!(lines[1].trim_start().starts_with("caveat") && !lines[1].contains(':'));
    }

    #[test]
    fn throttle_wording() {
        let t = ThrottleState {
            now: Some(false),
            since_boot: Some(true),
        };
        assert_eq!(throttle_text(&t), "No (has occurred since boot)");
        let t = ThrottleState {
            now: None,
            since_boot: Some(false),
        };
        assert_eq!(throttle_text(&t), "Unknown (no live flag) (not since boot)");
    }

    #[test]
    fn two_dedicated_gpus_get_separate_numbered_sections() {
        use crate::hardware::gpu::{GpuKind, PciAddress, RawAdapter, identify};
        use crate::measurement::Measurement as M;
        use crate::report::gpu::{GpuCommandReport, device_report};
        use crate::sensors::gpu::GpuSample;
        let card = |bus: u32, temp: f64| {
            let mut d = identify(RawAdapter {
                name: Some(format!("Card on bus {bus}")),
                vendor_id: Some(0x10DE),
                pci: Some(PciAddress {
                    domain: 0,
                    bus,
                    device: 0,
                    function: 0,
                }),
                source: "test",
                ..Default::default()
            });
            d.kind = GpuKind::Discrete;
            let mut s = GpuSample::for_device(&d);
            s.temperature_c = M::measured(temp, "°C", "NVML");
            device_report(&d, Some(&s), None)
        };
        let r = GpuCommandReport {
            dedicated: vec![card(1, 55.0), card(2, 77.0)],
            unclassified: vec![],
            integrated: vec![],
            software: vec![],
            integrated_count: 0,
            software_count: 0,
            detection_error: None,
            notes: vec![],
        };
        let t = Thresholds::new(70.0, 85.0, 95.0).unwrap();
        let text = render_gpu_report(&r, std::time::UNIX_EPOCH, &t, Style::plain(Zone::Utc), None);
        let first = text.find("DEDICATED GPU 1 OF 2").expect(&text);
        let second = text.find("DEDICATED GPU 2 OF 2").expect(&text);
        assert!(first < second);
        // Each card's values stay in its own section.
        let (a, b) = text.split_at(second);
        assert!(a.contains("Card on bus 1") && a.contains("55.0 °C"), "{a}");
        assert!(b.contains("Card on bus 2") && b.contains("77.0 °C"), "{b}");
        assert!(!a.contains("77.0 °C") && !b.contains("55.0 °C"));
        assert!(!text.contains("Not detected"));
    }
}
