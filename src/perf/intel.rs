//! Intel power and clock register decoding (pure arithmetic, all OS).
//!
//! References: Intel SDM Vol. 3B §15.10 (RAPL), Vol. 4 (MSR listing), and the
//! Linux `turbostat` tool for the frequency formulas.
//!
//! - `MSR_RAPL_POWER_UNIT` (0x606): power unit = 1/2^bits[3:0] W, energy unit
//!   = 1/2^bits[12:8] J.
//! - `MSR_PKG_ENERGY_STATUS` (0x611), `MSR_PP0_ENERGY_STATUS` (0x639):
//!   bits[31:0] cumulative energy in energy units; wraps at 2^32.
//! - `MSR_PKG_POWER_LIMIT` (0x610): PL1 = bits[14:0] × power unit, enabled
//!   bit 15; PL2 = bits[46:32] × power unit, enabled bit 47.
//! - `MSR_PKG_POWER_INFO` (0x614): thermal spec power (TDP) = bits[14:0] × power unit.
//! - `MSR_PLATFORM_INFO` (0xCE): bits[15:8] maximum non-turbo ratio.
//! - `MSR_TURBO_RATIO_LIMIT` (0x1AD): eight ratio bytes; the largest is the
//!   maximum single-group turbo ratio.
//! - `IA32_MPERF` (0xE7) counts at a fixed reference rate while the CPU is
//!   active; `IA32_APERF` (0xE8) counts actual cycles. Busy frequency =
//!   reference × ΔAPERF / ΔMPERF (turbostat "Bzy_MHz"). turbostat uses the
//!   `MSR_PLATFORM_INFO` ratio as the reference; on the i7-13620H test
//!   machine the measured MPERF rate matched the TSC rate (2918 MHz) instead
//!   of the 2900 MHz ratio, so the Windows backend uses the measured TSC rate.
//!
//! Ratios are converted to MHz with a 100 MHz bus clock, which is the
//! reference clock on all Intel Core processors since 2011 (BCLK
//! overclocking would make these values wrong).

/// `IA32_PERF_STATUS`: bits 15:0 current performance state (ratio in 15:8),
/// bits 47:32 current core voltage in units of 2⁻¹³ V on Intel Core
/// processors since Sandy Bridge. Validated on the i7-13620H: the voltage
/// tracked the ratio (0.71 V at ratio 23, 1.18 V at ratio 46).
pub const MSR_IA32_PERF_STATUS: u32 = 0x198;
pub const MSR_IA32_MPERF: u32 = 0xE7;
pub const MSR_IA32_APERF: u32 = 0xE8;
pub const MSR_PLATFORM_INFO: u32 = 0xCE;
pub const MSR_TURBO_RATIO_LIMIT: u32 = 0x1AD;
pub const MSR_RAPL_POWER_UNIT: u32 = 0x606;
pub const MSR_PKG_POWER_LIMIT: u32 = 0x610;
pub const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
pub const MSR_PKG_POWER_INFO: u32 = 0x614;
pub const MSR_PP0_ENERGY_STATUS: u32 = 0x639;
/// `MSR_PP1_ENERGY_STATUS`: graphics power plane (integrated GPU) on client
/// CPUs. Validated on the i7-13620H: 0.05–0.84 W, flat under CPU-only load.
pub const MSR_PP1_ENERGY_STATUS: u32 = 0x641;

/// Bus clock assumed for ratio → MHz conversion.
pub const BCLK_MHZ: f64 = 100.0;

/// RAPL units from `MSR_RAPL_POWER_UNIT`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaplUnits {
    pub power_w: f64,
    pub energy_j: f64,
}

/// Decodes `MSR_RAPL_POWER_UNIT`.
pub fn decode_rapl_units(raw: u64) -> RaplUnits {
    RaplUnits {
        power_w: 1.0 / (1u64 << (raw & 0xF)) as f64,
        energy_j: 1.0 / (1u64 << ((raw >> 8) & 0x1F)) as f64,
    }
}

/// Energy consumed between two 32-bit counter readings, handling one wrap.
pub fn energy_delta(previous: u32, current: u32) -> u32 {
    current.wrapping_sub(previous)
}

/// Average power in watts from two energy counter readings `seconds` apart.
pub fn average_power(previous: u32, current: u32, units: RaplUnits, seconds: f64) -> Option<f64> {
    (seconds > 0.0).then(|| energy_delta(previous, current) as f64 * units.energy_j / seconds)
}

/// Package power limits decoded from `MSR_PKG_POWER_LIMIT`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PkgPowerLimit {
    pub pl1_w: f64,
    pub pl1_enabled: bool,
    pub pl2_w: f64,
    pub pl2_enabled: bool,
}

/// Decodes `MSR_PKG_POWER_LIMIT`.
pub fn decode_pkg_power_limit(raw: u64, units: RaplUnits) -> PkgPowerLimit {
    PkgPowerLimit {
        pl1_w: (raw & 0x7FFF) as f64 * units.power_w,
        pl1_enabled: raw & (1 << 15) != 0,
        pl2_w: ((raw >> 32) & 0x7FFF) as f64 * units.power_w,
        pl2_enabled: raw & (1 << 47) != 0,
    }
}

/// TDP ("thermal spec power") from `MSR_PKG_POWER_INFO`, `None` if zero.
pub fn decode_tdp(raw: u64, units: RaplUnits) -> Option<f64> {
    let v = (raw & 0x7FFF) as f64 * units.power_w;
    (v > 0.0).then_some(v)
}

/// Base (maximum non-turbo) frequency in MHz from `MSR_PLATFORM_INFO`.
pub fn decode_base_mhz(raw: u64) -> Option<f64> {
    let ratio = (raw >> 8) & 0xFF;
    (ratio != 0).then_some(ratio as f64 * BCLK_MHZ)
}

/// Maximum turbo frequency in MHz from `MSR_TURBO_RATIO_LIMIT`.
pub fn decode_max_turbo_mhz(raw: u64) -> Option<f64> {
    let max = (0..8).map(|i| (raw >> (i * 8)) & 0xFF).max().unwrap_or(0);
    (max != 0).then_some(max as f64 * BCLK_MHZ)
}

/// Plausible range for a CPU core voltage reading, in volts.
pub const PLAUSIBLE_VCORE_V: std::ops::RangeInclusive<f64> = 0.3..=2.0;

/// Core voltage from `IA32_PERF_STATUS` bits 47:32 (units of 2⁻¹³ V).
/// `None` when the field is zero (not reported) or implausible.
pub fn decode_core_voltage(raw: u64) -> Option<f64> {
    let v = ((raw >> 32) & 0xFFFF) as f64 / 8192.0;
    PLAUSIBLE_VCORE_V.contains(&v).then_some(v)
}

/// Busy frequency from summed APERF/MPERF deltas (turbostat "Bzy_MHz").
pub fn busy_mhz(reference_mhz: f64, delta_aperf: u64, delta_mperf: u64) -> Option<f64> {
    (delta_mperf > 0).then(|| reference_mhz * delta_aperf as f64 / delta_mperf as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units() -> RaplUnits {
        // Typical client value 0x000A0E03: power 1/8 W, energy 1/16384 J.
        decode_rapl_units(0x000A_0E03)
    }

    #[test]
    fn rapl_units() {
        let u = units();
        assert_eq!(u.power_w, 0.125);
        assert_eq!(u.energy_j, 1.0 / 16384.0);
    }

    #[test]
    fn energy_and_power() {
        let u = units();
        // 16384 units = 1 J over 0.5 s = 2 W.
        assert_eq!(average_power(1000, 1000 + 16384, u, 0.5), Some(2.0));
        // Wrap-around: 100 units before wrap + 16284 after = 16384 units.
        assert_eq!(energy_delta(u32::MAX - 99, 16_284), 16_384);
        assert_eq!(average_power(u32::MAX - 99, 16_284, u, 1.0), Some(1.0));
        assert_eq!(average_power(0, 1, u, 0.0), None);
    }

    #[test]
    fn power_limits() {
        // PL1 = 280 * 1/8 W = 35 W enabled; PL2 = 920 * 1/8 = 115 W enabled.
        let raw = (1u64 << 47) | (920u64 << 32) | (1 << 15) | 280;
        let l = decode_pkg_power_limit(raw, units());
        assert_eq!((l.pl1_w, l.pl1_enabled), (35.0, true));
        assert_eq!((l.pl2_w, l.pl2_enabled), (115.0, true));
        // Time-window and lock bits do not leak into the values.
        let raw = (1u64 << 63) | (0x7Fu64 << 49) | (0x7F << 17) | 280;
        let l = decode_pkg_power_limit(raw, units());
        assert_eq!(
            (l.pl1_w, l.pl1_enabled, l.pl2_w, l.pl2_enabled),
            (35.0, false, 0.0, false)
        );
    }

    #[test]
    fn tdp() {
        assert_eq!(decode_tdp(360, units()), Some(45.0));
        assert_eq!(decode_tdp(0, units()), None);
    }

    #[test]
    fn base_and_turbo() {
        assert_eq!(decode_base_mhz(0x0000_1800), Some(2400.0)); // ratio 24
        assert_eq!(decode_base_mhz(0), None);
        // Ratios 49,49,47,46,45,45,44,44 (fewest active cores first).
        assert_eq!(decode_max_turbo_mhz(0x2C2C_2D2D_2E2F_3131), Some(4900.0));
        assert_eq!(decode_max_turbo_mhz(0), None);
    }

    #[test]
    fn core_voltage() {
        // Raw values captured on the i7-13620H (Probe C).
        assert!((decode_core_voltage(0x0000_18b8_0000_1700).unwrap() - 0.7725).abs() < 1e-4);
        assert!((decode_core_voltage(0x0000_25e2_0000_2e00).unwrap() - 1.1838).abs() < 1e-4);
        assert!((decode_core_voltage(0x0000_1ca5_0000_1e00).unwrap() - 0.8951).abs() < 1e-4);
        // Not reported / implausible.
        assert_eq!(decode_core_voltage(0x0000_0000_0000_1700), None);
        assert_eq!(decode_core_voltage(0x0000_ffff_0000_0000), None); // ~8 V
        assert_eq!(decode_core_voltage(0x0000_0100_0000_0000), None); // ~0.03 V
    }

    #[test]
    fn busy_frequency() {
        assert_eq!(busy_mhz(2400.0, 3_000, 2_000), Some(3600.0));
        assert_eq!(busy_mhz(2400.0, 10, 0), None);
    }
}
