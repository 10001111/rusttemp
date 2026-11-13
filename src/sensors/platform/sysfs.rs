//! Linux sysfs backend.
//!
//! Interfaces (kernel ABI documentation: `Documentation/hwmon/sysfs-interface.rst`
//! and `Documentation/ABI/testing/sysfs-class-thermal`):
//!
//! - `/sys/class/hwmon/hwmonN/name`: driver / chip name.
//! - `/sys/class/hwmon/hwmonN/tempK_input`: temperature in millidegrees Celsius.
//! - `/sys/class/hwmon/hwmonN/tempK_label`: optional channel label.
//! - `/sys/class/hwmon/hwmonN/tempK_max`, `tempK_crit`: optional limits (m°C).
//! - Older drivers place these attributes in `hwmonN/device/` instead.
//! - `/sys/class/thermal/thermal_zoneN/type` and `temp` (m°C), plus
//!   `trip_point_K_type` / `trip_point_K_temp`.
//!
//! Thermal zones whose `type` equals the name of an hwmon chip already found
//! (typically `acpitz`) are skipped, because the kernel registers those zones
//! as hwmon devices too and they would otherwise be listed twice.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::errors::{Error, Result};
use crate::sensors::classify::{classify_hwmon, classify_thermal_zone};
use crate::sensors::{
    HardwareLimits, Reading, SensorBackend, SensorInfo, ThrottleCounters, millidegrees_to_celsius,
};

/// Reads hwmon and thermal-zone temperatures from a sysfs tree.
pub struct SysfsBackend {
    root: PathBuf,
    sensors: Vec<SensorInfo>,
    inputs: Vec<PathBuf>,
    counters: Option<ThrottleCounters>,
}

impl SysfsBackend {
    /// Discovers sensors below `root` (normally `/`).
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(Error::Io {
                path: root.to_path_buf(),
                source: io::Error::new(io::ErrorKind::NotFound, "sysfs root is not a directory"),
            });
        }
        let mut sensors = Vec::new();
        let mut inputs = Vec::new();

        let hwmon_chips = discover_hwmon(&root.join("sys/class/hwmon"), &mut sensors, &mut inputs)?;
        discover_thermal_zones(
            &root.join("sys/class/thermal"),
            &hwmon_chips,
            &mut sensors,
            &mut inputs,
        )?;

        Ok(SysfsBackend {
            root: root.to_path_buf(),
            sensors,
            inputs,
            counters: None,
        })
    }
}

impl SensorBackend for SysfsBackend {
    fn name(&self) -> &'static str {
        "linux-sysfs"
    }

    fn sensors(&self) -> &[SensorInfo] {
        &self.sensors
    }

    fn read(&mut self) -> Vec<Reading> {
        self.counters = read_throttle_counters(&self.root);
        self.inputs.iter().map(|p| read_temperature(p)).collect()
    }

    fn throttle_counters(&self) -> Option<ThrottleCounters> {
        self.counters
    }
}

/// Reads `sys/devices/system/cpu/cpuN/thermal_throttle/{core,package}_throttle_count`
/// (Intel, kernel `therm_throt`). `None` when no CPU exposes them.
pub fn read_throttle_counters(root: &Path) -> Option<ThrottleCounters> {
    let cpus = numbered_entries(&root.join("sys/devices/system/cpu"), "cpu").ok()?;
    let mut c = ThrottleCounters::default();
    for (_, _, dir) in cpus {
        let tt = dir.join("thermal_throttle");
        let count = |f: &str| -> Option<u64> { read_trimmed(&tt.join(f))?.parse().ok() };
        let core = count("core_throttle_count");
        let pkg = count("package_throttle_count");
        if core.is_none() && pkg.is_none() {
            continue;
        }
        c.cpus_reporting += 1;
        if let Some(n) = core {
            c.core_events_max = c.core_events_max.max(n);
            if n > 0 {
                c.cpus_with_core_events += 1;
            }
        }
        if let Some(n) = pkg {
            c.package_events = c.package_events.max(n);
        }
    }
    (c.cpus_reporting > 0).then_some(c)
}

/// Reads a millidegree-Celsius sysfs attribute.
pub fn read_temperature(path: &Path) -> Reading {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Reading::Unavailable("sensor file no longer exists".into());
        }
        Err(e) => return Reading::Unavailable(format!("read failed: {e}")),
    };
    match raw.trim().parse::<i64>() {
        Ok(milli) => Reading::from_celsius(millidegrees_to_celsius(milli)),
        Err(_) => Reading::Unavailable(format!("invalid sensor value {:?}", raw.trim())),
    }
}

/// Lists directory entries whose name is `prefix` followed by digits, sorted numerically.
fn numbered_entries(dir: &Path, prefix: &str) -> Result<Vec<(u64, String, PathBuf)>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(Error::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(n) = name
            .strip_prefix(prefix)
            .and_then(|s| s.parse::<u64>().ok())
        {
            out.push((n, name, entry.path()));
        }
    }
    out.sort_by_key(|(n, _, _)| *n);
    Ok(out)
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Reads an optional limit attribute; invalid or implausible values are ignored.
fn read_limit(path: &Path) -> Option<f64> {
    read_temperature(path).celsius()
}

/// Channel numbers K for which `tempK_input` exists in `dir`, sorted.
fn temp_channels(dir: &Path) -> Vec<u64> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut channels: Vec<u64> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("temp")?
                .strip_suffix("_input")?
                .parse::<u64>()
                .ok()
        })
        .collect();
    channels.sort_unstable();
    channels
}

fn discover_hwmon(
    class_dir: &Path,
    sensors: &mut Vec<SensorInfo>,
    inputs: &mut Vec<PathBuf>,
) -> Result<HashSet<String>> {
    let mut chips = HashSet::new();
    for (_, dir_name, hwmon_dir) in numbered_entries(class_dir, "hwmon")? {
        // Attributes live in hwmonN/ on current kernels, hwmonN/device/ on old drivers.
        let device_dir = hwmon_dir.join("device");
        let attr_dir = if temp_channels(&hwmon_dir).is_empty() && device_dir.is_dir() {
            device_dir.clone()
        } else {
            hwmon_dir.clone()
        };
        let chip = read_trimmed(&hwmon_dir.join("name"))
            .or_else(|| read_trimmed(&device_dir.join("name")))
            .unwrap_or_else(|| "unknown".to_string());
        chips.insert(chip.clone());

        for k in temp_channels(&attr_dir) {
            let label = read_trimmed(&attr_dir.join(format!("temp{k}_label")));
            let input = attr_dir.join(format!("temp{k}_input"));
            sensors.push(SensorInfo {
                id: format!("{dir_name}/temp{k}"),
                kind: classify_hwmon(&chip, label.as_deref()),
                label: label.unwrap_or_else(|| format!("temp{k}")),
                chip: chip.clone(),
                source: input.display().to_string(),
                limits: HardwareLimits {
                    max_c: read_limit(&attr_dir.join(format!("temp{k}_max"))),
                    crit_c: read_limit(&attr_dir.join(format!("temp{k}_crit"))),
                },
            });
            inputs.push(input);
        }
    }
    Ok(chips)
}

fn discover_thermal_zones(
    class_dir: &Path,
    hwmon_chips: &HashSet<String>,
    sensors: &mut Vec<SensorInfo>,
    inputs: &mut Vec<PathBuf>,
) -> Result<()> {
    for (_, dir_name, zone_dir) in numbered_entries(class_dir, "thermal_zone")? {
        let input = zone_dir.join("temp");
        if !input.is_file() {
            continue;
        }
        let zone_type = read_trimmed(&zone_dir.join("type")).unwrap_or_else(|| "unknown".into());
        if hwmon_chips.contains(&zone_type) {
            continue; // Already reported through hwmon.
        }
        sensors.push(SensorInfo {
            id: dir_name.clone(),
            kind: classify_thermal_zone(&zone_type),
            label: zone_type.clone(),
            chip: format!("thermal:{zone_type}"),
            source: input.display().to_string(),
            limits: HardwareLimits {
                max_c: None,
                crit_c: critical_trip_point(&zone_dir),
            },
        });
        inputs.push(input);
    }
    Ok(())
}

/// Temperature of the first trip point whose type is `critical`.
fn critical_trip_point(zone_dir: &Path) -> Option<f64> {
    (0..32).find_map(|k| {
        let ty = read_trimmed(&zone_dir.join(format!("trip_point_{k}_type")))?;
        if ty == "critical" {
            read_limit(&zone_dir.join(format!("trip_point_{k}_temp")))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::SensorKind;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// Intel laptop layout: coretemp package + 2 cores, acpitz, nvme.
    fn intel_tree() -> TempDir {
        let t = TempDir::new().unwrap();
        let r = t.path();
        write(r, "sys/class/hwmon/hwmon0/name", "acpitz\n");
        write(r, "sys/class/hwmon/hwmon0/temp1_input", "27800\n");
        write(r, "sys/class/hwmon/hwmon0/temp1_crit", "119000\n");
        write(r, "sys/class/hwmon/hwmon2/name", "nvme\n");
        write(r, "sys/class/hwmon/hwmon2/temp1_input", "38850\n");
        write(r, "sys/class/hwmon/hwmon2/temp1_label", "Composite\n");
        write(r, "sys/class/hwmon/hwmon10/name", "coretemp\n");
        write(r, "sys/class/hwmon/hwmon10/temp1_input", "52000\n");
        write(r, "sys/class/hwmon/hwmon10/temp1_label", "Package id 0\n");
        write(r, "sys/class/hwmon/hwmon10/temp1_max", "100000\n");
        write(r, "sys/class/hwmon/hwmon10/temp1_crit", "100000\n");
        write(r, "sys/class/hwmon/hwmon10/temp2_input", "49000\n");
        write(r, "sys/class/hwmon/hwmon10/temp2_label", "Core 0\n");
        write(r, "sys/class/hwmon/hwmon10/temp10_input", "51000\n");
        write(r, "sys/class/hwmon/hwmon10/temp10_label", "Core 8\n");
        // Thermal zones: acpitz duplicates hwmon0, x86_pkg_temp is new.
        write(r, "sys/class/thermal/thermal_zone0/type", "acpitz\n");
        write(r, "sys/class/thermal/thermal_zone0/temp", "27800\n");
        write(r, "sys/class/thermal/thermal_zone1/type", "x86_pkg_temp\n");
        write(r, "sys/class/thermal/thermal_zone1/temp", "52000\n");
        write(
            r,
            "sys/class/thermal/thermal_zone1/trip_point_0_type",
            "passive\n",
        );
        write(
            r,
            "sys/class/thermal/thermal_zone1/trip_point_0_temp",
            "0\n",
        );
        write(
            r,
            "sys/class/thermal/thermal_zone1/trip_point_1_type",
            "critical\n",
        );
        write(
            r,
            "sys/class/thermal/thermal_zone1/trip_point_1_temp",
            "105000\n",
        );
        // Cooling devices must be ignored.
        write(r, "sys/class/thermal/cooling_device0/type", "Processor\n");
        t
    }

    #[test]
    fn discovers_and_identifies_intel_sensors() {
        let t = intel_tree();
        let b = SysfsBackend::new(t.path()).unwrap();
        let s = b.sensors();
        let ids: Vec<&str> = s.iter().map(|x| x.id.as_str()).collect();
        // hwmon sorted numerically (hwmon2 before hwmon10), channels numerically.
        assert_eq!(
            ids,
            [
                "hwmon0/temp1",
                "hwmon2/temp1",
                "hwmon10/temp1",
                "hwmon10/temp2",
                "hwmon10/temp10",
                "thermal_zone1"
            ]
        );
        assert_eq!(s[0].kind, SensorKind::AcpiThermalZone);
        assert_eq!(s[0].label, "temp1");
        assert_eq!(s[0].limits.crit_c, Some(119.0));
        assert_eq!(s[1].kind, SensorKind::Other);
        assert_eq!(s[1].label, "Composite");
        assert_eq!(s[2].kind, SensorKind::CpuPackage);
        assert_eq!(s[2].label, "Package id 0");
        assert_eq!(s[2].chip, "coretemp");
        assert_eq!(s[2].limits.max_c, Some(100.0));
        assert_eq!(s[3].kind, SensorKind::CpuCore);
        assert_eq!(s[4].label, "Core 8");
        assert_eq!(s[5].kind, SensorKind::CpuPackage);
        assert_eq!(s[5].chip, "thermal:x86_pkg_temp");
        assert_eq!(s[5].limits.crit_c, Some(105.0));
        assert_eq!(s.iter().filter(|x| x.kind.is_cpu()).count(), 4);
    }

    #[test]
    fn reads_and_converts_millidegrees() {
        let t = intel_tree();
        let mut b = SysfsBackend::new(t.path()).unwrap();
        let r = b.read();
        assert_eq!(r.len(), b.sensors().len());
        assert_eq!(r[0], Reading::Celsius(27.8));
        assert_eq!(r[2], Reading::Celsius(52.0));
        assert_eq!(r[3], Reading::Celsius(49.0));
        // Values change between reads.
        write(t.path(), "sys/class/hwmon/hwmon10/temp1_input", "61125\n");
        assert_eq!(b.read()[2], Reading::Celsius(61.125));
    }

    #[test]
    fn amd_k10temp_tree() {
        let t = TempDir::new().unwrap();
        let r = t.path();
        write(r, "sys/class/hwmon/hwmon1/name", "k10temp\n");
        write(r, "sys/class/hwmon/hwmon1/temp1_input", "61375\n");
        write(r, "sys/class/hwmon/hwmon1/temp1_label", "Tctl\n");
        write(r, "sys/class/hwmon/hwmon1/temp3_input", "55250\n");
        write(r, "sys/class/hwmon/hwmon1/temp3_label", "Tccd1\n");
        let mut b = SysfsBackend::new(r).unwrap();
        let kinds: Vec<SensorKind> = b.sensors().iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [SensorKind::CpuControl, SensorKind::CpuDie]);
        assert_eq!(
            b.read(),
            [Reading::Celsius(61.375), Reading::Celsius(55.25)]
        );
    }

    #[test]
    fn legacy_device_subdirectory() {
        let t = TempDir::new().unwrap();
        let r = t.path();
        write(r, "sys/class/hwmon/hwmon0/device/name", "coretemp\n");
        write(r, "sys/class/hwmon/hwmon0/device/temp2_input", "40000\n");
        write(r, "sys/class/hwmon/hwmon0/device/temp2_label", "Core 0\n");
        let mut b = SysfsBackend::new(r).unwrap();
        assert_eq!(b.sensors().len(), 1);
        assert_eq!(b.sensors()[0].kind, SensorKind::CpuCore);
        assert_eq!(b.read(), [Reading::Celsius(40.0)]);
    }

    #[test]
    fn missing_sensor_file_becomes_unavailable() {
        let t = intel_tree();
        let mut b = SysfsBackend::new(t.path()).unwrap();
        fs::remove_file(t.path().join("sys/class/hwmon/hwmon10/temp2_input")).unwrap();
        let r = b.read();
        assert!(matches!(&r[3], Reading::Unavailable(m) if m.contains("no longer exists")));
        // Other sensors are unaffected.
        assert_eq!(r[2], Reading::Celsius(52.0));
    }

    #[test]
    fn invalid_values_become_unavailable() {
        let t = TempDir::new().unwrap();
        let r = t.path();
        write(r, "sys/class/hwmon/hwmon0/name", "coretemp\n");
        write(r, "sys/class/hwmon/hwmon0/temp1_input", "garbage\n");
        write(r, "sys/class/hwmon/hwmon0/temp2_input", "\n");
        write(r, "sys/class/hwmon/hwmon0/temp3_input", "-273150\n");
        write(r, "sys/class/hwmon/hwmon0/temp4_input", "45.5\n");
        let mut b = SysfsBackend::new(r).unwrap();
        let readings = b.read();
        assert_eq!(readings.len(), 4);
        for reading in &readings {
            assert!(matches!(reading, Reading::Unavailable(_)), "{reading:?}");
        }
        assert!(matches!(&readings[0], Reading::Unavailable(m) if m.contains("invalid")));
        assert!(matches!(&readings[2], Reading::Unavailable(m) if m.contains("implausible")));
    }

    #[test]
    fn empty_and_missing_trees_have_no_sensors() {
        let t = TempDir::new().unwrap();
        assert!(SysfsBackend::new(t.path()).unwrap().sensors().is_empty());
        // hwmon devices without temperature channels (WSL2: AC adapter, battery).
        write(t.path(), "sys/class/hwmon/hwmon0/name", "AC1\n");
        write(t.path(), "sys/class/hwmon/hwmon1/name", "BAT1\n");
        write(t.path(), "sys/class/hwmon/hwmon1/in0_input", "12000\n");
        assert!(SysfsBackend::new(t.path()).unwrap().sensors().is_empty());
    }

    #[test]
    fn throttle_counters() {
        let t = TempDir::new().unwrap();
        let r = t.path();
        assert_eq!(read_throttle_counters(r), None);
        let base = "sys/devices/system/cpu";
        write(
            r,
            &format!("{base}/cpu0/thermal_throttle/core_throttle_count"),
            "0
",
        );
        write(
            r,
            &format!("{base}/cpu0/thermal_throttle/package_throttle_count"),
            "4
",
        );
        write(
            r,
            &format!("{base}/cpu1/thermal_throttle/core_throttle_count"),
            "12
",
        );
        write(
            r,
            &format!("{base}/cpu1/thermal_throttle/package_throttle_count"),
            "4
",
        );
        write(
            r,
            &format!("{base}/cpu2/thermal_throttle/core_throttle_count"),
            "3
",
        );
        write(
            r,
            &format!("{base}/cpufreq/policy0/scaling_driver"),
            "intel_pstate
",
        );
        let c = read_throttle_counters(r).unwrap();
        assert_eq!(
            c,
            ThrottleCounters {
                cpus_reporting: 3,
                core_events_max: 12,
                cpus_with_core_events: 2,
                package_events: 4,
            }
        );
        // The backend captures them on read.
        let mut b = SysfsBackend::new(r).unwrap();
        assert_eq!(b.throttle_counters(), None);
        b.read();
        assert_eq!(b.throttle_counters(), Some(c));
    }

    #[test]
    fn nonexistent_root_is_an_error() {
        let t = TempDir::new().unwrap();
        assert!(SysfsBackend::new(t.path().join("nope")).is_err());
    }
}
