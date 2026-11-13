//! The `cpu` report: CPU identification, frequency and load, temperatures,
//! power and voltage, and throttling, as [`Measurement`]s.
//!
//! The report is plain data built from one monitoring [`Snapshot`] and the
//! detected [`CpuInfo`]. The terminal renderer and JSON output both use it,
//! and it can be unit-tested with synthetic snapshots.

use serde::Serialize;

use crate::cpuinfo::CpuInfo;
use crate::hardware::tables;
use crate::health;
use crate::measurement::Measurement;
use crate::monitoring::Snapshot;
use crate::perf::{PerfSample, RunningStat};
use crate::sensors::{Reading, SensorInfo, SensorKind};

type M<T> = Measurement<T>;

/// Complete CPU report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuReport {
    pub identification: CpuIdentification,
    pub activity: CpuActivity,
    pub temperature: CpuTemperature,
    pub power: CpuPower,
    pub throttling: CpuThrottling,
    pub integrated_gpu: crate::report::gpu::IntegratedGpuSection,
    /// Diagnostics, e.g. why admin-only values are unavailable.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuIdentification {
    pub manufacturer: M<String>,
    pub model: M<String>,
    pub architecture: M<String>,
    pub microarchitecture: M<String>,
    pub physical_cores: M<usize>,
    pub performance_cores: M<usize>,
    pub efficiency_cores: M<usize>,
    pub logical_processors: M<usize>,
    pub base_frequency_mhz: M<f64>,
    pub max_boost_frequency_mhz: M<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuActivity {
    /// Average clock of the cores that were active in the window.
    pub current_frequency_mhz: M<f64>,
    pub utilization_pct: M<f64>,
    pub cores: Vec<CoreLine>,
}

/// One physical core (or one temperature sensor without speed data).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoreLine {
    pub label: String,
    pub frequency_mhz: M<f64>,
    pub utilization_pct: M<f64>,
    pub temperature_c: M<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuTemperature {
    /// Package sensor, or the best available CPU-wide sensor.
    pub overall_c: M<f64>,
    pub package_c: M<f64>,
    /// Highest CPU temperature seen during this session.
    pub session_max_c: M<f64>,
    /// Hardware-reported limit (TjMax on Intel).
    pub limit_c: M<f64>,
    /// Which CPU temperature sensors are available.
    pub sensors: M<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuPower {
    pub package_w: M<f64>,
    pub cores_w: M<f64>,
    pub voltage_v: M<f64>,
    pub pl1_w: M<f64>,
    pub pl2_w: M<f64>,
    pub tdp_w: M<f64>,
    /// Session statistics of package power (min/max/avg/samples).
    pub session: RunningStat,
    /// Recent package-power readings, oldest first.
    pub history_w: Vec<f64>,
}

/// Throttling now / since boot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ThrottleState {
    /// Active right now (`None` when the source has no live flag).
    pub now: Option<bool>,
    /// Happened since boot (or since software last cleared the flag).
    pub since_boot: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CpuThrottling {
    pub thermal: M<ThrottleState>,
    pub power: M<ThrottleState>,
    pub prochot: M<ThrottleState>,
}

/// Prefix of the backend note explaining why CPU temperatures are missing.
const TEMP_NOTE_PREFIX: &str = "CPU core temperatures unavailable: ";

/// Why CPU-register values are unavailable, from the backend notes.
fn register_reason(snap: &Snapshot<'_>) -> String {
    snap.notes
        .iter()
        .find_map(|n| n.strip_prefix(TEMP_NOTE_PREFIX))
        .map(friendly)
        .unwrap_or_else(|| "no CPU temperature sensor was found".to_string())
}

/// Short per-field reason when PawnIO refused access (not elevated).
pub const ADMIN_REASON: &str = "needs administrator rights (processor registers via PawnIO)";

/// One note explaining why administrator rights matter, replacing the raw
/// "access denied" backend notes.
pub const ADMIN_NOTE: &str = "Run `cpu` from an administrator terminal to also see CPU \
    temperatures, power, voltage, power limits, max boost, throttling and integrated-GPU \
    power. Windows only \
    lets administrators read processor registers, which RustTemp does through the \
    PawnIO driver. Everything else works without administrator rights.";

fn is_access_denied(reason: &str) -> bool {
    reason.contains("PawnIO access denied")
}

/// Replaces the raw access-denied text with [`ADMIN_REASON`].
pub(crate) fn friendly(reason: &str) -> String {
    if is_access_denied(reason) {
        ADMIN_REASON.to_string()
    } else {
        reason.to_string()
    }
}

/// Backend notes with access-denied notes merged into [`ADMIN_NOTE`].
/// Note shown when the OS reports running inside a virtual machine.
pub const VM_NOTE: &str = "Running inside a virtual machine: core counts, caches and \
    frequencies describe the virtual CPU and may differ from the physical processor; \
    VMs usually expose no temperature or power sensors.";

fn report_notes(cpu: &CpuInfo, snap: &Snapshot<'_>) -> Vec<String> {
    let mut notes: Vec<String> = snap
        .notes
        .iter()
        .filter(|n| !is_access_denied(n))
        .cloned()
        .collect();
    if snap.notes.iter().any(|n| is_access_denied(n)) {
        notes.insert(0, ADMIN_NOTE.to_string());
    }
    if cpu.virtual_machine {
        notes.insert(0, VM_NOTE.to_string());
    }
    notes
}

fn identity_source(cpu: &CpuInfo) -> &'static str {
    if cpu.sources.contains(&"cpuid") {
        "CPUID"
    } else {
        "/proc/cpuinfo"
    }
}

fn topology_source(cpu: &CpuInfo) -> &'static str {
    if cpu.sources.contains(&"windows topology") {
        "Windows processor topology"
    } else {
        "/proc/cpuinfo"
    }
}

fn identification(
    cpu: &CpuInfo,
    perf: Option<&PerfSample>,
    snap: &Snapshot<'_>,
) -> CpuIdentification {
    let id_src = identity_source(cpu);
    let topo_src = topology_source(cpu);
    let not_reported = || "not reported by the system".to_string();

    let manufacturer = match (&cpu.vendor, &cpu.vendor_id) {
        (Some(v), Some(id)) if v != id => M::measured(format!("{v} ({id})"), "", id_src),
        (Some(v), _) => M::measured(v.clone(), "", id_src),
        _ => M::unavailable(not_reported()),
    };
    let microarchitecture = match (&cpu.vendor_id, cpu.family, cpu.model) {
        (Some(v), Some(f), Some(m)) => match tables::microarchitecture(v, f, m) {
            Some(name) => M::measured(
                name.to_string(),
                "",
                format!("{id_src} family {f} / model {m} (0x{m:X}) + model table"),
            ),
            None => M::unavailable(format!(
                "family {f} / model {m} (0x{m:X}) is not in RustTemp's model table"
            )),
        },
        _ => M::unavailable(not_reported()),
    };
    let (performance_cores, efficiency_cores) = match (
        cpu.physical_cores,
        cpu.performance_cores,
        cpu.efficiency_cores,
    ) {
        (_, Some(p), Some(e)) => (M::measured(p, "", topo_src), M::measured(e, "", topo_src)),
        (Some(_), _, _) => (
            M::not_applicable("all cores are the same type"),
            M::not_applicable("all cores are the same type"),
        ),
        _ => (
            M::unavailable(not_reported()),
            M::unavailable(not_reported()),
        ),
    };
    let base = match (cpu.base_mhz, cpu.base_mhz_source) {
        (Some(b), Some(src)) => M::measured(b as f64, "MHz", src),
        _ => match perf.and_then(|p| p.base_mhz) {
            Some(b) => M::measured(b, "MHz", "cpufreq"),
            None => M::unavailable(not_reported()),
        },
    };
    let max = match (cpu.max_mhz, cpu.max_mhz_source) {
        (Some(m), Some(src)) => M::measured(m as f64, "MHz", src),
        _ => match perf.and_then(|p| p.max_turbo_mhz) {
            Some(m) => M::measured(m, "MHz", "MSR_TURBO_RATIO_LIMIT via PawnIO"),
            None if snap.backend != crate::sensors::platform::WINDOWS_BACKEND => {
                M::unavailable("not reported by CPUID or cpufreq")
            }
            None => M::unavailable(match register_reason(snap) {
                r if r == ADMIN_REASON => {
                    "not reported by CPUID; reading it from the CPU needs administrator rights (PawnIO)"
                        .to_string()
                }
                r => format!("not reported by CPUID; reading it from the CPU failed: {r}"),
            }),
        },
    };

    CpuIdentification {
        manufacturer,
        model: M::from_option(cpu.brand.clone(), "", id_src, not_reported),
        architecture: M::measured(
            tables::instruction_set().to_string(),
            "",
            "build target / CPUID",
        ),
        microarchitecture,
        physical_cores: M::from_option(cpu.physical_cores, "", topo_src, not_reported),
        performance_cores,
        efficiency_cores,
        logical_processors: M::from_option(cpu.logical_processors, "", topo_src, not_reported),
        base_frequency_mhz: base,
        max_boost_frequency_mhz: max,
    }
}

fn reason_or(r: &Option<String>, default: &str) -> String {
    r.as_deref().map_or_else(|| default.to_string(), friendly)
}

/// Temperature of the CPU sensor with this exact label, if any.
fn temp_by_label(snap: &Snapshot<'_>, label: &str) -> Option<(f64, String)> {
    snap.sensors
        .iter()
        .zip(snap.readings)
        .find(|(s, _)| s.kind.is_cpu() && s.label == label)
        .and_then(|(s, r)| r.celsius().map(|c| (c, s.chip.clone())))
}

fn activity(snap: &Snapshot<'_>, perf: Option<&PerfSample>, temp_reason: &str) -> CpuActivity {
    let speed_reason = perf.map_or("no speed source on this platform".to_string(), |p| {
        reason_or(&p.speed_unavailable, "not reported")
    });
    let core_temp = |label: &str| match temp_by_label(snap, label) {
        Some((c, chip)) => M::measured(c, "°C", chip),
        None => M::unavailable(temp_reason.to_string()),
    };

    let mut cores: Vec<CoreLine> = Vec::new();
    if let Some(p) = perf {
        for c in &p.cores {
            cores.push(CoreLine {
                label: c.label.clone(),
                frequency_mhz: M::from_option(c.mhz, "MHz", p.speed_source.unwrap_or(""), || {
                    if p.speed_source.is_some() {
                        "core was idle for the whole measurement".to_string()
                    } else {
                        speed_reason.clone()
                    }
                }),
                utilization_pct: M::from_option(
                    c.load_pct,
                    "%",
                    p.load_source.unwrap_or(""),
                    || "not reported for this core".to_string(),
                ),
                temperature_c: core_temp(&c.label),
            });
        }
    }
    // Per-core temperature sensors that have no matching speed row.
    for (s, r) in snap.sensors.iter().zip(snap.readings) {
        if s.kind == SensorKind::CpuCore && !cores.iter().any(|c| c.label == s.label) {
            cores.push(CoreLine {
                label: s.label.clone(),
                frequency_mhz: M::unavailable(speed_reason.clone()),
                utilization_pct: M::unavailable("not reported for this sensor"),
                temperature_c: match r {
                    Reading::Celsius(c) => M::measured(*c, "°C", s.chip.clone()),
                    Reading::Unavailable(why) => M::unavailable(why.clone()),
                },
            });
        }
    }

    CpuActivity {
        current_frequency_mhz: match perf {
            Some(p) => M::from_option(p.avg_mhz, "MHz", p.speed_source.unwrap_or(""), || {
                speed_reason.clone()
            }),
            None => M::unavailable(speed_reason.clone()),
        },
        utilization_pct: match perf {
            Some(p) => M::from_option(p.total_load_pct, "%", p.load_source.unwrap_or(""), || {
                "measuring…".to_string()
            }),
            None => M::unavailable("no load source on this platform"),
        },
        cores,
    }
}

fn cpu_sensors<'a>(snap: &'a Snapshot<'a>) -> impl Iterator<Item = (usize, &'a SensorInfo)> + 'a {
    snap.sensors
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind.is_cpu())
}

fn temperature(snap: &Snapshot<'_>, reason: &str) -> CpuTemperature {
    let reading = |i: usize| snap.readings.get(i).and_then(Reading::celsius);
    let package = cpu_sensors(snap)
        .find(|(i, s)| s.kind == SensorKind::CpuPackage && reading(*i).is_some())
        .map(|(i, s)| (reading(i).unwrap(), s));
    // Overall: package; otherwise die, control, generic CPU, then hottest core.
    let overall = package.or_else(|| {
        [SensorKind::CpuDie, SensorKind::CpuControl, SensorKind::Cpu]
            .iter()
            .find_map(|k| {
                cpu_sensors(snap)
                    .find(|(i, s)| s.kind == *k && reading(*i).is_some())
                    .map(|(i, s)| (reading(i).unwrap(), s))
            })
            .or_else(|| {
                cpu_sensors(snap)
                    .filter_map(|(i, s)| reading(i).map(|c| (c, s)))
                    .max_by(|a, b| a.0.total_cmp(&b.0))
            })
    });
    let session_max = cpu_sensors(snap)
        .filter_map(|(i, s)| snap.stats.get(i).and_then(|st| st.max()).map(|m| (m, s)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let limit = cpu_sensors(snap)
        .filter_map(|(_, s)| s.limits.crit_c.map(|c| (c, s)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let count = cpu_sensors(snap).count();
    let chips: Vec<&str> = {
        let mut v: Vec<&str> = cpu_sensors(snap).map(|(_, s)| s.chip.as_str()).collect();
        v.dedup();
        v
    };

    let src = |s: &SensorInfo| format!("{}: {}", s.chip, s.label);
    CpuTemperature {
        overall_c: match overall {
            Some((c, s)) => M::measured(c, "°C", src(s)),
            None => M::unavailable(reason.to_string()),
        },
        package_c: match package {
            Some((c, s)) => M::measured(c, "°C", src(s)),
            None if count > 0 => M::unavailable("this CPU's driver exposes no package sensor"),
            None => M::unavailable(reason.to_string()),
        },
        session_max_c: match session_max {
            Some((m, s)) => M::measured(m, "°C", src(s)),
            None => M::unavailable(reason.to_string()),
        },
        limit_c: match limit {
            Some((c, s)) => M::measured(c, "°C", format!("{} (hardware-reported)", s.chip)),
            None if count > 0 => M::unavailable("the sensor driver reports no limit"),
            None => M::unavailable(reason.to_string()),
        },
        sensors: if count > 0 {
            M::measured(
                format!("{count} CPU sensor{}", if count == 1 { "" } else { "s" }),
                "",
                chips.join(", "),
            )
        } else {
            M::unavailable(reason.to_string())
        },
    }
}

fn power(snap: &Snapshot<'_>, perf: Option<&PerfSample>) -> CpuPower {
    let none = "no power source on this platform";
    let Some(p) = perf else {
        return CpuPower {
            package_w: M::unavailable(none),
            cores_w: M::unavailable(none),
            voltage_v: M::unavailable(none),
            pl1_w: M::unavailable(none),
            pl2_w: M::unavailable(none),
            tdp_w: M::unavailable(none),
            session: RunningStat::default(),
            history_w: Vec::new(),
        };
    };
    let power_reason = reason_or(&p.power_unavailable, "not reported");
    let limits_src = p.limits_source.unwrap_or("");
    let limit = |v: Option<f64>, name: &str| match v {
        Some(w) => M::measured(w, "W", limits_src),
        None if p.limits.any() => M::unavailable(format!("{name} is not reported")),
        None => M::unavailable(power_reason.clone()),
    };
    CpuPower {
        package_w: M::from_option(p.package_w, "W", p.power_source.unwrap_or(""), || {
            power_reason.clone()
        }),
        cores_w: M::from_option(p.cores_w, "W", p.power_source.unwrap_or(""), || {
            if p.package_w.is_some() {
                "this CPU does not report a cores-only counter".to_string()
            } else {
                power_reason.clone()
            }
        }),
        voltage_v: M::from_option(p.voltage_v, "V", p.voltage_source.unwrap_or(""), || {
            reason_or(&p.voltage_unavailable, "not reported")
        }),
        pl1_w: limit(p.limits.pl1_w, "PL1"),
        pl2_w: limit(p.limits.pl2_w, "PL2"),
        tdp_w: limit(p.limits.tdp_w, "TDP"),
        session: snap.perf_stats.package_w,
        history_w: snap.perf_stats.package_w_history.iter().copied().collect(),
    }
}

fn throttling(snap: &Snapshot<'_>, reason: &str) -> CpuThrottling {
    let h = health::summarize(snap);
    let flags_src = "CPU thermal status registers via PawnIO";
    let state = |c: &health::Condition| ThrottleState {
        now: Some(!c.now.is_empty()),
        since_boot: Some(!c.logged.is_empty()),
    };
    if h.flags_available {
        return CpuThrottling {
            thermal: M::measured(state(&h.thermal_throttling), "", flags_src),
            power: match &h.power_limit_throttling {
                Some(c) => M::measured(state(c), "", flags_src),
                None => M::unavailable("this CPU does not report power-limit status"),
            },
            prochot: M::measured(state(&h.prochot), "", flags_src),
        };
    }
    if let Some(c) = h.throttle_counters {
        let none = "Linux exposes no unprivileged power-throttling flag";
        return CpuThrottling {
            thermal: M::measured(
                ThrottleState {
                    now: None,
                    since_boot: Some(c.package_events > 0 || c.core_events_max > 0),
                },
                "",
                "Linux thermal_throttle counters",
            ),
            power: M::unavailable(none),
            prochot: M::unavailable(none),
        };
    }
    CpuThrottling {
        thermal: M::unavailable(reason.to_string()),
        power: M::unavailable(reason.to_string()),
        prochot: M::unavailable(reason.to_string()),
    }
}

/// Builds the CPU report from detected identity and one snapshot.
/// `gpu_error` explains why GPU detection could not run, if it failed.
pub fn build(cpu: &CpuInfo, snap: &Snapshot<'_>, gpu_error: Option<&str>) -> CpuReport {
    let perf = snap.perf;
    let reason = register_reason(snap);
    CpuReport {
        identification: identification(cpu, perf, snap),
        activity: activity(snap, perf, &reason),
        temperature: temperature(snap, &reason),
        power: power(snap, perf),
        throttling: throttling(snap, &reason),
        integrated_gpu: crate::report::gpu::integrated_section(snap, gpu_error),
        notes: report_notes(cpu, snap),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::SensorStats;
    use crate::perf::{CoreActivity, PerfStats, PowerLimits};
    use crate::sensors::{HardwareLimits, ThrottleFlags};
    use std::time::{Duration, UNIX_EPOCH};

    fn sensor(label: &str, kind: SensorKind, crit: Option<f64>) -> SensorInfo {
        SensorInfo {
            id: label.into(),
            label: label.into(),
            chip: "intel-dts (PawnIO)".into(),
            kind,
            source: "x".into(),
            limits: HardwareLimits {
                max_c: None,
                crit_c: crit,
            },
        }
    }

    fn cpu() -> CpuInfo {
        CpuInfo {
            vendor_id: Some("GenuineIntel".into()),
            vendor: Some("Intel".into()),
            brand: Some("13th Gen Intel(R) Core(TM) i7-13620H".into()),
            family: Some(6),
            model: Some(186),
            stepping: Some(2),
            physical_cores: Some(10),
            performance_cores: Some(6),
            efficiency_cores: Some(4),
            logical_processors: Some(16),
            base_mhz: Some(2400),
            base_mhz_source: Some("Windows power information (rated speed)"),
            sources: vec!["cpuid", "windows topology"],
            ..Default::default()
        }
    }

    struct Fixture {
        sensors: Vec<SensorInfo>,
        readings: Vec<Reading>,
        stats: Vec<SensorStats>,
        flags: Vec<Option<ThrottleFlags>>,
        perf: PerfSample,
        perf_stats: PerfStats,
        notes: Vec<String>,
        gpu_devices: Vec<crate::hardware::gpu::GpuDevice>,
        gpu_samples: Vec<crate::sensors::gpu::GpuSample>,
    }

    impl Fixture {
        fn snap(&self) -> Snapshot<'_> {
            Snapshot {
                backend: "test",
                at: UNIX_EPOCH,
                elapsed: Duration::from_secs(1),
                sample: 1,
                sensors: &self.sensors,
                readings: &self.readings,
                stats: &self.stats,
                notes: &self.notes,
                flags: &self.flags,
                counters: None,
                throttle_samples: 0,
                perf: Some(&self.perf),
                perf_stats: &self.perf_stats,
                gpu_devices: &self.gpu_devices,
                gpu_samples: &self.gpu_samples,
            }
        }
    }

    /// Elevated Windows session on the i7-13620H.
    fn elevated() -> Fixture {
        let sensors = vec![
            sensor("Package id 0", SensorKind::CpuPackage, Some(100.0)),
            sensor("Core 0 (P-core)", SensorKind::CpuCore, Some(100.0)),
            sensor("Core 1 (E-core)", SensorKind::CpuCore, Some(100.0)),
        ];
        let readings = vec![
            Reading::Celsius(75.0),
            Reading::Celsius(70.0),
            Reading::Celsius(66.0),
        ];
        let mut stats = vec![SensorStats::default(); 3];
        for (st, c) in stats.iter_mut().zip([81.0, 70.0, 66.0]) {
            st.record(&Reading::Celsius(c), UNIX_EPOCH);
        }
        let flag = Some(ThrottleFlags {
            thermal_logged: true,
            power_limit_now: Some(true),
            power_limit_logged: Some(true),
            ..Default::default()
        });
        let perf = PerfSample {
            cores: vec![
                CoreActivity {
                    label: "Core 0 (P-core)".into(),
                    mhz: Some(3600.0),
                    load_pct: Some(41.0),
                },
                CoreActivity {
                    label: "Core 1 (E-core)".into(),
                    mhz: None,
                    load_pct: Some(0.0),
                },
            ],
            avg_mhz: Some(3600.0),
            total_load_pct: Some(9.0),
            package_w: Some(17.0),
            cores_w: Some(11.3),
            limits: PowerLimits {
                pl1_w: Some(82.0),
                pl1_enabled: Some(true),
                pl2_w: Some(82.0),
                pl2_enabled: Some(true),
                tdp_w: Some(45.0),
            },
            max_turbo_mhz: Some(4900.0),
            speed_source: Some("APERF/MPERF via PawnIO"),
            load_source: Some("PDH"),
            power_source: Some("Intel RAPL via PawnIO"),
            limits_source: Some("CPU power-limit registers via PawnIO"),
            voltage_v: Some(0.885),
            voltage_source: Some("IA32_PERF_STATUS via PawnIO"),
            ..Default::default()
        };
        let mut perf_stats = PerfStats::default();
        perf_stats.record(&perf);
        Fixture {
            sensors,
            readings,
            stats,
            flags: vec![flag; 3],
            perf,
            perf_stats,
            notes: vec![],
            gpu_devices: vec![],
            gpu_samples: vec![],
        }
    }

    #[test]
    fn elevated_report_has_every_value_with_a_source() {
        let f = elevated();
        let r = build(&cpu(), &f.snap(), None);
        let id = &r.identification;
        assert_eq!(id.manufacturer.value().unwrap(), "Intel (GenuineIntel)");
        assert_eq!(id.microarchitecture.value().unwrap(), "Raptor Lake");
        assert_eq!(id.performance_cores.get(), Some(6));
        assert_eq!(id.base_frequency_mhz.get(), Some(2400.0));
        assert_eq!(id.max_boost_frequency_mhz.get(), Some(4900.0));
        assert_eq!(
            id.max_boost_frequency_mhz.source(),
            Some("MSR_TURBO_RATIO_LIMIT via PawnIO")
        );

        assert_eq!(r.activity.current_frequency_mhz.get(), Some(3600.0));
        assert_eq!(r.activity.cores[0].temperature_c.get(), Some(70.0));
        // Idle core: speed unavailable with a reason, load measured as a real 0.
        assert!(!r.activity.cores[1].frequency_mhz.is_measured());
        assert_eq!(r.activity.cores[1].utilization_pct.get(), Some(0.0));

        assert_eq!(r.temperature.overall_c.get(), Some(75.0));
        assert_eq!(r.temperature.package_c.get(), Some(75.0));
        assert_eq!(r.temperature.session_max_c.get(), Some(81.0));
        assert_eq!(r.temperature.limit_c.get(), Some(100.0));
        assert_eq!(r.temperature.sensors.value().unwrap(), "3 CPU sensors");

        assert_eq!(r.power.package_w.get(), Some(17.0));
        assert_eq!(r.power.voltage_v.get(), Some(0.885));
        assert_eq!(r.power.pl1_w.get(), Some(82.0));
        assert_eq!(r.power.history_w, [17.0]);

        let t = r.throttling.thermal.get().unwrap();
        assert_eq!((t.now, t.since_boot), (Some(false), Some(true)));
        assert_eq!(r.throttling.power.get().unwrap().now, Some(true));
    }

    #[test]
    fn unelevated_report_explains_instead_of_inventing() {
        let reason = "PawnIO access denied: run RustTemp from an administrator terminal";
        let f = Fixture {
            sensors: vec![SensorInfo {
                kind: SensorKind::AcpiThermalZone,
                ..sensor("\\_TZ.THRM", SensorKind::AcpiThermalZone, None)
            }],
            readings: vec![Reading::Celsius(64.0)],
            stats: vec![SensorStats::default()],
            flags: vec![],
            perf: PerfSample {
                cores: vec![CoreActivity {
                    label: "Core 0 (P-core)".into(),
                    mhz: Some(2061.0),
                    load_pct: Some(4.0),
                }],
                avg_mhz: Some(2061.0),
                total_load_pct: Some(14.0),
                speed_source: Some("Windows % Processor Performance"),
                load_source: Some("PDH"),
                power_unavailable: Some(reason.into()),
                voltage_unavailable: Some(reason.into()),
                ..Default::default()
            },
            perf_stats: PerfStats::default(),
            notes: vec![format!("{TEMP_NOTE_PREFIX}{reason}")],
            gpu_devices: vec![],
            gpu_samples: vec![],
        };
        let mut info = cpu();
        info.max_mhz = None;
        let mut snap = f.snap();
        snap.backend = crate::sensors::platform::WINDOWS_BACKEND;
        let r = build(&info, &snap, None);
        let reason = ADMIN_REASON;
        assert_eq!(r.notes, [ADMIN_NOTE]);

        // The ACPI zone is never used as the CPU temperature.
        assert_eq!(r.temperature.overall_c, M::unavailable(reason));
        assert_eq!(r.temperature.package_c, M::unavailable(reason));
        assert_eq!(r.power.package_w, M::unavailable(reason));
        assert_eq!(r.power.voltage_v, M::unavailable(reason));
        assert!(r.power.history_w.is_empty());
        assert_eq!(r.throttling.thermal, M::unavailable(reason));
        assert!(
            matches!(&r.identification.max_boost_frequency_mhz, M::Unavailable { reason: x } if x.contains("administrator"))
        );
        // What works without admin is still measured.
        assert_eq!(r.activity.utilization_pct.get(), Some(14.0));
        assert_eq!(r.activity.cores[0].frequency_mhz.get(), Some(2061.0));
        assert_eq!(r.activity.cores[0].temperature_c, M::unavailable(reason));
    }

    #[test]
    fn amd_style_sensors_and_no_perf() {
        let mut die = sensor("Tdie", SensorKind::CpuDie, None);
        die.chip = "k10temp".into();
        let mut ctl = sensor("Tctl", SensorKind::CpuControl, None);
        ctl.chip = "k10temp".into();
        let f = Fixture {
            sensors: vec![ctl, die],
            readings: vec![Reading::Celsius(70.0), Reading::Celsius(60.0)],
            stats: vec![SensorStats::default(); 2],
            flags: vec![],
            perf: PerfSample::default(),
            perf_stats: PerfStats::default(),
            notes: vec![],
            gpu_devices: vec![],
            gpu_samples: vec![],
        };
        let mut snap = f.snap();
        snap.perf = None;
        let mut info = cpu();
        info.vendor_id = Some("AuthenticAMD".into());
        info.family = Some(0x19);
        info.model = Some(0x21);
        info.performance_cores = None;
        info.efficiency_cores = None;
        let r = build(&info, &snap, None);
        // Tdie preferred over Tctl (which may carry an offset).
        assert_eq!(r.temperature.overall_c.get(), Some(60.0));
        assert!(matches!(r.temperature.package_c, M::Unavailable { .. }));
        assert_eq!(r.identification.microarchitecture.value().unwrap(), "Zen 3");
        assert!(matches!(
            r.identification.performance_cores,
            M::NotApplicable { .. }
        ));
        assert!(matches!(r.power.package_w, M::Unavailable { .. }));
        assert_eq!(
            r.identification.max_boost_frequency_mhz,
            M::unavailable("not reported by CPUID or cpufreq")
        );
        assert!(r.notes.is_empty());
        info.virtual_machine = true;
        assert_eq!(build(&info, &snap, None).notes, [VM_NOTE]);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["temperature"]["overall_c"]["status"], "measured");
        assert_eq!(json["power"]["package_w"]["status"], "unavailable");
    }
}
