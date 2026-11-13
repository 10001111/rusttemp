//! GPU report data: one [`GpuReport`] per device, used by the integrated-GPU
//! section of `cpu` and by `gpu`.

use serde::Serialize;

use crate::hardware::gpu::{Evidence, GpuDevice, GpuKind, VENDOR_INTEL};
use crate::measurement::Measurement;
use crate::monitoring::Snapshot;
use crate::perf::PerfSample;
use crate::sensors::gpu::GpuSample;

type M<T> = Measurement<T>;

/// Report for one graphics device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GpuReport {
    pub kind: GpuKind,
    pub manufacturer: M<String>,
    pub model: M<String>,
    pub architecture: M<String>,
    pub pci_vendor_id: M<String>,
    pub pci_device_id: M<String>,
    pub pci_address: M<String>,
    pub driver_version: M<String>,
    /// Why the device was classified the way it was.
    pub classification: Vec<Evidence>,
    #[serde(flatten)]
    pub sample: GpuSample,
    /// Used ÷ total of the memory the GPU primarily uses (shared memory for
    /// integrated GPUs, dedicated VRAM otherwise).
    pub memory_utilization_pct: M<f64>,
}

const IGPU_POWER_SRC: &str = "CPU graphics power plane (RAPL PP1) via PawnIO";

/// Builds the report for one device.
pub fn device_report(
    dev: &GpuDevice,
    sample: Option<&GpuSample>,
    perf: Option<&PerfSample>,
) -> GpuReport {
    let src = dev.raw.source;
    let not_reported = || "not reported by the system".to_string();
    let mut sample = sample
        .cloned()
        .unwrap_or_else(|| GpuSample::for_device(dev));

    // Integrated Intel GPU power comes from the CPU's graphics plane.
    if dev.kind == GpuKind::Integrated && dev.raw.vendor_id == Some(VENDOR_INTEL) {
        if let Some(p) = perf {
            sample.power_w = match (p.graphics_w, &p.graphics_unavailable) {
                (Some(w), _) => M::measured(w, "W", IGPU_POWER_SRC),
                (None, Some(why)) => M::unavailable(crate::report::cpu::friendly(why)),
                (None, None) => M::unavailable("graphics-plane energy is not reported"),
            };
        }
    }

    let (used, total) = if dev.kind == GpuKind::Integrated {
        (
            &sample.shared_memory_used_bytes,
            &sample.shared_memory_total_bytes,
        )
    } else {
        (
            &sample.dedicated_memory_used_bytes,
            &sample.dedicated_memory_total_bytes,
        )
    };
    let memory_utilization_pct = match (used.get(), total.get()) {
        (Some(u), Some(t)) if t > 0 => M::measured(
            u as f64 * 100.0 / t as f64,
            "%",
            "derived: used ÷ total memory",
        ),
        (Some(_), Some(_)) => M::unavailable("total memory is reported as 0"),
        _ => M::unavailable("memory usage or total is unavailable"),
    };

    GpuReport {
        kind: dev.kind,
        manufacturer: if dev.raw.vendor_id.is_some() {
            M::measured(dev.vendor.clone(), "", src)
        } else {
            M::unavailable(not_reported())
        },
        model: M::measured(dev.display_name(), "", src),
        architecture: match (dev.architecture, &sample.architecture) {
            (Some(a), _) => M::measured(a.to_string(), "", "PCI device-ID table"),
            (None, m @ M::Measured { .. }) => m.clone(),
            (None, _) => M::unavailable("this device ID is not in RustTemp's architecture table"),
        },
        pci_vendor_id: M::from_option(
            dev.raw.vendor_id.map(|v| format!("0x{v:04X}")),
            "",
            src,
            not_reported,
        ),
        pci_device_id: M::from_option(
            dev.raw.device_id.map(|v| format!("0x{v:04X}")),
            "",
            src,
            not_reported,
        ),
        pci_address: match dev.raw.pci {
            Some(p) => M::measured(p.to_string(), "", src),
            None => M::not_applicable("not a PCI device"),
        },
        // A vendor library's version (e.g. NVML "566.36") is preferred.
        driver_version: if sample.driver_version.is_measured() {
            sample.driver_version.clone()
        } else {
            M::from_option(
                dev.raw
                    .driver_version
                    .clone()
                    .or_else(|| dev.raw.driver.clone()),
                "",
                if dev.raw.driver_version.is_some() {
                    "Windows SetupAPI (driver registry key)"
                } else {
                    "kernel driver name"
                },
                not_reported,
            )
        },
        classification: dev.evidence.clone(),
        sample,
        memory_utilization_pct,
    }
}

/// The integrated-GPU section of the `cpu` report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntegratedGpuSection {
    pub detected: bool,
    pub devices: Vec<GpuReport>,
    /// Graphics devices that could be neither confirmed integrated nor
    /// dedicated (shown by `gpu`).
    pub unclassified: usize,
    /// Why GPU detection could not run, if it failed.
    pub detection_error: Option<String>,
}

/// Builds the integrated-GPU section from a snapshot.
pub fn integrated_section(
    snap: &Snapshot<'_>,
    detection_error: Option<&str>,
) -> IntegratedGpuSection {
    let devices: Vec<GpuReport> = snap
        .gpu_devices
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == GpuKind::Integrated)
        .map(|(i, d)| device_report(d, snap.gpu_samples.get(i), snap.perf))
        .collect();
    IntegratedGpuSection {
        detected: !devices.is_empty(),
        unclassified: snap
            .gpu_devices
            .iter()
            .filter(|d| d.kind == GpuKind::Unknown)
            .count(),
        devices,
        detection_error: detection_error.map(str::to_string),
    }
}

/// The `gpu` command's report. Dedicated GPUs are always listed one by one;
/// integrated GPUs and software renderers only with `--all`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GpuCommandReport {
    pub dedicated: Vec<GpuReport>,
    /// Devices that could be neither confirmed integrated nor dedicated.
    pub unclassified: Vec<GpuReport>,
    /// Integrated GPUs (filled only with `--all`; see `cpu`).
    pub integrated: Vec<GpuReport>,
    /// Software renderers (filled only with `--all`).
    pub software: Vec<GpuReport>,
    pub integrated_count: usize,
    pub software_count: usize,
    /// Why GPU detection could not run, if it failed.
    pub detection_error: Option<String>,
    pub notes: Vec<String>,
}

/// Builds the `gpu` report from a snapshot.
pub fn gpu_command_report(
    snap: &Snapshot<'_>,
    all: bool,
    detection_error: Option<&str>,
) -> GpuCommandReport {
    let of_kind = |k: GpuKind| -> Vec<GpuReport> {
        snap.gpu_devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == k)
            .map(|(i, d)| device_report(d, snap.gpu_samples.get(i), snap.perf))
            .collect()
    };
    let count = |k: GpuKind| snap.gpu_devices.iter().filter(|d| d.kind == k).count();
    GpuCommandReport {
        dedicated: of_kind(GpuKind::Discrete),
        unclassified: of_kind(GpuKind::Unknown),
        integrated: if all {
            of_kind(GpuKind::Integrated)
        } else {
            Vec::new()
        },
        software: if all {
            of_kind(GpuKind::Software)
        } else {
            Vec::new()
        },
        integrated_count: count(GpuKind::Integrated),
        software_count: count(GpuKind::Software),
        detection_error: detection_error.map(str::to_string),
        notes: snap.notes.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::gpu::{AdapterFlags, PciAddress, RawAdapter, identify};

    fn uhd() -> GpuDevice {
        identify(RawAdapter {
            name: Some("Intel(R) UHD Graphics".into()),
            vendor_id: Some(0x8086),
            device_id: Some(0xA7A8),
            pci: Some(PciAddress {
                domain: 0,
                bus: 0,
                device: 2,
                function: 0,
            }),
            luid: Some((0, 0x1087d)),
            flags: Some(AdapterFlags::from_d3dkmt(0x232b)),
            dedicated_memory_bytes: Some(128 << 20),
            shared_memory_bytes: Some(8 << 30),
            driver_version: Some("32.0.101.7076".into()),
            source: "D3DKMT",
            ..Default::default()
        })
    }

    #[test]
    fn integrated_intel_report() {
        let dev = uhd();
        let mut sample = GpuSample::for_device(&dev);
        sample.shared_memory_used_bytes = M::measured(2 << 30, "B", "PDH");
        let perf = PerfSample {
            graphics_w: Some(0.08),
            ..Default::default()
        };
        let r = device_report(&dev, Some(&sample), Some(&perf));
        assert_eq!(r.kind, GpuKind::Integrated);
        assert_eq!(r.manufacturer.value().unwrap(), "Intel");
        assert_eq!(r.pci_address.value().unwrap(), "0000:00:02.0");
        assert_eq!(r.pci_device_id.value().unwrap(), "0xA7A8");
        assert_eq!(r.driver_version.value().unwrap(), "32.0.101.7076");
        assert_eq!(r.sample.power_w.get(), Some(0.08));
        assert_eq!(r.sample.power_w.source(), Some(IGPU_POWER_SRC));
        assert_eq!(r.memory_utilization_pct.get(), Some(25.0));
        assert!(!r.classification.is_empty());
    }

    #[test]
    fn json_has_no_duplicate_keys() {
        // The sample is flattened into the report; a field present in both
        // would be emitted twice, and JSON parsers keep the last copy (the
        // raw, unmerged one). Regression test for exactly that bug.
        let dev = uhd();
        let r = device_report(&dev, None, None);
        let text = serde_json::to_string(&r).unwrap();
        let top = serde_json::to_value(&r).unwrap();
        for key in top.as_object().unwrap().keys() {
            let needle = format!("\"{key}\":");
            // Keys of nested objects ("value", "status"...) are not top-level
            // report keys, so a count above 1 can only be a duplicate.
            assert_eq!(text.matches(&needle).count(), 1, "duplicate key {key}");
        }
        assert_eq!(
            top["architecture"]["value"],
            "Xe-LP (Gen12, Raptor Lake-P graphics)"
        );
        assert_eq!(top["driver_version"]["value"], "32.0.101.7076");
    }

    #[test]
    fn igpu_power_without_admin_is_explained() {
        let dev = uhd();
        let perf = PerfSample {
            graphics_unavailable: Some(
                "PawnIO access denied: run RustTemp from an administrator terminal".into(),
            ),
            ..Default::default()
        };
        let r = device_report(&dev, None, Some(&perf));
        assert_eq!(
            r.sample.power_w,
            M::unavailable(crate::report::cpu::ADMIN_REASON)
        );
    }

    #[test]
    fn gpu_command_separates_kinds_and_never_merges() {
        use crate::monitoring::SensorStats;
        let nv = |bus: u32| {
            let mut d = identify(RawAdapter {
                name: Some(format!("NVIDIA GPU on bus {bus}")),
                vendor_id: Some(0x10DE),
                pci: Some(PciAddress {
                    domain: 0,
                    bus,
                    device: 0,
                    function: 0,
                }),
                flags: Some(AdapterFlags {
                    hybrid_discrete: true,
                    ..Default::default()
                }),
                source: "test",
                ..Default::default()
            });
            d.kind = GpuKind::Discrete;
            d
        };
        let software = identify(RawAdapter {
            vendor_id: Some(0x1414),
            source: "test",
            ..Default::default()
        });
        let devices = vec![uhd(), nv(1), nv(2), software];
        let mut samples: Vec<GpuSample> = devices.iter().map(GpuSample::for_device).collect();
        samples[1].temperature_c = M::measured(60.0, "°C", "NVML");
        samples[2].temperature_c = M::measured(70.0, "°C", "NVML");
        let stats: Vec<SensorStats> = vec![];
        let snap = Snapshot {
            backend: "test",
            at: std::time::UNIX_EPOCH,
            elapsed: std::time::Duration::ZERO,
            sample: 1,
            sensors: &[],
            readings: &[],
            stats: &stats,
            notes: &[],
            flags: &[],
            counters: None,
            throttle_samples: 0,
            perf: None,
            perf_stats: &crate::perf::PerfStats::default(),
            gpu_devices: &devices,
            gpu_samples: &samples,
        };
        let r = gpu_command_report(&snap, false, None);
        assert_eq!(r.dedicated.len(), 2);
        assert_eq!(r.dedicated[0].sample.temperature_c.get(), Some(60.0));
        assert_eq!(r.dedicated[1].sample.temperature_c.get(), Some(70.0));
        assert!(r.integrated.is_empty() && r.software.is_empty());
        assert_eq!((r.integrated_count, r.software_count), (1, 1));
        let all = gpu_command_report(&snap, true, None);
        assert_eq!(all.integrated.len(), 1);
        assert_eq!(all.software.len(), 1);
        assert_eq!(all.integrated[0].kind, GpuKind::Integrated);
    }

    #[test]
    fn json_flattens_sample_fields() {
        let r = device_report(&uhd(), None, None);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["kind"], "integrated");
        assert_eq!(v["memory_clock_mhz"]["status"], "not_applicable");
        assert_eq!(v["dedicated_memory_total_bytes"]["value"], 128u64 << 20);
    }
}
