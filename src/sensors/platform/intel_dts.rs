//! Intel Digital Thermal Sensor (DTS) register decoding.
//!
//! Reference: Intel 64 and IA-32 Architectures Software Developer's Manual,
//! Vol. 3B §15.8 / Vol. 4 (MSR listing):
//!
//! - `IA32_THERM_STATUS` (0x19C), per core: bits 22:16 = digital readout,
//!   i.e. degrees **below** TjMax; bit 31 = reading valid.
//! - `IA32_PACKAGE_THERM_STATUS` (0x1B1), per package: bits 22:16 = package
//!   digital readout (no valid bit). Present when CPUID.06H:EAX[6] = 1.
//! - `MSR_TEMPERATURE_TARGET` (0x1A2): bits 23:16 = TjMax, the temperature
//!   the readouts are relative to.
//!
//! Throttle flags (same bit positions in both status MSRs): bit 0 thermal
//! status (TCC active now), bit 1 its sticky log, bit 2 PROCHOT#/FORCEPR#
//! event, bit 3 its log, bit 4 critical temperature, bit 5 its log, bit 10
//! power limitation status, bit 11 its log (power bits valid when
//! CPUID.06H:EAX[4] (PLN) = 1; for the package also requires EAX[6] (PTM)).
//!
//! Temperature = TjMax − readout, with 1 °C resolution. This is the same
//! source Linux's `coretemp` driver uses. If TjMax cannot be read, no
//! temperature is produced: RustTemp never substitutes an assumed TjMax.
//!
//! This module is plain arithmetic so it is compiled and tested on every OS;
//! register access lives in the Windows PawnIO backend.

/// `IA32_THERM_STATUS`.
pub const MSR_IA32_THERM_STATUS: u32 = 0x19C;
/// `IA32_PACKAGE_THERM_STATUS`.
pub const MSR_IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
/// `MSR_TEMPERATURE_TARGET`.
pub const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;

const READING_VALID: u64 = 1 << 31;

use crate::sensors::ThrottleFlags;

/// Decodes the throttle flags of `IA32_THERM_STATUS` or
/// `IA32_PACKAGE_THERM_STATUS`. `power_bits_valid` says whether bits 10/11
/// are defined on this CPU.
pub fn decode_throttle_flags(raw: u64, power_bits_valid: bool) -> ThrottleFlags {
    let bit = |n: u32| raw & (1u64 << n) != 0;
    ThrottleFlags {
        thermal_now: bit(0),
        thermal_logged: bit(1),
        prochot_now: bit(2),
        prochot_logged: bit(3),
        critical_now: bit(4),
        critical_logged: bit(5),
        power_limit_now: power_bits_valid.then(|| bit(10)),
        power_limit_logged: power_bits_valid.then(|| bit(11)),
    }
}

/// TjMax in °C from `MSR_TEMPERATURE_TARGET`, or `None` if the field is zero.
pub fn decode_tjmax(raw: u64) -> Option<f64> {
    let t = (raw >> 16) & 0xFF;
    (t != 0).then_some(t as f64)
}

/// Core temperature from `IA32_THERM_STATUS`.
pub fn decode_core_temperature(raw: u64, tjmax: f64) -> Result<f64, &'static str> {
    if raw & READING_VALID == 0 {
        return Err("digital readout not valid");
    }
    Ok(tjmax - ((raw >> 16) & 0x7F) as f64)
}

/// Package temperature from `IA32_PACKAGE_THERM_STATUS`.
pub fn decode_package_temperature(raw: u64, tjmax: f64) -> f64 {
    tjmax - ((raw >> 16) & 0x7F) as f64
}

/// Thermal capabilities from CPUID leaf 6 EAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalCpuid {
    /// CPUID.06H:EAX[0]: digital temperature sensor present.
    pub dts: bool,
    /// CPUID.06H:EAX[4]: power limit notification (power-limit status bits).
    pub power_limit_notification: bool,
    /// CPUID.06H:EAX[6]: package thermal management (package DTS) present.
    pub package_dts: bool,
}

impl ThermalCpuid {
    /// Decodes CPUID.06H:EAX.
    pub fn from_leaf6_eax(eax: u32) -> Self {
        ThermalCpuid {
            dts: eax & 1 != 0,
            power_limit_notification: eax & (1 << 4) != 0,
            package_dts: eax & (1 << 6) != 0,
        }
    }
}

/// A physical core as reported by the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreTopology {
    /// Processor group of the logical CPU used to read this core.
    pub group: u16,
    /// Index of that logical CPU inside its group.
    pub cpu: u32,
    /// OS efficiency class: higher means a more performant core.
    pub efficiency_class: u8,
}

/// Labels for cores in enumeration order. On hybrid CPUs (mixed efficiency
/// classes) the highest class is labelled P-core and the others E-core.
pub fn core_labels(cores: &[CoreTopology]) -> Vec<String> {
    let max = cores.iter().map(|c| c.efficiency_class).max();
    let min = cores.iter().map(|c| c.efficiency_class).min();
    let hybrid = max != min;
    cores
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if !hybrid {
                format!("Core {i}")
            } else if Some(c.efficiency_class) == max {
                format!("Core {i} (P-core)")
            } else {
                format!("Core {i} (E-core)")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tjmax() {
        // TjMax 100 °C in bits 23:16, TCC offset bits elsewhere ignored.
        assert_eq!(decode_tjmax(0x0064_0000), Some(100.0));
        assert_eq!(decode_tjmax(0x0564_0A00), Some(100.0));
        assert_eq!(decode_tjmax(0x005F_0000), Some(95.0));
        assert_eq!(decode_tjmax(0), None);
    }

    #[test]
    fn core_temperature() {
        // Valid bit set, readout 0x2A = 42 below TjMax 100 -> 58 °C.
        let raw = (1u64 << 31) | (0x2A << 16) | 0x0000_0001;
        assert_eq!(decode_core_temperature(raw, 100.0), Ok(58.0));
        // Readout 0: at TjMax.
        assert_eq!(decode_core_temperature(1 << 31, 100.0), Ok(100.0));
        // Resolution bits (30:27) and high bits do not leak into the readout.
        let raw = (1u64 << 31) | (0xF << 27) | (0x7F << 16) | (0xFFFF_FFFF << 32);
        assert_eq!(decode_core_temperature(raw, 100.0), Ok(-27.0));
        // Invalid reading.
        assert!(decode_core_temperature(0x2A << 16, 100.0).is_err());
    }

    #[test]
    fn package_temperature() {
        assert_eq!(decode_package_temperature(0x0030_0000, 100.0), 52.0);
        assert_eq!(decode_package_temperature(0x0000_0000, 95.0), 95.0);
    }

    #[test]
    fn cpuid_leaf6() {
        assert_eq!(
            ThermalCpuid::from_leaf6_eax(0b101_0001),
            ThermalCpuid {
                dts: true,
                power_limit_notification: true,
                package_dts: true
            }
        );
        assert_eq!(
            ThermalCpuid::from_leaf6_eax(0b1),
            ThermalCpuid {
                dts: true,
                power_limit_notification: false,
                package_dts: false
            }
        );
        assert!(!ThermalCpuid::from_leaf6_eax(0).dts);
    }

    #[test]
    fn throttle_flags() {
        // Valid reading, readout 30, thermal log + power limit now + power log.
        let raw = (1u64 << 31) | (30 << 16) | (1 << 1) | (1 << 10) | (1 << 11);
        let f = decode_throttle_flags(raw, true);
        assert!(!f.thermal_now && f.thermal_logged);
        assert!(!f.prochot_now && !f.prochot_logged);
        assert!(!f.critical_now && !f.critical_logged);
        assert_eq!(
            (f.power_limit_now, f.power_limit_logged),
            (Some(true), Some(true))
        );
        // Power bits undefined on this CPU.
        let f = decode_throttle_flags(raw, false);
        assert_eq!((f.power_limit_now, f.power_limit_logged), (None, None));
        // Everything set in bits 0..5.
        let f = decode_throttle_flags(0b11_1111, true);
        assert!(f.thermal_now && f.prochot_now && f.critical_now);
        assert!(f.thermal_logged && f.prochot_logged && f.critical_logged);
        assert_eq!(f.power_limit_now, Some(false));
        // Readout and valid bits never leak into the flags.
        assert_eq!(
            decode_throttle_flags((1 << 31) | (0x7F << 16), false),
            ThrottleFlags::default()
        );
    }

    fn core(class: u8) -> CoreTopology {
        CoreTopology {
            group: 0,
            cpu: 0,
            efficiency_class: class,
        }
    }

    #[test]
    fn labels_for_uniform_and_hybrid_cpus() {
        assert_eq!(core_labels(&[core(0), core(0)]), ["Core 0", "Core 1"]);
        assert_eq!(
            core_labels(&[core(1), core(1), core(0)]),
            ["Core 0 (P-core)", "Core 1 (P-core)", "Core 2 (E-core)"]
        );
        assert!(core_labels(&[]).is_empty());
    }
}
