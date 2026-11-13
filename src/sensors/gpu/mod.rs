//! GPU sensor acquisition: what can be measured for each identified device.
//!
//! Identification lives in [`crate::hardware`]; this module only reads
//! sensors for devices that were already identified. Every field is a
//! [`Measurement`], so a value is either measured with its source,
//! unavailable with a reason, or not applicable.

#[cfg(windows)]
pub mod igcl;
pub mod linux;
pub mod nvidia;
#[cfg(windows)]
pub mod windows;

use std::path::Path;

use serde::Serialize;

use crate::hardware::gpu::{GpuDevice, GpuKind};
use crate::measurement::Measurement;

type M<T> = Measurement<T>;

/// One reading of a GPU's sensors.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GpuSample {
    pub utilization_pct: M<f64>,
    pub core_clock_mhz: M<f64>,
    pub memory_clock_mhz: M<f64>,
    pub temperature_c: M<f64>,
    pub memory_temperature_c: M<f64>,
    pub temperature_limit_c: M<f64>,
    pub power_w: M<f64>,
    pub power_limit_w: M<f64>,
    pub voltage_v: M<f64>,
    pub dedicated_memory_used_bytes: M<u64>,
    pub dedicated_memory_total_bytes: M<u64>,
    pub shared_memory_used_bytes: M<u64>,
    pub shared_memory_total_bytes: M<u64>,
    pub fan_speed_rpm: M<f64>,
    pub fan_speed_pct: M<f64>,
    pub thermal_throttling: M<bool>,
    pub power_throttling: M<bool>,
    /// Intel: execution units reported by the driver.
    pub execution_units: M<u32>,
    /// Architecture reported by a vendor library (e.g. NVML). An input to
    /// the report's own `architecture` field, so it is not serialized here
    /// (that would emit the key twice in the flattened report).
    #[serde(skip)]
    pub architecture: M<String>,
    /// Driver version reported by a vendor library (e.g. NVML). An input to
    /// the report's own `driver_version` field; not serialized here.
    #[serde(skip)]
    pub driver_version: M<String>,
}

/// Reason used when no source reports a value.
pub const NOT_REPORTED: &str = "not reported by any available source";

/// Integrated GPUs have no separate temperature sensor on the validated
/// platform; their temperature is part of the CPU package reading.
pub const IGPU_TEMPERATURE_REASON: &str = "no separate GPU sensor is exposed; the graphics \
    share the CPU die, so the CPU package temperature includes them";

fn unreported<T>() -> M<T> {
    M::unavailable(NOT_REPORTED)
}

fn software<T>() -> M<T> {
    M::not_applicable("software renderer, not a GPU")
}

impl GpuSample {
    /// A sample with every value unavailable, then adjusted for what the
    /// device kind makes not applicable, plus memory totals known from
    /// identification.
    pub fn for_device(dev: &GpuDevice) -> Self {
        let mut s = GpuSample {
            utilization_pct: unreported(),
            core_clock_mhz: unreported(),
            memory_clock_mhz: unreported(),
            temperature_c: unreported(),
            memory_temperature_c: unreported(),
            temperature_limit_c: unreported(),
            power_w: unreported(),
            power_limit_w: unreported(),
            voltage_v: unreported(),
            dedicated_memory_used_bytes: unreported(),
            dedicated_memory_total_bytes: unreported(),
            shared_memory_used_bytes: unreported(),
            shared_memory_total_bytes: unreported(),
            fan_speed_rpm: unreported(),
            fan_speed_pct: unreported(),
            thermal_throttling: unreported(),
            power_throttling: unreported(),
            execution_units: M::not_applicable("only reported for Intel GPUs"),
            architecture: unreported(),
            driver_version: unreported(),
        };
        if let Some(b) = dev.raw.dedicated_memory_bytes {
            s.dedicated_memory_total_bytes = M::measured(b, "B", dev.raw.source);
        }
        if let Some(b) = dev.raw.shared_memory_bytes {
            s.shared_memory_total_bytes = M::measured(b, "B", dev.raw.source);
        }
        match dev.kind {
            GpuKind::Integrated => {
                let sys = "uses system memory (no VRAM of its own)";
                s.memory_clock_mhz = M::not_applicable(sys);
                s.memory_temperature_c = M::not_applicable(sys);
                let fan = "cooled together with the CPU; no fan of its own";
                s.fan_speed_rpm = M::not_applicable(fan);
                s.fan_speed_pct = M::not_applicable(fan);
                s.temperature_c = M::unavailable(IGPU_TEMPERATURE_REASON);
            }
            GpuKind::Software => {
                s = GpuSample {
                    utilization_pct: software(),
                    core_clock_mhz: software(),
                    memory_clock_mhz: software(),
                    temperature_c: software(),
                    memory_temperature_c: software(),
                    temperature_limit_c: software(),
                    power_w: software(),
                    power_limit_w: software(),
                    voltage_v: software(),
                    dedicated_memory_used_bytes: software(),
                    dedicated_memory_total_bytes: software(),
                    shared_memory_used_bytes: software(),
                    shared_memory_total_bytes: software(),
                    fan_speed_rpm: software(),
                    fan_speed_pct: software(),
                    thermal_throttling: software(),
                    power_throttling: software(),
                    execution_units: software(),
                    architecture: software(),
                    driver_version: software(),
                };
            }
            GpuKind::Discrete | GpuKind::Unknown => {}
        }
        s
    }
}

/// A source of GPU readings for a fixed set of identified devices.
pub trait GpuSource {
    /// Identified devices (same order as [`sample`](Self::sample) results).
    fn devices(&self) -> &[GpuDevice];
    /// Reads every device; rates cover the time since the previous call.
    fn sample(&mut self) -> Vec<GpuSample>;
    /// Diagnostics from setup (e.g. a vendor library that is missing).
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Plausible GPU core voltage range in volts; outside it a driver value is
/// rejected (the Intel UHD driver reports a constant 0.26 V).
pub const PLAUSIBLE_GPU_VOLTAGE_V: std::ops::RangeInclusive<f64> = 0.5..=1.5;

/// Enumerates GPUs and creates the platform's sensor source.
pub fn default_source(sysfs_root: Option<&Path>) -> Result<Box<dyn GpuSource>, String> {
    let devices = crate::hardware::enumerate_gpus(sysfs_root)?;
    if let Some(root) = sysfs_root {
        return Ok(Box::new(linux::LinuxGpu::new(root, devices)));
    }
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsGpu::new(devices)))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxGpu::new(Path::new("/"), devices)))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = devices;
        Err(format!(
            "GPU sensors are not implemented on {}",
            std::env::consts::OS
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::gpu::{RawAdapter, identify};

    fn device(kind_hint: &str) -> GpuDevice {
        let mut d = identify(RawAdapter {
            vendor_id: Some(0x8086),
            dedicated_memory_bytes: Some(128 << 20),
            shared_memory_bytes: Some(8 << 30),
            source: "test",
            ..Default::default()
        });
        d.kind = match kind_hint {
            "i" => GpuKind::Integrated,
            "s" => GpuKind::Software,
            _ => GpuKind::Discrete,
        };
        d
    }

    #[test]
    fn integrated_defaults_distinguish_not_applicable_from_unavailable() {
        let s = GpuSample::for_device(&device("i"));
        assert!(matches!(s.memory_clock_mhz, M::NotApplicable { .. }));
        assert!(matches!(s.fan_speed_rpm, M::NotApplicable { .. }));
        assert_eq!(s.temperature_c, M::unavailable(IGPU_TEMPERATURE_REASON));
        assert_eq!(s.dedicated_memory_total_bytes.get(), Some(128 << 20));
        assert_eq!(s.utilization_pct, M::unavailable(NOT_REPORTED));
    }

    #[test]
    fn discrete_and_software_defaults() {
        let d = GpuSample::for_device(&device("d"));
        assert_eq!(d.fan_speed_rpm, M::unavailable(NOT_REPORTED));
        let s = GpuSample::for_device(&device("s"));
        assert!(matches!(s.utilization_pct, M::NotApplicable { .. }));
        assert!(matches!(
            s.dedicated_memory_total_bytes,
            M::NotApplicable { .. }
        ));
    }
}
