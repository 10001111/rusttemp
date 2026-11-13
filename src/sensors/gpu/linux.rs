//! Linux GPU sensors from `/sys/class/drm/cardN` (root-relative).
//!
//! - `amdgpu`: `device/gpu_busy_percent`, `device/mem_info_vram_{used,total}`,
//!   `device/mem_info_gtt_{used,total}` (shared), and hwmon under
//!   `device/hwmon/hwmon*`:
//!   - `temp*_input` with labels `edge` / `junction` / `mem` (m°C)
//!   - `power1_average` or `power1_input` and `power1_cap` (µW)
//!   - `fan1_input` (RPM), `pwm1` (0–255)
//!   - `freq1_input` (core clock, Hz), `freq2_input` (memory clock, Hz)
//!   - `in0_input` (mV)
//! - `i915`: `gt_act_freq_mhz` (actual GPU clock). `xe`:
//!   `device/tile0/gt0/freq0/act_freq`.
//! - Utilization for i915/xe needs the perf PMU (privileged), so it is
//!   reported as unavailable.
//! - NVIDIA: see Phase 5 (NVML).

use std::fs;
use std::path::{Path, PathBuf};

use super::nvidia::{self, NvmlSession};
use super::{GpuSample, GpuSource};
use crate::hardware::gpu::{GpuDevice, GpuKind};
use crate::measurement::Measurement as M;

fn read(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn num(p: &Path) -> Option<f64> {
    read(p)?.parse().ok()
}

/// First `hwmon*` directory under `device/hwmon`.
fn hwmon(dev: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dev.join("hwmon"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    v.sort();
    v.into_iter().next()
}

/// Temperature (°C) of the hwmon channel whose label is `label`.
fn temp_by_label(h: &Path, label: &str) -> Option<f64> {
    (1..=8).find_map(|k| {
        let l = read(&h.join(format!("temp{k}_label")))?;
        (l == label)
            .then(|| num(&h.join(format!("temp{k}_input"))))
            .flatten()
            .map(|m| m / 1000.0)
    })
}

/// Linux DRM GPU source.
pub struct LinuxGpu {
    devices: Vec<GpuDevice>,
    cards: Vec<PathBuf>,
    nvml: Option<NvmlSession>,
    nvml_error: Option<String>,
}

impl LinuxGpu {
    /// `devices` must come from enumerating the same `root`.
    pub fn new(root: &Path, devices: Vec<GpuDevice>) -> Self {
        let has_nvidia = devices.iter().any(|d| d.raw.vendor_id == Some(0x10DE));
        let (nvml, nvml_error) = if has_nvidia {
            match NvmlSession::open() {
                Ok(n) => (Some(n), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };
        // NVML knows an NVIDIA card's VRAM size, which sysfs does not
        // report; use it to complete identification.
        let devices = devices
            .into_iter()
            .map(|d| match (&nvml, d.raw.pci, d.raw.dedicated_memory_bytes) {
                (Some(n), Some(p), None) if d.raw.vendor_id == Some(0x10DE) => {
                    match n.read(p.bus, p.device).ok().and_then(|r| r.memory.ok()) {
                        Some((_, total)) => crate::hardware::gpu::with_dedicated_memory(&d, total),
                        None => d,
                    }
                }
                _ => d,
            })
            .collect();
        LinuxGpu {
            cards: crate::hardware::linux::card_dirs(root),
            devices,
            nvml,
            nvml_error,
        }
    }
}

fn sample_card(dev: &GpuDevice, card: &Path) -> GpuSample {
    let mut s = GpuSample::for_device(dev);
    if dev.kind == GpuKind::Software {
        return s;
    }
    let d = card.join("device");
    let driver = dev.raw.driver.as_deref().unwrap_or("");
    let src = |what: &str| format!("{driver} sysfs ({what})");
    match driver {
        "amdgpu" => {
            if let Some(u) = num(&d.join("gpu_busy_percent")) {
                s.utilization_pct = M::measured(u, "%", src("gpu_busy_percent"));
            }
            if let Some(b) = num(&d.join("mem_info_vram_used")) {
                s.dedicated_memory_used_bytes =
                    M::measured(b as u64, "B", src("mem_info_vram_used"));
            }
            if let Some(b) = num(&d.join("mem_info_gtt_used")) {
                s.shared_memory_used_bytes = M::measured(b as u64, "B", src("mem_info_gtt_used"));
            }
            if let Some(b) = num(&d.join("mem_info_gtt_total")) {
                s.shared_memory_total_bytes = M::measured(b as u64, "B", src("mem_info_gtt_total"));
            }
        }
        "i915" => {
            let f =
                num(&card.join("gt_act_freq_mhz")).or_else(|| num(&card.join("gt_cur_freq_mhz")));
            if let Some(f) = f {
                s.core_clock_mhz = M::measured(f, "MHz", src("gt_act_freq_mhz"));
            }
            s.utilization_pct =
                M::unavailable("i915 exposes utilization only through the perf PMU");
        }
        "xe" => {
            if let Some(f) = num(&d.join("tile0/gt0/freq0/act_freq")) {
                s.core_clock_mhz = M::measured(f, "MHz", src("act_freq"));
            }
            s.utilization_pct = M::unavailable("xe exposes utilization only through the perf PMU");
        }
        _ => {}
    }
    if let Some(h) = hwmon(&d) {
        let hs = |what: &str| format!("{driver} hwmon ({what})");
        let temp = temp_by_label(&h, "junction")
            .map(|t| (t, "junction"))
            .or_else(|| temp_by_label(&h, "edge").map(|t| (t, "edge")))
            .or_else(|| num(&h.join("temp1_input")).map(|m| (m / 1000.0, "temp1")));
        if let Some((t, which)) = temp {
            s.temperature_c = M::measured(t, "°C", hs(which));
        }
        if let Some(t) = temp_by_label(&h, "mem") {
            s.memory_temperature_c = M::measured(t, "°C", hs("mem"));
        }
        if let Some(uw) = num(&h.join("power1_average")).or_else(|| num(&h.join("power1_input"))) {
            s.power_w = M::measured(uw / 1e6, "W", hs("power1"));
        }
        if let Some(uw) = num(&h.join("power1_cap")) {
            s.power_limit_w = M::measured(uw / 1e6, "W", hs("power1_cap"));
        }
        if let Some(rpm) = num(&h.join("fan1_input")) {
            s.fan_speed_rpm = M::measured(rpm, "RPM", hs("fan1_input"));
        }
        if let Some(p) = num(&h.join("pwm1")) {
            s.fan_speed_pct = M::measured(p / 255.0 * 100.0, "%", hs("pwm1"));
        }
        if let Some(hz) = num(&h.join("freq1_input")) {
            s.core_clock_mhz = M::measured(hz / 1e6, "MHz", hs("freq1_input"));
        }
        if let Some(hz) = num(&h.join("freq2_input")) {
            s.memory_clock_mhz = M::measured(hz / 1e6, "MHz", hs("freq2_input"));
        }
        if let Some(mv) = num(&h.join("in0_input")) {
            s.voltage_v = M::measured(mv / 1000.0, "V", hs("in0_input"));
        }
    }
    s
}

impl GpuSource for LinuxGpu {
    fn devices(&self) -> &[GpuDevice] {
        &self.devices
    }

    fn sample(&mut self) -> Vec<GpuSample> {
        self.devices
            .iter()
            .enumerate()
            .map(|(i, dev)| {
                let mut s = match self.cards.get(i) {
                    Some(card) => sample_card(dev, card),
                    None => GpuSample::for_device(dev),
                };
                if dev.raw.vendor_id == Some(0x10DE) {
                    let read = match (&self.nvml, dev.raw.pci) {
                        (Some(n), Some(p)) => n.read(p.bus, p.device),
                        (Some(_), None) => Err("no PCI location to match with NVML".into()),
                        (None, _) => Err(self
                            .nvml_error
                            .clone()
                            .unwrap_or_else(|| "NVML not available".into())),
                    };
                    match read {
                        Ok(r) => nvidia::apply(&mut s, &r),
                        Err(e) => {
                            s.utilization_pct = crate::measurement::Measurement::unavailable(e)
                        }
                    }
                }
                s
            })
            .collect()
    }

    fn notes(&self) -> Vec<String> {
        self.nvml_error.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn amdgpu_and_i915() {
        let t = TempDir::new().unwrap();
        let r = t.path();
        write(r, "sys/class/drm/card0/device/vendor", "0x8086");
        write(
            r,
            "sys/class/drm/card0/device/uevent",
            "DRIVER=i915\nPCI_SLOT_NAME=0000:00:02.0\n",
        );
        write(r, "sys/class/drm/card0/gt_act_freq_mhz", "1300\n");
        let c1 = "sys/class/drm/card1/device";
        write(r, &format!("{c1}/vendor"), "0x1002");
        write(
            r,
            &format!("{c1}/uevent"),
            "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n",
        );
        write(r, &format!("{c1}/gpu_busy_percent"), "37\n");
        write(r, &format!("{c1}/mem_info_vram_total"), "8589934592\n");
        write(r, &format!("{c1}/mem_info_vram_used"), "1073741824\n");
        let h = format!("{c1}/hwmon/hwmon5");
        write(r, &format!("{h}/temp1_label"), "edge\n");
        write(r, &format!("{h}/temp1_input"), "55000\n");
        write(r, &format!("{h}/temp2_label"), "junction\n");
        write(r, &format!("{h}/temp2_input"), "68000\n");
        write(r, &format!("{h}/temp3_label"), "mem\n");
        write(r, &format!("{h}/temp3_input"), "60000\n");
        write(r, &format!("{h}/power1_average"), "95000000\n");
        write(r, &format!("{h}/power1_cap"), "180000000\n");
        write(r, &format!("{h}/fan1_input"), "1450\n");
        write(r, &format!("{h}/freq1_input"), "2100000000\n");
        write(r, &format!("{h}/in0_input"), "906\n");

        let devices: Vec<GpuDevice> = crate::hardware::linux::enumerate(r)
            .into_iter()
            .map(crate::hardware::gpu::identify)
            .collect();
        let mut src = LinuxGpu::new(r, devices);
        let s = src.sample();
        assert_eq!(s[0].core_clock_mhz.get(), Some(1300.0));
        assert!(!s[0].utilization_pct.is_measured());
        assert_eq!(s[1].utilization_pct.get(), Some(37.0));
        assert_eq!(s[1].temperature_c.get(), Some(68.0)); // junction preferred
        assert_eq!(s[1].memory_temperature_c.get(), Some(60.0));
        assert_eq!(s[1].power_w.get(), Some(95.0));
        assert_eq!(s[1].power_limit_w.get(), Some(180.0));
        assert_eq!(s[1].fan_speed_rpm.get(), Some(1450.0));
        assert_eq!(s[1].core_clock_mhz.get(), Some(2100.0));
        assert_eq!(s[1].voltage_v.get(), Some(0.906));
        assert_eq!(s[1].dedicated_memory_used_bytes.get(), Some(1 << 30));
        assert_eq!(s[1].dedicated_memory_total_bytes.get(), Some(8 << 30));
        // Not present in the tree: unavailable, never zero.
        assert!(!s[1].memory_clock_mhz.is_measured());
    }
}
