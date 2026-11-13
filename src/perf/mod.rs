//! CPU speed, load and power.
//!
//! Sources:
//! - **Windows**: per-logical-CPU load from the PDH counter
//!   `\Processor Information(*)\% Processor Time` (no admin). Speed from
//!   APERF/MPERF through PawnIO when available (admin), otherwise from
//!   `% Processor Performance`. Package/core power, power limits, base and
//!   max turbo clock from Intel RAPL/platform MSRs through PawnIO (admin).
//! - **Linux**: load from `/proc/stat`, speed from `cpufreq/scaling_cur_freq`,
//!   power and limits from `/sys/class/powercap/intel-rapl:*` (energy
//!   counters are usually readable only by root).
//!
//! Rates (power, load, speed) need two readings; sources keep the previous
//! reading and report "measuring" until an interval has elapsed.

pub mod intel;
pub mod linux;
#[cfg(windows)]
pub mod windows;

use std::path::Path;

use serde::Serialize;

/// Minimum interval between readings for a rate to be computed.
pub const MIN_INTERVAL_S: f64 = 0.05;

/// Speed and load of one physical core.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoreActivity {
    /// Same labels as the temperature sensors where possible, e.g. `Core 3 (P-core)`.
    pub label: String,
    /// Effective clock while active, MHz.
    pub mhz: Option<f64>,
    /// Load of the core's busiest logical CPU, percent.
    pub load_pct: Option<f64>,
}

/// Power limits as reported by the CPU / OS.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct PowerLimits {
    /// Long-term limit (PL1).
    pub pl1_w: Option<f64>,
    pub pl1_enabled: Option<bool>,
    /// Short-term limit (PL2).
    pub pl2_w: Option<f64>,
    pub pl2_enabled: Option<bool>,
    /// Thermal design power reported by the CPU.
    pub tdp_w: Option<f64>,
}

impl PowerLimits {
    /// True when any limit is known.
    pub fn any(&self) -> bool {
        self.pl1_w.is_some() || self.pl2_w.is_some() || self.tdp_w.is_some()
    }
}

/// One reading of speed, load and power.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct PerfSample {
    pub cores: Vec<CoreActivity>,
    /// Mean effective clock of the cores that reported one, MHz.
    pub avg_mhz: Option<f64>,
    /// Whole-CPU load, percent.
    pub total_load_pct: Option<f64>,
    /// Package power, watts.
    pub package_w: Option<f64>,
    /// Cores-only power (Intel PP0 / powercap "core"), watts.
    pub cores_w: Option<f64>,
    pub limits: PowerLimits,
    pub base_mhz: Option<f64>,
    pub max_turbo_mhz: Option<f64>,
    /// Reference clock the APERF/MPERF ratio is scaled by (measured TSC rate).
    pub reference_mhz: Option<f64>,
    /// Where speed values come from.
    pub speed_source: Option<&'static str>,
    /// Where load values come from.
    pub load_source: Option<&'static str>,
    /// Where the power limits come from.
    pub limits_source: Option<&'static str>,
    /// Where power values come from.
    pub power_source: Option<&'static str>,
    /// Why speed is unavailable (or "measuring").
    pub speed_unavailable: Option<String>,
    /// Why power is unavailable (or "measuring").
    pub power_unavailable: Option<String>,
    /// Integrated-GPU power from the CPU's graphics power plane (Intel PP1), watts.
    pub graphics_w: Option<f64>,
    /// Why the graphics-plane power is unavailable.
    pub graphics_unavailable: Option<String>,
    /// Current core voltage, volts.
    pub voltage_v: Option<f64>,
    /// Where the voltage comes from.
    pub voltage_source: Option<&'static str>,
    /// Why the voltage is unavailable.
    pub voltage_unavailable: Option<String>,
}

impl PerfSample {
    /// Fills `avg_mhz` from the per-core values.
    pub fn compute_average(&mut self) {
        let v: Vec<f64> = self.cores.iter().filter_map(|c| c.mhz).collect();
        self.avg_mhz = (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    }

    /// True when there is anything worth displaying.
    pub fn has_data(&self) -> bool {
        !self.cores.is_empty()
            || self.package_w.is_some()
            || self.total_load_pct.is_some()
            || self.limits.any()
    }
}

/// A source of speed/load/power readings.
pub trait PerfSource {
    /// Takes a reading; rates cover the time since the previous call.
    fn sample(&mut self) -> PerfSample;
    /// Diagnostics from construction (e.g. why power is unavailable).
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Running min/max/mean of a value.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct RunningStat {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub avg: Option<f64>,
    pub samples: u64,
    #[serde(skip)]
    sum: f64,
}

impl RunningStat {
    /// Adds a value.
    pub fn record(&mut self, v: f64) {
        self.min = Some(self.min.map_or(v, |m| m.min(v)));
        self.max = Some(self.max.map_or(v, |m| m.max(v)));
        self.sum += v;
        self.samples += 1;
        self.avg = Some(self.sum / self.samples as f64);
    }
}

/// Number of recent package-power samples kept for the history display.
pub const POWER_HISTORY_LEN: usize = 60;

/// Session statistics for speed, load and power.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct PerfStats {
    pub package_w: RunningStat,
    /// Most recent package-power readings (oldest first, at most
    /// [`POWER_HISTORY_LEN`]). Samples without a reading are skipped.
    pub package_w_history: std::collections::VecDeque<f64>,
    pub total_load_pct: RunningStat,
    /// Highest per-core clock seen and on which core.
    pub peak_mhz: Option<f64>,
    pub peak_core: Option<String>,
}

impl PerfStats {
    /// Adds one sample.
    pub fn record(&mut self, s: &PerfSample) {
        if let Some(w) = s.package_w {
            self.package_w.record(w);
            if self.package_w_history.len() == POWER_HISTORY_LEN {
                self.package_w_history.pop_front();
            }
            self.package_w_history.push_back(w);
        }
        if let Some(l) = s.total_load_pct {
            self.total_load_pct.record(l);
        }
        for c in &s.cores {
            if let Some(m) = c.mhz {
                if self.peak_mhz.is_none_or(|p| m > p) {
                    self.peak_mhz = Some(m);
                    self.peak_core = Some(c.label.clone());
                }
            }
        }
    }
}

/// Creates the default source for this OS, or a sysfs-tree source when
/// `sysfs_root` is given. `None` when no source exists for the platform.
pub fn default_source(sysfs_root: Option<&Path>) -> Option<Box<dyn PerfSource>> {
    if let Some(root) = sysfs_root {
        return Some(Box::new(linux::LinuxPerf::new(root)));
    }
    #[cfg(target_os = "linux")]
    {
        Some(Box::new(linux::LinuxPerf::new(Path::new("/"))))
    }
    #[cfg(windows)]
    {
        Some(Box::new(windows::WindowsPerf::new()))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_stat() {
        let mut r = RunningStat::default();
        assert_eq!(r.avg, None);
        for v in [10.0, 30.0, 20.0] {
            r.record(v);
        }
        assert_eq!(
            (r.min, r.max, r.avg, r.samples),
            (Some(10.0), Some(30.0), Some(20.0), 3)
        );
    }

    #[test]
    fn perf_stats_track_peak_core() {
        let mut st = PerfStats::default();
        let core = |l: &str, m: Option<f64>| CoreActivity {
            label: l.into(),
            mhz: m,
            load_pct: None,
        };
        let mut s = PerfSample {
            cores: vec![
                core("Core 0", Some(3000.0)),
                core("Core 1", Some(4100.0)),
                core("Core 2", None),
            ],
            package_w: Some(20.0),
            total_load_pct: Some(50.0),
            ..Default::default()
        };
        s.compute_average();
        assert_eq!(s.avg_mhz, Some(3550.0));
        st.record(&s);
        s.cores[0].mhz = Some(4500.0);
        s.package_w = Some(40.0);
        st.record(&s);
        assert_eq!(st.peak_mhz, Some(4500.0));
        assert_eq!(st.peak_core.as_deref(), Some("Core 0"));
        assert_eq!(st.package_w.avg, Some(30.0));
        assert_eq!(st.package_w_history, [20.0, 40.0]);
        assert!(s.has_data());
        assert!(!PerfSample::default().has_data());
    }

    #[test]
    fn power_history_is_bounded_and_skips_missing_readings() {
        let mut st = PerfStats::default();
        for i in 0..(POWER_HISTORY_LEN + 5) {
            st.record(&PerfSample {
                package_w: Some(i as f64),
                ..Default::default()
            });
        }
        st.record(&PerfSample::default()); // no reading: not recorded as 0
        assert_eq!(st.package_w_history.len(), POWER_HISTORY_LEN);
        assert_eq!(st.package_w_history.front(), Some(&5.0));
        assert_eq!(
            st.package_w_history.back(),
            Some(&((POWER_HISTORY_LEN + 4) as f64))
        );
    }
}
