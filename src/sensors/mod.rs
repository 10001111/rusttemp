//! Sensor model, classification and platform backends.

pub mod classify;
pub mod gpu;
pub mod platform;

use serde::Serialize;

/// Readings outside this range (°C) are reported as unavailable instead of
/// being displayed. Disconnected or unpopulated sensors commonly report
/// sentinel values such as -273.15 °C (0 K), -128 °C or 6553.5 °C.
pub const PLAUSIBLE_RANGE_C: std::ops::RangeInclusive<f64> = -55.0..=150.0;

/// What a sensor is known to measure. CPU kinds are only assigned when the
/// reporting driver documents the sensor as a CPU sensor; everything else is
/// `AcpiThermalZone` or `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    /// Whole-package temperature (Intel `coretemp` "Package id N", `x86_pkg_temp`).
    CpuPackage,
    /// Per-core digital thermal sensor (Intel `coretemp` "Core N").
    CpuCore,
    /// Die / chiplet temperature (AMD "Tdie", "TccdN").
    CpuDie,
    /// AMD control temperature "Tctl": used for fan control and may include
    /// a model-specific offset above the real die temperature.
    CpuControl,
    /// A sensor its driver identifies as a CPU sensor without finer detail.
    Cpu,
    /// ACPI firmware thermal zone. Its physical location is defined by the
    /// firmware and is not guaranteed to be the CPU.
    AcpiThermalZone,
    /// Any other temperature sensor (GPU, NVMe, chipset, Wi-Fi, ...).
    Other,
}

impl SensorKind {
    /// True for sensors positively identified as measuring the CPU.
    pub fn is_cpu(self) -> bool {
        matches!(
            self,
            SensorKind::CpuPackage
                | SensorKind::CpuCore
                | SensorKind::CpuDie
                | SensorKind::CpuControl
                | SensorKind::Cpu
        )
    }

    /// Short human-readable description.
    pub fn describe(self) -> &'static str {
        match self {
            SensorKind::CpuPackage => "CPU package",
            SensorKind::CpuCore => "CPU core",
            SensorKind::CpuDie => "CPU die",
            SensorKind::CpuControl => "CPU control (Tctl, may be offset)",
            SensorKind::Cpu => "CPU",
            SensorKind::AcpiThermalZone => "ACPI thermal zone (location not confirmed)",
            SensorKind::Other => "other / unclassified",
        }
    }
}

/// Limits reported by the hardware or driver itself (not by RustTemp).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct HardwareLimits {
    /// Driver-reported maximum / high temperature, if any.
    pub max_c: Option<f64>,
    /// Driver-reported critical temperature, if any.
    pub crit_c: Option<f64>,
}

/// Static identity of a discovered sensor.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorInfo {
    /// Identifier unique within one discovery run, e.g. `hwmon3/temp1`.
    pub id: String,
    /// Driver-provided label, or a generated one such as `temp1`.
    pub label: String,
    /// Reporting chip / driver, e.g. `coretemp`, `k10temp`, `acpitz`.
    pub chip: String,
    /// Classification.
    pub kind: SensorKind,
    /// Where the value is read from (file path or OS counter path).
    pub source: String,
    /// Hardware-reported limits.
    pub limits: HardwareLimits,
}

/// One measurement attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// A valid temperature in degrees Celsius.
    Celsius(f64),
    /// No valid value could be obtained; the string explains why.
    Unavailable(String),
}

impl Reading {
    /// Builds a reading from a converted value, rejecting implausible values.
    pub fn from_celsius(c: f64) -> Reading {
        if c.is_finite() && PLAUSIBLE_RANGE_C.contains(&c) {
            Reading::Celsius(c)
        } else {
            Reading::Unavailable(format!("implausible value reported ({c} °C)"))
        }
    }

    /// The temperature, if valid.
    pub fn celsius(&self) -> Option<f64> {
        match self {
            Reading::Celsius(c) => Some(*c),
            Reading::Unavailable(_) => None,
        }
    }
}

/// A source of temperature readings.
///
/// Discovery happens when the backend is constructed; `read` re-samples every
/// discovered sensor.
pub trait SensorBackend {
    /// Short name of the backend, e.g. `linux-sysfs`.
    fn name(&self) -> &'static str;
    /// Sensors found during discovery.
    fn sensors(&self) -> &[SensorInfo];
    /// Samples every sensor. Returns exactly one reading per entry of
    /// [`sensors`](Self::sensors), in the same order.
    fn read(&mut self) -> Vec<Reading>;
    /// Diagnostic notes gathered during discovery, such as why a sensor
    /// source is unavailable.
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
    /// Throttle flags captured by the most recent [`read`](Self::read), one
    /// entry per sensor (same order), `None` where the source has no flags.
    /// An empty vector means the backend reports no flags at all.
    fn throttle_flags(&self) -> Vec<Option<ThrottleFlags>> {
        Vec::new()
    }
    /// OS-level throttle event counters captured by the most recent read.
    fn throttle_counters(&self) -> Option<ThrottleCounters> {
        None
    }
}

/// Throttling status reported by the processor for one sensor (Intel
/// `IA32_THERM_STATUS` / `IA32_PACKAGE_THERM_STATUS`).
///
/// `*_now` fields are live status. `*_logged` fields are sticky: the CPU sets
/// them when the condition occurs and they stay set until power-on reset or
/// until software clears them, so they mean "happened at least once since
/// boot or since another program last cleared the flag".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ThrottleFlags {
    /// Thermal control circuit active: the CPU is reducing speed due to heat.
    pub thermal_now: bool,
    pub thermal_logged: bool,
    /// PROCHOT# / FORCEPR# asserted (throttling requested by the platform,
    /// e.g. a hot VRM or the embedded controller).
    pub prochot_now: bool,
    pub prochot_logged: bool,
    /// At or above the critical temperature (shutdown territory).
    pub critical_now: bool,
    pub critical_logged: bool,
    /// Speed limited by a power limit; `None` when the CPU does not report it.
    pub power_limit_now: Option<bool>,
    pub power_limit_logged: Option<bool>,
}

/// Throttle event counters kept by the operating system (Linux
/// `/sys/devices/system/cpu/cpuN/thermal_throttle/*_throttle_count`), since boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ThrottleCounters {
    /// CPUs that expose counters.
    pub cpus_reporting: usize,
    /// Highest per-CPU core throttle count.
    pub core_events_max: u64,
    /// CPUs with at least one core throttle event.
    pub cpus_with_core_events: usize,
    /// Package throttle count (highest across CPUs; all CPUs of a package
    /// report the same counter).
    pub package_events: u64,
}

/// Converts millidegrees Celsius (the Linux hwmon/thermal sysfs unit) to °C.
pub fn millidegrees_to_celsius(milli: i64) -> f64 {
    milli as f64 / 1000.0
}

/// Converts tenths of a kelvin (Windows "High Precision Temperature") to °C.
pub fn decikelvin_to_celsius(dk: f64) -> f64 {
    dk / 10.0 - 273.15
}

/// Converts kelvin to °C.
pub fn kelvin_to_celsius(k: f64) -> f64 {
    k - 273.15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_conversions() {
        assert_eq!(millidegrees_to_celsius(45_000), 45.0);
        assert_eq!(millidegrees_to_celsius(45_500), 45.5);
        assert_eq!(millidegrees_to_celsius(-5_250), -5.25);
        assert!((decikelvin_to_celsius(3432.0) - 70.05).abs() < 1e-9);
        assert!((decikelvin_to_celsius(2731.5) - 0.0).abs() < 1e-9);
        assert!((kelvin_to_celsius(343.0) - 69.85).abs() < 1e-9);
    }

    #[test]
    fn implausible_values_are_rejected() {
        assert_eq!(Reading::from_celsius(42.0), Reading::Celsius(42.0));
        assert!(matches!(
            Reading::from_celsius(-273.15),
            Reading::Unavailable(_)
        ));
        assert!(matches!(
            Reading::from_celsius(6553.5),
            Reading::Unavailable(_)
        ));
        assert!(matches!(
            Reading::from_celsius(f64::NAN),
            Reading::Unavailable(_)
        ));
        assert_eq!(Reading::from_celsius(150.0), Reading::Celsius(150.0));
        assert_eq!(Reading::from_celsius(-55.0), Reading::Celsius(-55.0));
    }

    #[test]
    fn cpu_kinds() {
        assert!(SensorKind::CpuPackage.is_cpu());
        assert!(SensorKind::CpuControl.is_cpu());
        assert!(!SensorKind::AcpiThermalZone.is_cpu());
        assert!(!SensorKind::Other.is_cpu());
    }
}
