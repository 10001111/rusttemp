//! CPU sensor identification.
//!
//! Classification is based only on the reporting driver and the label it
//! assigns, following the Linux kernel hwmon driver documentation:
//!
//! - `coretemp` (Intel digital thermal sensors): `Package id N` (older kernels:
//!   `Physical id N`) is the package; `Core N` is a core.
//! - `k10temp` (AMD) and the out-of-tree `zenpower`: `Tctl` is the control
//!   temperature (may carry an offset), `Tdie` the die temperature, `TccdN` a
//!   core-complex die. Older families expose a single unlabelled Tctl value.
//! - Thermal zone `x86_pkg_temp`: Intel package temperature.
//! - Names of the form `cpu*thermal` (e.g. `cpu_thermal` on Raspberry Pi and
//!   other ARM SoCs) are explicitly CPU sensors.
//! - `acpitz`: ACPI thermal zone; never treated as a CPU sensor.
//!
//! Anything else is `Other`. Unknown sensors are never promoted to CPU.

use super::SensorKind;

/// Classifies one `tempN` channel of an hwmon chip.
pub fn classify_hwmon(chip: &str, label: Option<&str>) -> SensorKind {
    let label = label.map(str::trim).unwrap_or("");
    match chip {
        "coretemp" => {
            if label.starts_with("Package id") || label.starts_with("Physical id") {
                SensorKind::CpuPackage
            } else if label.starts_with("Core ") {
                SensorKind::CpuCore
            } else {
                SensorKind::Cpu
            }
        }
        "k10temp" | "zenpower" => {
            if label.is_empty() || label == "Tctl" {
                SensorKind::CpuControl
            } else if label == "Tdie" || label.starts_with("Tccd") {
                SensorKind::CpuDie
            } else {
                SensorKind::Cpu
            }
        }
        "acpitz" => SensorKind::AcpiThermalZone,
        name if is_cpu_thermal_name(name) => SensorKind::Cpu,
        _ => SensorKind::Other,
    }
}

/// Classifies a `/sys/class/thermal/thermal_zoneN` by its `type`.
pub fn classify_thermal_zone(zone_type: &str) -> SensorKind {
    match zone_type.trim() {
        "x86_pkg_temp" => SensorKind::CpuPackage,
        "acpitz" => SensorKind::AcpiThermalZone,
        name if is_cpu_thermal_name(name) => SensorKind::Cpu,
        _ => SensorKind::Other,
    }
}

/// `cpu_thermal`, `cpu-thermal`, `cpu0-thermal`, `cpu-big0-thermal`, ...
fn is_cpu_thermal_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("cpu") && n.ends_with("thermal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use SensorKind::*;

    #[test]
    fn intel_coretemp() {
        assert_eq!(classify_hwmon("coretemp", Some("Package id 0")), CpuPackage);
        assert_eq!(
            classify_hwmon("coretemp", Some("Physical id 1")),
            CpuPackage
        );
        assert_eq!(classify_hwmon("coretemp", Some("Core 0")), CpuCore);
        assert_eq!(classify_hwmon("coretemp", Some("Core 12\n")), CpuCore);
        assert_eq!(classify_hwmon("coretemp", None), Cpu);
    }

    #[test]
    fn amd_k10temp_and_zenpower() {
        assert_eq!(classify_hwmon("k10temp", Some("Tctl")), CpuControl);
        assert_eq!(classify_hwmon("k10temp", Some("Tdie")), CpuDie);
        assert_eq!(classify_hwmon("k10temp", Some("Tccd1")), CpuDie);
        assert_eq!(classify_hwmon("k10temp", Some("Tccd8")), CpuDie);
        assert_eq!(classify_hwmon("k10temp", None), CpuControl);
        assert_eq!(classify_hwmon("zenpower", Some("Tdie")), CpuDie);
    }

    #[test]
    fn non_cpu_chips_are_not_promoted() {
        assert_eq!(classify_hwmon("acpitz", None), AcpiThermalZone);
        assert_eq!(classify_hwmon("nvme", Some("Composite")), Other);
        assert_eq!(classify_hwmon("amdgpu", Some("edge")), Other);
        assert_eq!(classify_hwmon("iwlwifi_1", None), Other);
        assert_eq!(classify_hwmon("pch_cannonlake", None), Other);
        // Label alone never makes a sensor a CPU sensor.
        assert_eq!(classify_hwmon("nct6798", Some("CPUTIN")), Other);
        assert_eq!(classify_hwmon("it8688", Some("Core 0")), Other);
    }

    #[test]
    fn arm_cpu_thermal() {
        assert_eq!(classify_hwmon("cpu_thermal", None), Cpu);
        assert_eq!(classify_hwmon("cpu0_thermal", None), Cpu);
        assert_eq!(classify_thermal_zone("cpu-thermal"), Cpu);
        assert_eq!(classify_thermal_zone("cpu-big0-thermal"), Cpu);
        assert_eq!(classify_thermal_zone("gpu-thermal"), Other);
    }

    #[test]
    fn thermal_zones() {
        assert_eq!(classify_thermal_zone("x86_pkg_temp"), CpuPackage);
        assert_eq!(classify_thermal_zone("x86_pkg_temp\n"), CpuPackage);
        assert_eq!(classify_thermal_zone("acpitz"), AcpiThermalZone);
        assert_eq!(classify_thermal_zone("INT3400 Thermal"), Other);
        assert_eq!(classify_thermal_zone("iwlwifi_1"), Other);
    }
}
