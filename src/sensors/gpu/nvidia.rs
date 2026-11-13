//! NVIDIA GPUs via NVML (`nvml-wrapper`), loaded at runtime from an
//! explicit path: `%SystemRoot%\System32\nvml.dll` on Windows,
//! `libnvidia-ml.so.1` on Linux. No admin rights are needed.
//!
//! Reading the hardware ([`NvmlSession::read`]) is separate from mapping
//! values into a [`GpuSample`] ([`apply`]), so the mapping is unit-tested
//! with synthetic data. No NVIDIA hardware was available during
//! development, so the live path is **hardware-unverified**.
//!
//! Not exposed by NVML on consumer GPUs, so reported as unavailable:
//! - GPU voltage
//! - memory temperature (data-centre HBM parts only)

use super::GpuSample;
use crate::measurement::Measurement as M;

const SRC: &str = "NVIDIA NVML";

/// Values read from NVML for one device. `Err` carries the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct NvmlReading {
    pub utilization_pct: Result<u32, String>,
    pub graphics_clock_mhz: Result<u32, String>,
    pub memory_clock_mhz: Result<u32, String>,
    pub temperature_c: Result<u32, String>,
    pub slowdown_threshold_c: Result<u32, String>,
    pub power_mw: Result<u32, String>,
    pub power_limit_mw: Result<u32, String>,
    pub fan_count: Result<u32, String>,
    pub fan_pct: Result<u32, String>,
    pub fan_rpm: Result<u32, String>,
    /// `(used, total)` bytes.
    pub memory: Result<(u64, u64), String>,
    /// Raw clock-throttle reason bits.
    pub throttle_reasons: Result<u64, String>,
    pub architecture: Result<String, String>,
    pub driver_version: Result<String, String>,
}

/// NVML clock-throttle reason bits (nvml.h).
pub const SW_POWER_CAP: u64 = 0x4;
pub const SW_THERMAL_SLOWDOWN: u64 = 0x20;
pub const HW_THERMAL_SLOWDOWN: u64 = 0x40;
pub const HW_POWER_BRAKE_SLOWDOWN: u64 = 0x80;

fn m<T: Copy, U>(r: &Result<T, String>, unit: &'static str, f: impl Fn(T) -> U) -> M<U> {
    match r {
        Ok(v) => M::measured(f(*v), unit, SRC),
        Err(e) => M::unavailable(e.clone()),
    }
}

/// Writes NVML values into a sample.
pub fn apply(s: &mut GpuSample, r: &NvmlReading) {
    s.utilization_pct = m(&r.utilization_pct, "%", f64::from);
    s.core_clock_mhz = m(&r.graphics_clock_mhz, "MHz", f64::from);
    s.memory_clock_mhz = m(&r.memory_clock_mhz, "MHz", f64::from);
    s.temperature_c = m(&r.temperature_c, "°C", f64::from);
    s.temperature_limit_c = m(&r.slowdown_threshold_c, "°C", f64::from);
    s.power_w = m(&r.power_mw, "W", |mw| f64::from(mw) / 1000.0);
    s.power_limit_w = m(&r.power_limit_mw, "W", |mw| f64::from(mw) / 1000.0);
    s.voltage_v = M::unavailable("NVML does not expose GPU voltage");
    s.memory_temperature_c = M::unavailable("not exposed by NVML for this GPU");
    match &r.memory {
        Ok((used, total)) => {
            s.dedicated_memory_used_bytes = M::measured(*used, "B", SRC);
            s.dedicated_memory_total_bytes = M::measured(*total, "B", SRC);
        }
        Err(e) => s.dedicated_memory_used_bytes = M::unavailable(e.clone()),
    }
    match r.fan_count {
        Ok(0) => {
            let none = "this GPU reports no fan of its own (cooled by the system)";
            s.fan_speed_pct = M::not_applicable(none);
            s.fan_speed_rpm = M::not_applicable(none);
        }
        _ => {
            s.fan_speed_pct = m(&r.fan_pct, "%", f64::from);
            s.fan_speed_rpm = m(&r.fan_rpm, "RPM", f64::from);
        }
    }
    match r.throttle_reasons {
        Ok(bits) => {
            s.thermal_throttling = M::measured(
                bits & (SW_THERMAL_SLOWDOWN | HW_THERMAL_SLOWDOWN) != 0,
                "",
                SRC,
            );
            s.power_throttling = M::measured(
                bits & (SW_POWER_CAP | HW_POWER_BRAKE_SLOWDOWN) != 0,
                "",
                SRC,
            );
        }
        Err(ref e) => {
            s.thermal_throttling = M::unavailable(e.clone());
            s.power_throttling = M::unavailable(e.clone());
        }
    }
    s.architecture = match &r.architecture {
        Ok(a) => M::measured(a.clone(), "", SRC),
        Err(e) => M::unavailable(e.clone()),
    };
    s.driver_version = match &r.driver_version {
        Ok(v) => M::measured(v.clone(), "", SRC),
        Err(e) => M::unavailable(e.clone()),
    };
}

#[cfg(feature = "nvidia")]
mod live {
    use super::NvmlReading;
    use nvml_wrapper::Nvml;
    use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor, TemperatureThreshold};
    use nvml_wrapper::error::NvmlError;

    fn reason(e: NvmlError) -> String {
        match e {
            NvmlError::NotSupported => "not supported by this GPU or driver".into(),
            NvmlError::NoPermission => "NVML denied access to this value".into(),
            other => format!("NVML error: {other}"),
        }
    }

    /// An initialized NVML library.
    pub struct NvmlSession {
        nvml: Nvml,
        /// `(bus, device)` of each NVML device index.
        pci: Vec<Option<(u32, u32)>>,
    }

    impl NvmlSession {
        /// Loads NVML from its fixed system location.
        pub fn open() -> Result<Self, String> {
            #[cfg(windows)]
            let path = std::path::PathBuf::from(
                std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()),
            )
            .join(r"System32\nvml.dll")
            .into_os_string();
            #[cfg(not(windows))]
            let path = std::ffi::OsString::from("libnvidia-ml.so.1");
            let nvml = Nvml::builder()
                .lib_path(&path)
                .init()
                .map_err(|e| format!("NVIDIA NVML is not available ({e})"))?;
            let count = nvml.device_count().map_err(reason)?;
            let pci = (0..count)
                .map(|i| {
                    nvml.device_by_index(i)
                        .ok()
                        .and_then(|d| d.pci_info().ok())
                        .map(|p| (p.bus, p.device))
                })
                .collect();
            Ok(NvmlSession { nvml, pci })
        }

        /// Reads the NVML device at PCI `(bus, device)`.
        pub fn read(&self, bus: u32, device: u32) -> Result<NvmlReading, String> {
            let index = self
                .pci
                .iter()
                .position(|p| *p == Some((bus, device)))
                .ok_or("NVML does not list this GPU")?;
            let d = self.nvml.device_by_index(index as u32).map_err(reason)?;
            let fan_count = d.num_fans().map_err(reason);
            Ok(NvmlReading {
                utilization_pct: d.utilization_rates().map(|u| u.gpu).map_err(reason),
                graphics_clock_mhz: d.clock_info(Clock::Graphics).map_err(reason),
                memory_clock_mhz: d.clock_info(Clock::Memory).map_err(reason),
                temperature_c: d.temperature(TemperatureSensor::Gpu).map_err(reason),
                slowdown_threshold_c: d
                    .temperature_threshold(TemperatureThreshold::Slowdown)
                    .map_err(reason),
                power_mw: d.power_usage().map_err(reason),
                power_limit_mw: d.enforced_power_limit().map_err(reason),
                fan_pct: d.fan_speed(0).map_err(reason),
                fan_rpm: d.fan_speed_rpm(0).map_err(reason),
                fan_count,
                memory: d.memory_info().map(|m| (m.used, m.total)).map_err(reason),
                throttle_reasons: d
                    .current_throttle_reasons()
                    .map(|t| t.bits())
                    .map_err(reason),
                architecture: d.architecture().map(|a| a.to_string()).map_err(reason),
                driver_version: self.nvml.sys_driver_version().map_err(reason),
            })
        }
    }
}

#[cfg(feature = "nvidia")]
pub use live::NvmlSession;

/// Stand-in when RustTemp is built without the `nvidia` feature.
#[cfg(not(feature = "nvidia"))]
pub struct NvmlSession;

#[cfg(not(feature = "nvidia"))]
impl NvmlSession {
    pub fn open() -> Result<Self, String> {
        Err("NVIDIA NVML support was not compiled in (Cargo feature `nvidia`)".into())
    }

    pub fn read(&self, _bus: u32, _device: u32) -> Result<NvmlReading, String> {
        Err("NVIDIA NVML support was not compiled in (Cargo feature `nvidia`)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::gpu::{GpuKind, RawAdapter, identify};

    fn sample() -> GpuSample {
        let mut d = identify(RawAdapter {
            vendor_id: Some(0x10DE),
            ..Default::default()
        });
        d.kind = GpuKind::Discrete;
        GpuSample::for_device(&d)
    }

    fn full() -> NvmlReading {
        NvmlReading {
            utilization_pct: Ok(87),
            graphics_clock_mhz: Ok(2310),
            memory_clock_mhz: Ok(8001),
            temperature_c: Ok(71),
            slowdown_threshold_c: Ok(87),
            power_mw: Ok(112_450),
            power_limit_mw: Ok(140_000),
            fan_count: Ok(2),
            fan_pct: Ok(54),
            fan_rpm: Ok(2140),
            memory: Ok((3 << 30, 8 << 30)),
            throttle_reasons: Ok(SW_POWER_CAP),
            architecture: Ok("Ada".into()),
            driver_version: Ok("566.36".into()),
        }
    }

    #[test]
    fn full_reading() {
        let mut s = sample();
        apply(&mut s, &full());
        assert_eq!(s.utilization_pct.get(), Some(87.0));
        assert_eq!(s.core_clock_mhz.get(), Some(2310.0));
        assert_eq!(s.memory_clock_mhz.get(), Some(8001.0));
        assert_eq!(s.temperature_c.get(), Some(71.0));
        assert_eq!(s.temperature_limit_c.get(), Some(87.0));
        assert_eq!(s.power_w.get(), Some(112.45));
        assert_eq!(s.power_limit_w.get(), Some(140.0));
        assert_eq!(s.fan_speed_pct.get(), Some(54.0));
        assert_eq!(s.fan_speed_rpm.get(), Some(2140.0));
        assert_eq!(s.dedicated_memory_used_bytes.get(), Some(3 << 30));
        assert_eq!(s.dedicated_memory_total_bytes.get(), Some(8 << 30));
        assert_eq!(s.power_throttling.get(), Some(true));
        assert_eq!(s.thermal_throttling.get(), Some(false));
        assert_eq!(s.architecture.value().unwrap(), "Ada");
        assert_eq!(s.driver_version.value().unwrap(), "566.36");
        assert!(!s.voltage_v.is_measured());
    }

    #[test]
    fn unsupported_values_and_fanless_laptop_gpu() {
        let mut r = full();
        r.fan_count = Ok(0);
        r.power_mw = Err("not supported by this GPU or driver".into());
        r.throttle_reasons = Ok(HW_THERMAL_SLOWDOWN);
        let mut s = sample();
        apply(&mut s, &r);
        assert!(matches!(s.fan_speed_rpm, M::NotApplicable { .. }));
        assert_eq!(
            s.power_w,
            M::unavailable("not supported by this GPU or driver")
        );
        assert_eq!(s.thermal_throttling.get(), Some(true));
        assert_eq!(s.power_throttling.get(), Some(false));
    }

    #[test]
    fn opening_without_nvidia_driver_explains() {
        // The development machine has no NVIDIA driver.
        if let Err(e) = NvmlSession::open() {
            assert!(e.contains("NVML") || e.contains("nvidia"), "{e}");
        }
    }
}
