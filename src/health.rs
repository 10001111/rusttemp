//! CPU health summary derived from one monitoring snapshot.
//!
//! Only measured facts are reported: processor throttle flags, OS throttle
//! counters, distance to the hardware-reported limit, and how much of the
//! session was spent at each warning level. There is no invented "health
//! score": CPUs do not report wear or remaining lifespan.

use serde::Serialize;

use crate::monitoring::{Level, Snapshot};
use crate::sensors::ThrottleCounters;

/// Names of CPU sensors for which a condition holds.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Condition {
    /// Sensors where the condition is active now.
    pub now: Vec<String>,
    /// Sensors where the CPU has logged it since boot (or since software last
    /// cleared the flag).
    pub logged: Vec<String>,
}

/// Hottest CPU sensor and its margin to the hardware limit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hottest {
    pub label: String,
    pub temperature_c: f64,
    /// Hardware-reported critical limit (TjMax on Intel), if known.
    pub limit_c: Option<f64>,
    /// `limit_c - temperature_c`.
    pub headroom_c: Option<f64>,
}

/// Highest share of session time spent at or above a level, across CPU sensors.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LevelShare {
    pub level: Level,
    pub percent: f64,
    /// Sensor with that share (absent when the share is 0).
    pub sensor: Option<String>,
}

/// Session-wide observations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Session {
    pub samples: u64,
    /// Samples in which any CPU sensor reported active thermal throttling.
    pub thermal_throttle_samples: u64,
    /// Shares for ELEVATED, HIGH and CRITICAL.
    pub time_at_or_above: Vec<LevelShare>,
}

/// Everything the health section shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HealthSummary {
    /// True when the processor's throttle flags could be read.
    pub flags_available: bool,
    /// Number of CPU sensors that reported flags.
    pub flag_sensors: usize,
    pub thermal_throttling: Condition,
    pub prochot: Condition,
    pub critical_temperature: Condition,
    /// `None` when the CPU does not report power-limit status.
    pub power_limit_throttling: Option<Condition>,
    /// OS throttle counters (Linux), since boot.
    pub throttle_counters: Option<ThrottleCounters>,
    pub hottest_cpu_sensor: Option<Hottest>,
    pub session: Session,
}

/// Fold step keeping the first item with the largest value (ties keep the
/// earlier item, so a package sensor listed first wins over equal cores).
fn first_max<T>(best: Option<(T, f64)>, item: (T, f64)) -> Option<(T, f64)> {
    match best {
        Some(b) if b.1 >= item.1 => Some(b),
        _ => Some(item),
    }
}

/// Builds the summary for `snap`. Only CPU sensors are considered.
pub fn summarize(snap: &Snapshot<'_>) -> HealthSummary {
    let mut thermal = Condition::default();
    let mut prochot = Condition::default();
    let mut critical = Condition::default();
    let mut power: Option<Condition> = None;
    let mut flag_sensors = 0usize;

    for (i, info) in snap.sensors.iter().enumerate() {
        if !info.kind.is_cpu() {
            continue;
        }
        let Some(f) = snap.flags.get(i).copied().flatten() else {
            continue;
        };
        flag_sensors += 1;
        let name = || info.label.clone();
        let add = |c: &mut Condition, now: bool, logged: bool| {
            if now {
                c.now.push(name());
            }
            if logged {
                c.logged.push(name());
            }
        };
        add(&mut thermal, f.thermal_now, f.thermal_logged);
        add(&mut prochot, f.prochot_now, f.prochot_logged);
        add(&mut critical, f.critical_now, f.critical_logged);
        if let (Some(now), Some(logged)) = (f.power_limit_now, f.power_limit_logged) {
            let c = power.get_or_insert_with(Condition::default);
            add(c, now, logged);
        }
    }

    let hottest = snap
        .sensors
        .iter()
        .zip(snap.readings)
        .filter(|(s, _)| s.kind.is_cpu())
        .filter_map(|(s, r)| r.celsius().map(|c| (s, c)))
        .fold(None, first_max)
        .map(|(s, c)| Hottest {
            label: s.label.clone(),
            temperature_c: c,
            limit_c: s.limits.crit_c,
            headroom_c: s.limits.crit_c.map(|l| l - c),
        });

    let time_at_or_above = [Level::Elevated, Level::High, Level::Critical]
        .into_iter()
        .map(|level| {
            let best = snap
                .sensors
                .iter()
                .zip(snap.stats)
                .filter(|(s, _)| s.kind.is_cpu())
                .filter_map(|(s, st)| st.percent_at_least(level).map(|p| (s, p)))
                .fold(None, first_max);
            LevelShare {
                level,
                percent: best.map_or(0.0, |b| b.1),
                sensor: best.filter(|b| b.1 > 0.0).map(|b| b.0.label.clone()),
            }
        })
        .collect();

    HealthSummary {
        flags_available: flag_sensors > 0,
        flag_sensors,
        thermal_throttling: thermal,
        prochot,
        critical_temperature: critical,
        power_limit_throttling: power,
        throttle_counters: snap.counters,
        hottest_cpu_sensor: hottest,
        session: Session {
            samples: snap.sample,
            thermal_throttle_samples: snap.throttle_samples,
            time_at_or_above,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::{SensorStats, Thresholds};
    use crate::sensors::{HardwareLimits, Reading, SensorInfo, SensorKind, ThrottleFlags};
    use std::time::{Duration, UNIX_EPOCH};

    fn sensor(label: &str, kind: SensorKind, crit: Option<f64>) -> SensorInfo {
        SensorInfo {
            id: label.into(),
            label: label.into(),
            chip: "x".into(),
            kind,
            source: "x".into(),
            limits: HardwareLimits {
                max_c: None,
                crit_c: crit,
            },
        }
    }

    #[test]
    fn summarizes_flags_hottest_and_session() {
        let sensors = vec![
            sensor("Package id 0", SensorKind::CpuPackage, Some(100.0)),
            sensor("Core 0", SensorKind::CpuCore, Some(100.0)),
            sensor("Core 1", SensorKind::CpuCore, Some(100.0)),
            sensor("ACPI", SensorKind::AcpiThermalZone, None),
        ];
        let readings = vec![
            Reading::Celsius(91.0),
            Reading::Celsius(88.0),
            Reading::Celsius(91.0),
            Reading::Celsius(99.0), // non-CPU: never the "hottest CPU sensor"
        ];
        let flags = vec![
            Some(ThrottleFlags {
                thermal_logged: true,
                power_limit_now: Some(true),
                power_limit_logged: Some(true),
                ..Default::default()
            }),
            Some(ThrottleFlags {
                thermal_now: true,
                thermal_logged: true,
                power_limit_now: Some(false),
                power_limit_logged: Some(false),
                ..Default::default()
            }),
            Some(ThrottleFlags {
                power_limit_now: Some(false),
                power_limit_logged: Some(false),
                ..Default::default()
            }),
            None,
        ];
        let th = Thresholds::default();
        let at = UNIX_EPOCH + Duration::from_secs(1);
        let mut stats = vec![SensorStats::default(); 4];
        // Core 1: 4 samples, 1 at HIGH, 3 normal.
        for c in [50.0, 50.0, 50.0, 91.0] {
            stats[2].record(&Reading::Celsius(c), at);
            stats[2].record_level(th.classify(c));
        }
        let snap = Snapshot {
            backend: "t",
            at,
            elapsed: Duration::from_secs(4),
            sample: 4,
            sensors: &sensors,
            readings: &readings,
            stats: &stats,
            notes: &[],
            flags: &flags,
            counters: None,
            throttle_samples: 1,
            perf: None,
            perf_stats: &crate::perf::PerfStats::default(),
            gpu_devices: &[],
            gpu_samples: &[],
        };
        let h = summarize(&snap);
        assert!(h.flags_available);
        assert_eq!(h.flag_sensors, 3);
        assert_eq!(h.thermal_throttling.now, ["Core 0"]);
        assert_eq!(h.thermal_throttling.logged, ["Package id 0", "Core 0"]);
        assert!(h.prochot.now.is_empty() && h.prochot.logged.is_empty());
        let p = h.power_limit_throttling.unwrap();
        assert_eq!(p.now, ["Package id 0"]);
        let hot = h.hottest_cpu_sensor.unwrap();
        assert_eq!(hot.label, "Package id 0"); // first of the two 91 °C sensors
        assert_eq!(hot.headroom_c, Some(9.0));
        assert_eq!(h.session.thermal_throttle_samples, 1);
        let high = &h.session.time_at_or_above[1];
        assert_eq!(high.level, Level::High);
        assert_eq!(high.percent, 25.0);
        assert_eq!(high.sensor.as_deref(), Some("Core 1"));
        assert_eq!(h.session.time_at_or_above[2].sensor, None);
    }

    #[test]
    fn no_flags_no_cpu_sensors() {
        let sensors = vec![sensor("ACPI", SensorKind::AcpiThermalZone, Some(110.0))];
        let readings = vec![Reading::Celsius(60.0)];
        let stats = vec![SensorStats::default()];
        let snap = Snapshot {
            backend: "t",
            at: UNIX_EPOCH,
            elapsed: Duration::ZERO,
            sample: 1,
            sensors: &sensors,
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
        let h = summarize(&snap);
        assert!(!h.flags_available);
        assert_eq!(h.power_limit_throttling, None);
        assert_eq!(h.hottest_cpu_sensor, None);
        assert_eq!(h.session.time_at_or_above[0].percent, 0.0);
    }
}
