//! JSON serialization.
//!
//! Unavailable values are emitted as `null` with a `status` and `error`
//! explaining why; no placeholder numbers are ever substituted.

use serde::Serialize;

use crate::cpuinfo::CpuInfo;
use crate::health::{self, HealthSummary};
use crate::monitoring::{Level, SensorStats, Snapshot, Thresholds};
use crate::sensors::{HardwareLimits, Reading, SensorInfo, SensorKind, ThrottleFlags};
use crate::time::{format_rfc3339_local, format_rfc3339_utc, unix_millis};

/// Version string included in every document.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Statement attached to the thresholds in JSON and terminal output.
pub const THRESHOLD_NOTICE: &str =
    "user-configurable warning thresholds; not processor-specific or hardware-certified limits";

#[derive(Serialize)]
struct SensorIdentity<'a> {
    id: &'a str,
    label: &'a str,
    chip: &'a str,
    kind: SensorKind,
    kind_description: &'static str,
    is_cpu: bool,
    source: &'a str,
    hardware_limits: Limits,
}

#[derive(Serialize)]
struct Limits {
    max_c: Option<f64>,
    crit_c: Option<f64>,
}

impl From<HardwareLimits> for Limits {
    fn from(l: HardwareLimits) -> Self {
        Limits {
            max_c: l.max_c.map(round3),
            crit_c: l.crit_c.map(round3),
        }
    }
}

impl<'a> From<&'a SensorInfo> for SensorIdentity<'a> {
    fn from(s: &'a SensorInfo) -> Self {
        SensorIdentity {
            id: &s.id,
            label: &s.label,
            chip: &s.chip,
            kind: s.kind,
            kind_description: s.kind.describe(),
            is_cpu: s.kind.is_cpu(),
            source: &s.source,
            hardware_limits: s.limits.into(),
        }
    }
}

#[derive(Serialize)]
struct SensorList<'a> {
    rusttemp_version: &'static str,
    backend: &'a str,
    sensor_count: usize,
    cpu_sensor_count: usize,
    notes: &'a [String],
    sensors: Vec<SensorIdentity<'a>>,
}

#[derive(Serialize)]
struct ThresholdsJson {
    elevated_c: f64,
    high_c: f64,
    critical_c: f64,
    note: &'static str,
}

#[derive(Serialize)]
struct LevelSamples {
    normal: u64,
    elevated: u64,
    high: u64,
    critical: u64,
}

#[derive(Serialize)]
struct StatsJson {
    min_c: Option<f64>,
    max_c: Option<f64>,
    avg_c: Option<f64>,
    valid_samples: u64,
    unavailable_samples: u64,
    last_valid_update: Option<String>,
    level_samples: LevelSamples,
    thermal_throttle_samples: u64,
}

impl From<&SensorStats> for StatsJson {
    fn from(s: &SensorStats) -> Self {
        StatsJson {
            min_c: s.min().map(round3),
            max_c: s.max().map(round3),
            avg_c: s.average().map(round3),
            valid_samples: s.valid_samples(),
            unavailable_samples: s.unavailable_samples(),
            last_valid_update: s.last_valid_at().map(format_rfc3339_utc),
            level_samples: {
                let [normal, elevated, high, critical] = s.level_samples();
                LevelSamples {
                    normal,
                    elevated,
                    high,
                    critical,
                }
            },
            thermal_throttle_samples: s.thermal_throttle_samples(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ok,
    Unavailable,
}

#[derive(Serialize)]
struct SensorReading<'a> {
    #[serde(flatten)]
    identity: SensorIdentity<'a>,
    unit: &'static str,
    status: Status,
    temperature_c: Option<f64>,
    error: Option<&'a str>,
    level: Option<Level>,
    /// `hardware_limits.crit_c - temperature_c`.
    headroom_c: Option<f64>,
    /// Processor throttle flags (null when the source has none).
    throttle: Option<ThrottleFlags>,
    stats: StatsJson,
}

#[derive(Serialize)]
struct CpuBrief<'a> {
    vendor: Option<&'a str>,
    brand: Option<&'a str>,
}

#[derive(Serialize)]
struct SnapshotJson<'a> {
    rusttemp_version: &'static str,
    backend: &'a str,
    timestamp: String,
    timestamp_local: Option<String>,
    timestamp_unix_ms: u64,
    sample: u64,
    monitoring_duration_s: f64,
    thresholds: ThresholdsJson,
    cpu: Option<CpuBrief<'a>>,
    health: HealthSummary,
    perf: Option<&'a crate::perf::PerfSample>,
    perf_session: Option<&'a crate::perf::PerfStats>,
    notes: &'a [String],
    sensors: Vec<SensorReading<'a>>,
}

#[derive(Serialize)]
struct InfoJson<'a> {
    rusttemp_version: &'static str,
    cpu: &'a CpuInfo,
    /// Highest hardware-reported limit among CPU sensors (TjMax on Intel).
    thermal_limit_c: Option<f64>,
    health: HealthSummary,
    perf: Option<&'a crate::perf::PerfSample>,
    notes: &'a [String],
}

/// Rounds to 3 decimal places (sysfs resolution is 1 m°C).
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// JSON document describing discovered sensors.
pub fn sensor_list_json(
    backend: &str,
    sensors: &[SensorInfo],
    notes: &[String],
) -> serde_json::Result<String> {
    serde_json::to_string_pretty(&SensorList {
        rusttemp_version: VERSION,
        backend,
        sensor_count: sensors.len(),
        cpu_sensor_count: sensors.iter().filter(|s| s.kind.is_cpu()).count(),
        notes,
        sensors: sensors.iter().map(SensorIdentity::from).collect(),
    })
}

/// JSON document for one sample. `pretty` selects indented output; compact
/// output is used for line-delimited streaming.
pub fn snapshot_json(
    snap: &Snapshot<'_>,
    thresholds: &Thresholds,
    cpu: Option<&CpuInfo>,
    pretty: bool,
) -> serde_json::Result<String> {
    let sensors = snap
        .sensors
        .iter()
        .zip(snap.readings)
        .zip(snap.stats)
        .enumerate()
        .map(|(i, ((info, reading), stats))| {
            let (status, temperature_c, error) = match reading {
                Reading::Celsius(c) => (Status::Ok, Some(round3(*c)), None),
                Reading::Unavailable(why) => (Status::Unavailable, None, Some(why.as_str())),
            };
            SensorReading {
                identity: info.into(),
                unit: "celsius",
                status,
                temperature_c,
                error,
                level: reading.celsius().map(|c| thresholds.classify(c)),
                headroom_c: reading
                    .celsius()
                    .zip(info.limits.crit_c)
                    .map(|(c, l)| round3(l - c)),
                throttle: snap.flags.get(i).copied().flatten(),
                stats: stats.into(),
            }
        })
        .collect();
    let doc = SnapshotJson {
        rusttemp_version: VERSION,
        backend: snap.backend,
        timestamp: format_rfc3339_utc(snap.at),
        timestamp_local: format_rfc3339_local(snap.at),
        timestamp_unix_ms: unix_millis(snap.at),
        sample: snap.sample,
        monitoring_duration_s: round3(snap.elapsed.as_secs_f64()),
        thresholds: ThresholdsJson {
            elevated_c: thresholds.elevated_c,
            high_c: thresholds.high_c,
            critical_c: thresholds.critical_c,
            note: THRESHOLD_NOTICE,
        },
        cpu: cpu.map(|c| CpuBrief {
            vendor: c.vendor.as_deref(),
            brand: c.brand.as_deref(),
        }),
        health: health::summarize(snap),
        perf: snap.perf,
        perf_session: snap.perf.map(|_| snap.perf_stats),
        notes: snap.notes,
        sensors,
    };
    if pretty {
        serde_json::to_string_pretty(&doc)
    } else {
        serde_json::to_string(&doc)
    }
}

/// `--info --json` document.
pub fn info_json(cpu: &CpuInfo, snap: &Snapshot<'_>) -> serde_json::Result<String> {
    let thermal_limit_c = snap
        .sensors
        .iter()
        .filter(|s| s.kind.is_cpu())
        .filter_map(|s| s.limits.crit_c)
        .reduce(f64::max);
    serde_json::to_string_pretty(&InfoJson {
        rusttemp_version: VERSION,
        cpu,
        thermal_limit_c,
        health: health::summarize(snap),
        perf: snap.perf,
        notes: snap.notes,
    })
}

#[derive(Serialize)]
struct CpuReportJson<'a> {
    rusttemp_version: &'static str,
    command: &'static str,
    timestamp: String,
    timestamp_local: Option<String>,
    sample: u64,
    thresholds: ThresholdsJson,
    #[serde(flatten)]
    report: &'a crate::report::cpu::CpuReport,
}

/// JSON document for the `cpu` command (compact when `pretty` is false).
pub fn cpu_report_json(
    report: &crate::report::cpu::CpuReport,
    at: std::time::SystemTime,
    sample: u64,
    thresholds: &Thresholds,
    pretty: bool,
) -> serde_json::Result<String> {
    let doc = CpuReportJson {
        rusttemp_version: VERSION,
        command: "cpu",
        timestamp: format_rfc3339_utc(at),
        timestamp_local: format_rfc3339_local(at),
        sample,
        thresholds: ThresholdsJson {
            elevated_c: thresholds.elevated_c,
            high_c: thresholds.high_c,
            critical_c: thresholds.critical_c,
            note: THRESHOLD_NOTICE,
        },
        report,
    };
    if pretty {
        serde_json::to_string_pretty(&doc)
    } else {
        serde_json::to_string(&doc)
    }
}

#[derive(Serialize)]
struct GpuReportJson<'a> {
    rusttemp_version: &'static str,
    command: &'static str,
    timestamp: String,
    timestamp_local: Option<String>,
    sample: u64,
    #[serde(flatten)]
    report: &'a crate::report::gpu::GpuCommandReport,
}

/// JSON document for the `gpu` command (compact when `pretty` is false).
pub fn gpu_report_json(
    report: &crate::report::gpu::GpuCommandReport,
    at: std::time::SystemTime,
    sample: u64,
    pretty: bool,
) -> serde_json::Result<String> {
    let doc = GpuReportJson {
        rusttemp_version: VERSION,
        command: "gpu",
        timestamp: format_rfc3339_utc(at),
        timestamp_local: format_rfc3339_local(at),
        sample,
        report,
    };
    if pretty {
        serde_json::to_string_pretty(&doc)
    } else {
        serde_json::to_string(&doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::{Duration, UNIX_EPOCH};

    fn sensors() -> Vec<SensorInfo> {
        vec![
            SensorInfo {
                id: "hwmon1/temp1".into(),
                label: "Package id 0".into(),
                chip: "coretemp".into(),
                kind: SensorKind::CpuPackage,
                source: "/sys/class/hwmon/hwmon1/temp1_input".into(),
                limits: HardwareLimits {
                    max_c: Some(100.0),
                    crit_c: None,
                },
            },
            SensorInfo {
                id: "hwmon0/temp1".into(),
                label: "temp1".into(),
                chip: "acpitz".into(),
                kind: SensorKind::AcpiThermalZone,
                source: "/sys/class/hwmon/hwmon0/temp1_input".into(),
                limits: HardwareLimits::default(),
            },
        ]
    }

    #[test]
    fn sensor_list_is_valid_json() {
        let s = sensors();
        let v: Value = serde_json::from_str(
            &sensor_list_json("linux-sysfs", &s, &["note".to_string()]).unwrap(),
        )
        .unwrap();
        assert_eq!(v["sensor_count"], 2);
        assert_eq!(v["cpu_sensor_count"], 1);
        assert_eq!(v["sensors"][0]["kind"], "cpu_package");
        assert_eq!(v["sensors"][0]["is_cpu"], true);
        assert_eq!(v["sensors"][0]["hardware_limits"]["max_c"], 100.0);
        assert!(v["sensors"][0]["hardware_limits"]["crit_c"].is_null());
        assert_eq!(v["sensors"][1]["kind"], "acpi_thermal_zone");
        assert_eq!(v["sensors"][1]["is_cpu"], false);
    }

    #[test]
    fn snapshot_represents_unavailable_as_null() {
        let s = sensors();
        let readings = vec![
            Reading::Celsius(72.5),
            Reading::Unavailable("read failed".into()),
        ];
        let at = UNIX_EPOCH + Duration::from_millis(1_794_476_040_000);
        let mut stats = vec![SensorStats::default(); 2];
        stats[0].record(&readings[0], at);
        stats[1].record(&readings[1], at);
        let snap = Snapshot {
            backend: "test",
            at,
            elapsed: Duration::from_millis(1500),
            sample: 1,
            sensors: &s,
            readings: &readings,
            stats: &stats,
            notes: &[],
            flags: &[],
            counters: None,
            throttle_samples: 0,
            perf: None,
            perf_stats: &crate::perf::PerfStats::default(),
            gpu_devices: &[],
            gpu_samples: &[],
        };
        let text = snapshot_json(&snap, &Thresholds::default(), None, false).unwrap();
        assert!(!text.contains('\n'), "compact output must be a single line");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["timestamp"], "2026-11-12T09:34:00.000Z");
        assert_eq!(v["timestamp_unix_ms"], 1_794_476_040_000u64);
        assert_eq!(v["monitoring_duration_s"], 1.5);
        assert_eq!(v["thresholds"]["elevated_c"], 70.0);
        assert!(
            v["thresholds"]["note"]
                .as_str()
                .unwrap()
                .contains("not processor-specific")
        );

        let ok = &v["sensors"][0];
        assert_eq!(ok["status"], "ok");
        assert_eq!(ok["temperature_c"], 72.5);
        assert_eq!(ok["unit"], "celsius");
        assert_eq!(ok["level"], "elevated");
        assert!(ok["error"].is_null());
        assert_eq!(ok["stats"]["avg_c"], 72.5);
        assert_eq!(ok["stats"]["last_valid_update"], "2026-11-12T09:34:00.000Z");

        let bad = &v["sensors"][1];
        assert_eq!(bad["status"], "unavailable");
        assert!(bad["temperature_c"].is_null());
        assert!(bad["level"].is_null());
        assert_eq!(bad["error"], "read failed");
        assert!(bad["stats"]["min_c"].is_null());
        assert_eq!(bad["stats"]["unavailable_samples"], 1);
    }

    #[test]
    fn rounding() {
        assert_eq!(round3(50.033_333_3), 50.033);
        assert_eq!(round3(61.125), 61.125);
    }
}
