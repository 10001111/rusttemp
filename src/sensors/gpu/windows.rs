//! Windows GPU sensors.
//!
//! - **Vendor-neutral** (no admin): PDH `GPU Engine` utilization (busiest
//!   engine, summed over processes, as Task Manager shows it) and
//!   `GPU Adapter Memory` dedicated/shared usage, both keyed by adapter LUID.
//! - **Intel** (no admin): IGCL clock, throttle flags and execution units.
//!   Its voltage is accepted only within [`PLAUSIBLE_GPU_VOLTAGE_V`]. Its energy
//!   counter is never used, because on integrated GPUs it equals the CPU
//!   package energy. Integrated GPU power comes from the CPU's graphics
//!   power plane instead (see `perf`).
//! - **NVIDIA / AMD vendor sensors:** see Phase 5 of the plan.

use std::collections::HashMap;

use super::igcl::Igcl;
use super::nvidia::{self, NvmlSession};
use super::{GpuSample, GpuSource, PLAUSIBLE_GPU_VOLTAGE_V};
use crate::hardware::gpu::{
    GpuDevice, GpuKind, VENDOR_AMD, VENDOR_INTEL, VENDOR_NVIDIA, parse_luid,
};
use crate::measurement::Measurement as M;
use crate::perf::windows::Pdh;

const ENGINE_SRC: &str = "Windows GPU engine counters (busiest engine)";
const MEMORY_SRC: &str = "Windows GPU adapter memory counters";
const IGCL_SRC: &str = "Intel Graphics Control Library";

/// Key of one GPU engine: `(luid, physical adapter index, engine index)`.
type EngineKey = ((i32, u32), u32, u32);

/// Parses `phys_N` and `eng_N` from a `GPU Engine` instance name.
pub fn parse_engine(instance: &str) -> Option<EngineKey> {
    let luid = parse_luid(instance)?;
    let num_after = |tag: &str| -> Option<u32> {
        let rest = &instance[instance.find(tag)? + tag.len()..];
        rest.split('_').next()?.parse().ok()
    };
    Some((luid, num_after("_phys_")?, num_after("_eng_")?))
}

/// Task-Manager-style utilization per adapter: sum each engine over all
/// processes, then take the busiest engine.
pub fn utilization_by_adapter(values: &[(String, f64)]) -> HashMap<(i32, u32), f64> {
    let mut per_engine: HashMap<EngineKey, f64> = HashMap::new();
    for (name, v) in values {
        if let Some(k) = parse_engine(name) {
            *per_engine.entry(k).or_default() += v;
        }
    }
    let mut out: HashMap<(i32, u32), f64> = HashMap::new();
    for ((luid, _, _), v) in per_engine {
        let e = out.entry(luid).or_default();
        *e = e.max(v);
    }
    out
}

/// Sum of a per-adapter memory counter by LUID.
pub fn memory_by_adapter(values: &[(String, f64)]) -> HashMap<(i32, u32), u64> {
    let mut out: HashMap<(i32, u32), u64> = HashMap::new();
    for (name, v) in values {
        if let Some(luid) = parse_luid(name) {
            *out.entry(luid).or_default() += v.max(0.0) as u64;
        }
    }
    out
}

/// Windows GPU sensor source.
pub struct WindowsGpu {
    devices: Vec<GpuDevice>,
    pdh: Option<Pdh>,
    pdh_error: Option<String>,
    igcl: Option<Igcl>,
    igcl_error: Option<String>,
    nvml: Option<NvmlSession>,
    nvml_error: Option<String>,
}

impl WindowsGpu {
    /// Opens the counters and vendor libraries and takes a baseline.
    pub fn new(devices: Vec<GpuDevice>) -> Self {
        let (pdh, pdh_error) = match Pdh::open(&[
            r"\GPU Engine(*)\Utilization Percentage",
            r"\GPU Adapter Memory(*)\Dedicated Usage",
            r"\GPU Adapter Memory(*)\Shared Usage",
        ]) {
            Ok(p) => {
                let _ = p.collect();
                (Some(p), None)
            }
            Err(s) => (
                None,
                Some(format!(
                    "GPU performance counters unavailable (PDH 0x{s:08X})"
                )),
            ),
        };
        let has_intel = devices
            .iter()
            .any(|d| d.raw.vendor_id == Some(VENDOR_INTEL));
        let (igcl, igcl_error) = if has_intel {
            match Igcl::open() {
                Ok(i) => (Some(i), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };
        let has_nvidia = devices
            .iter()
            .any(|d| d.raw.vendor_id == Some(VENDOR_NVIDIA));
        let (nvml, nvml_error) = if has_nvidia {
            match NvmlSession::open() {
                Ok(n) => (Some(n), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };
        WindowsGpu {
            devices,
            pdh,
            pdh_error,
            igcl,
            igcl_error,
            nvml,
            nvml_error,
        }
    }
}

fn vendor_reason(dev: &GpuDevice) -> String {
    match dev.raw.vendor_id {
        Some(VENDOR_NVIDIA) => "NVIDIA sensor library (NVML) not available".into(),
        Some(VENDOR_AMD) => "AMD vendor sensors are not implemented yet; only Windows' generic \
            load and memory counters are available"
            .into(),
        _ => "no vendor sensor library for this GPU".into(),
    }
}

impl GpuSource for WindowsGpu {
    fn devices(&self) -> &[GpuDevice] {
        &self.devices
    }

    fn sample(&mut self) -> Vec<GpuSample> {
        let (util, dedicated, shared, counters_ok) = match &self.pdh {
            Some(p) if p.collect().is_ok() => (
                utilization_by_adapter(&p.values(0)),
                memory_by_adapter(&p.values(1)),
                memory_by_adapter(&p.values(2)),
                true,
            ),
            _ => (HashMap::new(), HashMap::new(), HashMap::new(), false),
        };
        let counter_reason = self
            .pdh_error
            .clone()
            .unwrap_or_else(|| "GPU performance counters could not be read".into());

        self.devices
            .iter()
            .map(|dev| {
                let mut s = GpuSample::for_device(dev);
                if dev.kind == GpuKind::Software {
                    return s;
                }
                let luid = dev.raw.luid;
                s.utilization_pct = match luid.and_then(|l| util.get(&l)) {
                    Some(u) => M::measured(u.clamp(0.0, 100.0), "%", ENGINE_SRC),
                    None if counters_ok => {
                        M::unavailable("no process is using this GPU (no engine counters)")
                    }
                    None => M::unavailable(counter_reason.clone()),
                };
                for (map, field) in [
                    (&dedicated, &mut s.dedicated_memory_used_bytes),
                    (&shared, &mut s.shared_memory_used_bytes),
                ] {
                    *field = match luid.and_then(|l| map.get(&l)) {
                        Some(b) => M::measured(*b, "B", MEMORY_SRC),
                        None if counters_ok => M::unavailable("no memory counters for this GPU"),
                        None => M::unavailable(counter_reason.clone()),
                    };
                }

                let vendor = vendor_reason(dev);
                let igcl_dev = self.igcl.as_ref().and_then(|i| {
                    i.devices
                        .iter()
                        .find(|d| Some(d.luid) == luid)
                        .map(|d| (i, d))
                });
                match igcl_dev {
                    Some((igcl, d)) => {
                        if let Some(n) = d.execution_units {
                            s.execution_units = M::measured(n, "", IGCL_SRC);
                        }
                        match igcl.telemetry(d) {
                            Ok(t) => {
                                if let Some(c) = t.clock_mhz {
                                    s.core_clock_mhz = M::measured(c, "MHz", IGCL_SRC);
                                }
                                if let Some(c) = t.temperature_c {
                                    s.temperature_c = M::measured(c, "°C", IGCL_SRC);
                                } else if dev.kind != GpuKind::Integrated {
                                    s.temperature_c =
                                        M::unavailable("not reported by the Intel driver");
                                }
                                s.voltage_v = match t.voltage_v {
                                    Some(v) if PLAUSIBLE_GPU_VOLTAGE_V.contains(&v) => {
                                        M::measured(v, "V", IGCL_SRC)
                                    }
                                    Some(v) => M::unavailable(format!(
                                        "the driver's value ({v:.3} V) failed a plausibility check \
                                         (expected 0.5–1.5 V)"
                                    )),
                                    None => M::unavailable("not reported by the Intel driver"),
                                };
                                s.thermal_throttling =
                                    M::measured(t.temperature_limited, "", IGCL_SRC);
                                s.power_throttling = M::measured(t.power_limited, "", IGCL_SRC);
                            }
                            Err(e) => {
                                s.core_clock_mhz = M::unavailable(e.clone());
                                s.voltage_v = M::unavailable(e);
                            }
                        }
                        s.power_w = M::unavailable(if dev.kind == GpuKind::Integrated {
                            "read from the CPU's graphics power plane (needs administrator rights)"
                        } else {
                            "the Intel driver's energy counter is not validated for discrete GPUs"
                        });
                    }
                    None if dev.raw.vendor_id == Some(VENDOR_NVIDIA) && self.nvml.is_some() => {
                        let read = dev
                            .raw
                            .pci
                            .ok_or_else(|| "no PCI location to match with NVML".to_string())
                            .and_then(|p| self.nvml.as_ref().unwrap().read(p.bus, p.device));
                        match read {
                            Ok(r) => nvidia::apply(&mut s, &r),
                            Err(e) => {
                                for f in
                                    [&mut s.core_clock_mhz, &mut s.power_w, &mut s.temperature_c]
                                {
                                    *f = M::unavailable(e.clone());
                                }
                            }
                        }
                    }
                    None => {
                        let why = match (&self.igcl_error, &self.nvml_error, dev.raw.vendor_id) {
                            (Some(e), _, Some(VENDOR_INTEL)) => e.clone(),
                            (_, Some(e), Some(VENDOR_NVIDIA)) => e.clone(),
                            _ => vendor.clone(),
                        };
                        for f in [&mut s.core_clock_mhz, &mut s.voltage_v, &mut s.power_w] {
                            *f = M::unavailable(why.clone());
                        }
                        if dev.kind != GpuKind::Integrated {
                            s.temperature_c = M::unavailable(why.clone());
                            s.fan_speed_rpm = M::unavailable(why.clone());
                            s.fan_speed_pct = M::unavailable(why.clone());
                            s.memory_clock_mhz = M::unavailable(why.clone());
                        }
                        s.thermal_throttling = M::unavailable(why.clone());
                        s.power_throttling = M::unavailable(why);
                    }
                }
                s
            })
            .collect()
    }

    fn notes(&self) -> Vec<String> {
        self.pdh_error
            .iter()
            .chain(self.nvml_error.iter())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str, x: f64) -> (String, f64) {
        (name.to_string(), x)
    }

    #[test]
    fn engine_instances() {
        assert_eq!(
            parse_engine("pid_10652_luid_0x00000000_0x00010c2a_phys_0_eng_10_engtype_3d"),
            Some(((0, 0x10c2a), 0, 10))
        );
        assert_eq!(parse_engine("_Total"), None);
    }

    #[test]
    fn busiest_engine_summed_over_processes() {
        let vals = [
            // Adapter A, engine 0 (3D): two processes 20% + 15%.
            v(
                "pid_1_luid_0x00000000_0x0000000a_phys_0_eng_0_engtype_3D",
                20.0,
            ),
            v(
                "pid_2_luid_0x00000000_0x0000000a_phys_0_eng_0_engtype_3D",
                15.0,
            ),
            // Adapter A, engine 1 (video decode): 30%.
            v(
                "pid_2_luid_0x00000000_0x0000000a_phys_0_eng_1_engtype_VideoDecode",
                30.0,
            ),
            // Adapter B idle.
            v(
                "pid_3_luid_0x00000000_0x0000000b_phys_0_eng_0_engtype_3D",
                0.0,
            ),
        ];
        let u = utilization_by_adapter(&vals);
        assert_eq!(u[&(0, 0xa)], 35.0);
        assert_eq!(u[&(0, 0xb)], 0.0);
    }

    #[test]
    fn memory_counters() {
        let vals = [
            v("luid_0x00000000_0x0001087d_phys_0", 1_201_991_680.0),
            v("luid_0x00000000_0x00010c2a_phys_0", 8192.0),
        ];
        let m = memory_by_adapter(&vals);
        assert_eq!(m[&(0, 0x1087d)], 1_201_991_680);
        assert_eq!(m[&(0, 0x10c2a)], 8192);
    }

    /// Live: the Intel iGPU gets utilization and memory from the counters,
    /// and clock from IGCL; nothing is reported for the software renderer.
    #[test]
    fn live_sample() {
        let devices = crate::hardware::enumerate_gpus(None).unwrap();
        let mut src = WindowsGpu::new(devices);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let samples = src.sample();
        assert_eq!(samples.len(), src.devices().len());
        for (d, s) in src.devices().iter().zip(&samples) {
            if d.kind == GpuKind::Software {
                assert!(matches!(s.utilization_pct, M::NotApplicable { .. }));
            }
            if let Some(u) = s.utilization_pct.get() {
                assert!((0.0..=100.0).contains(&u));
            }
            assert!(
                !matches!(s.power_w, M::Measured { .. }),
                "no unvalidated GPU power"
            );
        }
    }

    /// Diagnostic (run while a GPU load is active):
    /// `cargo test --release diag_igpu_utilization -- --ignored --nocapture`.
    /// Compares, over identical 1 s windows, the Task-Manager-style PDH
    /// utilization with IGCL's busy-time counter (Δbusy ÷ Δtime), and shows
    /// the IGCL clock.
    #[test]
    #[ignore = "diagnostic: run while a GPU load is active"]
    fn diag_igpu_utilization() {
        let devices = crate::hardware::enumerate_gpus(None).unwrap();
        let dev = devices
            .iter()
            .find(|d| d.kind == GpuKind::Integrated)
            .expect("an integrated GPU")
            .clone();
        let luid = dev.raw.luid.unwrap();
        let igcl = Igcl::open().expect("IGCL");
        let idev = igcl
            .devices
            .iter()
            .find(|d| d.luid == luid)
            .expect("IGCL device");
        let pdh = Pdh::open(&[r"\GPU Engine(*)\Utilization Percentage"]).unwrap();
        pdh.collect().unwrap();
        let mut prev = igcl.telemetry(idev).unwrap();
        for round in 1..=6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            pdh.collect().unwrap();
            let t = igcl.telemetry(idev).unwrap();
            let pdh_u = utilization_by_adapter(&pdh.values(0)).get(&luid).copied();
            let igcl_u = match (
                t.global_activity_s,
                prev.global_activity_s,
                t.timestamp_s,
                prev.timestamp_s,
            ) {
                (Some(a), Some(pa), Some(ts), Some(pts)) if ts > pts => {
                    Some((a - pa) / (ts - pts) * 100.0)
                }
                _ => None,
            };
            println!(
                "round {round}: PDH busiest-engine {:?} % | IGCL busy {:?} % | IGCL clock {:?} MHz | power-limited {} temp-limited {}",
                pdh_u.map(|v| (v * 10.0).round() / 10.0),
                igcl_u.map(|v| (v * 10.0).round() / 10.0),
                t.clock_mhz,
                t.power_limited,
                t.temperature_limited
            );
            prev = t;
        }
    }
}
