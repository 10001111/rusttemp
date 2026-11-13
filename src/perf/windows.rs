//! Windows speed, load and power.
//!
//! - **Load** (no admin): PDH `\Processor Information(*)\% Processor Time`.
//! - **Speed**:
//!   - with PawnIO + admin: TSC rate × ΔAPERF/ΔMPERF per core, with the TSC
//!     rate measured over the same window. On the i7-13620H test machine the
//!     MPERF tick rate of fully busy cores equalled the TSC rate (2918 MHz),
//!     not the 2900 MHz `MSR_PLATFORM_INFO` ratio nor the 2400 MHz base.
//!   - otherwise: PDH `% Processor Performance` × the *highest* rated MHz
//!     (P-core base). Measured on the test machine, Windows uses that same
//!     reference for E-cores too; this agreed with APERF/MPERF within ~1.5 %
//!     (see the `diag_reference_clocks` test).
//! - **Power, limits, base/max turbo** (PawnIO + admin): Intel RAPL and
//!   platform MSRs, read on the package's first logical CPU (max turbo on the
//!   first performance core, since hybrid CPUs report per core type).

use std::collections::HashMap;
use std::ptr;
use std::time::Instant;

use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows_sys::Win32::System::SystemInformation::RelationProcessorPackage;

use super::intel::{
    MSR_IA32_APERF, MSR_IA32_MPERF, MSR_IA32_PERF_STATUS, MSR_PKG_ENERGY_STATUS,
    MSR_PKG_POWER_INFO, MSR_PKG_POWER_LIMIT, MSR_PLATFORM_INFO, MSR_PP0_ENERGY_STATUS,
    MSR_PP1_ENERGY_STATUS, MSR_RAPL_POWER_UNIT, MSR_TURBO_RATIO_LIMIT, RaplUnits, average_power,
    busy_mhz, decode_base_mhz, decode_core_voltage, decode_max_turbo_mhz, decode_pkg_power_limit,
    decode_rapl_units, decode_tdp,
};
use super::{CoreActivity, MIN_INTERVAL_S, PerfSample, PerfSource, PowerLimits};
use crate::cpuinfo::windows::{CoreCpus, core_cpu_sets, processor_records, rated_mhz_per_cpu};
use crate::sensors::platform::intel_dts::{CoreTopology, core_labels};
use crate::sensors::platform::pawnio::{AffinityGuard, INTEL_MSR_MODULE, PawnIo, cpu_identity};

/// `PDH_FMT_NOSCALE` from pdh.h.
const PDH_FMT_NOSCALE: u32 = 0x0000_1000;

/// Minimal PDH query with several wildcard counters.
pub(crate) struct Pdh {
    query: PDH_HQUERY,
    counters: Vec<PDH_HCOUNTER>,
}

impl Pdh {
    pub(crate) fn open(paths: &[&str]) -> Result<Self, u32> {
        let mut query: PDH_HQUERY = ptr::null_mut();
        // SAFETY: valid out-pointer; null data source = real-time data.
        let status = unsafe { PdhOpenQueryW(ptr::null(), 0, &mut query) };
        if status != 0 {
            return Err(status);
        }
        let mut pdh = Pdh {
            query,
            counters: Vec::new(),
        };
        for path in paths {
            let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
            let mut c: PDH_HCOUNTER = ptr::null_mut();
            // SAFETY: open query, NUL-terminated path.
            let status = unsafe { PdhAddEnglishCounterW(pdh.query, wide.as_ptr(), 0, &mut c) };
            if status != 0 {
                return Err(status);
            }
            pdh.counters.push(c);
        }
        Ok(pdh)
    }

    pub(crate) fn collect(&self) -> Result<(), u32> {
        // SAFETY: open query.
        match unsafe { PdhCollectQueryData(self.query) } {
            0 => Ok(()),
            s => Err(s),
        }
    }

    /// `(instance, value)` pairs of counter `i` from the last two collections.
    pub(crate) fn values(&self, i: usize) -> Vec<(String, f64)> {
        let counter = self.counters[i];
        let format = PDH_FMT_DOUBLE | PDH_FMT_NOSCALE;
        let mut size = 0u32;
        let mut count = 0u32;
        // SAFETY: size query with a null buffer.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, ptr::null_mut())
        };
        if status != PDH_MORE_DATA {
            return Vec::new();
        }
        let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        // SAFETY: buffer of `size` bytes, 8-byte aligned.
        let status =
            unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, items) };
        if status != 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: PDH wrote `count` items; names are NUL-terminated in `buf`.
            let item = unsafe { &*items.add(i) };
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) {
                continue;
            }
            let name = unsafe { wide_to_string(item.szName) };
            // SAFETY: PDH_FMT_DOUBLE selects `doubleValue`.
            out.push((name, unsafe { item.FmtValue.Anonymous.doubleValue }));
        }
        out
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        // SAFETY: query closed exactly once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// # Safety
/// `p` must be null or a NUL-terminated UTF-16 string.
unsafe fn wide_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: caller guarantees NUL termination.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` elements were read above.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// Mean of per-CPU load percentages, `None` when there are none.
pub fn mean_load(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f64)
}

/// Parses a `Processor Information` instance name such as `0,12`.
pub fn parse_instance(name: &str) -> Option<(u16, u32)> {
    let (g, n) = name.split_once(',')?;
    Some((g.trim().parse().ok()?, n.trim().parse().ok()?))
}

/// One reading of the RAPL energy counters.
#[derive(Clone, Copy)]
struct Energy {
    pkg: u32,
    pp0: Option<u32>,
    pp1: Option<u32>,
    at: Instant,
}

/// Average power over one interval.
#[derive(Default)]
struct RaplPower {
    package: Option<f64>,
    cores: Option<f64>,
    /// Graphics plane (integrated GPU).
    graphics: Option<f64>,
    why: Option<String>,
}

/// PawnIO-based power, limits and APERF/MPERF speed.
struct IntelPerf {
    pawnio: PawnIo,
    units: Option<RaplUnits>,
    package_cpu: (u16, u32),
    limits: PowerLimits,
    base_mhz: Option<f64>,
    max_turbo_mhz: Option<f64>,
    /// Logical CPU (on a P-core) used for core-voltage reads.
    vcore_cpu: (u16, u32),
    prev_energy: Option<Energy>,
    prev_am: HashMap<(u16, u32), (u64, u64)>,
    prev_tsc: (u64, Instant),
    /// TSC rate measured over the last sample window.
    tsc_mhz: Option<f64>,
}

fn rdtsc() -> u64 {
    // SAFETY: RDTSC is available on every x86-64 CPU.
    #[allow(unused_unsafe)]
    unsafe {
        std::arch::x86_64::_rdtsc()
    }
}

impl IntelPerf {
    fn new(cores: &[CoreCpus]) -> Result<Self, String> {
        let (vendor, _) = cpu_identity();
        if vendor != "GenuineIntel" {
            return Err(if vendor == "AuthenticAMD" {
                "AMD power and clock readings on Windows are not implemented yet".into()
            } else {
                format!("CPU vendor {vendor:?} is not supported")
            });
        }
        let pawnio = PawnIo::open(INTEL_MSR_MODULE)?;
        let package_cpu = processor_records(RelationProcessorPackage)
            .ok()
            .and_then(|p| p.first().map(|c| (c.group, c.cpu)))
            .unwrap_or((0, 0));
        let read_on = |cpu: (u16, u32), msr: u32| -> Option<u64> {
            let _pin = AffinityGuard::pin(cpu.0, cpu.1).ok()?;
            pawnio.read_msr(msr).ok()
        };
        let units = read_on(package_cpu, MSR_RAPL_POWER_UNIT).map(decode_rapl_units);
        let mut limits = PowerLimits::default();
        if let Some(u) = units {
            if let Some(raw) = read_on(package_cpu, MSR_PKG_POWER_LIMIT) {
                let l = decode_pkg_power_limit(raw, u);
                limits.pl1_w = Some(l.pl1_w);
                limits.pl1_enabled = Some(l.pl1_enabled);
                limits.pl2_w = Some(l.pl2_w);
                limits.pl2_enabled = Some(l.pl2_enabled);
            }
            limits.tdp_w = read_on(package_cpu, MSR_PKG_POWER_INFO).and_then(|r| decode_tdp(r, u));
        }
        let base_mhz = read_on(package_cpu, MSR_PLATFORM_INFO).and_then(decode_base_mhz);
        // Max turbo is per core type on hybrid CPUs: read it on a P-core.
        let p_core = cores
            .iter()
            .max_by_key(|c| c.efficiency_class)
            .and_then(|c| c.cpus.first().copied())
            .unwrap_or(package_cpu);
        let max_turbo_mhz = read_on(p_core, MSR_TURBO_RATIO_LIMIT).and_then(decode_max_turbo_mhz);

        let mut s = IntelPerf {
            pawnio,
            units,
            package_cpu,
            limits,
            base_mhz,
            max_turbo_mhz,
            vcore_cpu: p_core,
            prev_energy: None,
            prev_am: HashMap::new(),
            prev_tsc: (rdtsc(), Instant::now()),
            tsc_mhz: None,
        };
        s.prev_energy = s.read_energy();
        for c in cores {
            for &cpu in &c.cpus {
                if let Some(am) = s.read_aperf_mperf(cpu) {
                    s.prev_am.insert(cpu, am);
                }
            }
        }
        Ok(s)
    }

    fn read_energy(&self) -> Option<Energy> {
        self.units?;
        let _pin = AffinityGuard::pin(self.package_cpu.0, self.package_cpu.1).ok()?;
        let pkg = self.pawnio.read_msr(MSR_PKG_ENERGY_STATUS).ok()? as u32;
        let opt = |msr| self.pawnio.read_msr(msr).ok().map(|v| v as u32);
        Some(Energy {
            pkg,
            pp0: opt(MSR_PP0_ENERGY_STATUS),
            pp1: opt(MSR_PP1_ENERGY_STATUS),
            at: Instant::now(),
        })
    }

    /// Current core voltage from `IA32_PERF_STATUS` on a P-core.
    fn core_voltage(&self) -> Result<f64, String> {
        let _pin = AffinityGuard::pin(self.vcore_cpu.0, self.vcore_cpu.1)?;
        let raw = self.pawnio.read_msr(MSR_IA32_PERF_STATUS)?;
        decode_core_voltage(raw).ok_or_else(|| {
            format!("IA32_PERF_STATUS reported no plausible voltage (raw 0x{raw:016X})")
        })
    }

    fn read_aperf_mperf(&self, cpu: (u16, u32)) -> Option<(u64, u64)> {
        let _pin = AffinityGuard::pin(cpu.0, cpu.1).ok()?;
        let a = self.pawnio.read_msr(MSR_IA32_APERF).ok()?;
        let m = self.pawnio.read_msr(MSR_IA32_MPERF).ok()?;
        Some((a, m))
    }

    /// Package, cores and graphics-plane power since the previous call.
    fn power(&mut self) -> RaplPower {
        let fail = |why: &str| RaplPower {
            why: Some(why.to_string()),
            ..Default::default()
        };
        let Some(units) = self.units else {
            return fail("RAPL power units could not be read");
        };
        let cur = self.read_energy();
        match (self.prev_energy, cur) {
            (Some(p), Some(c)) => {
                let dt = c.at.duration_since(p.at).as_secs_f64();
                if dt < MIN_INTERVAL_S {
                    return fail("measuring…");
                }
                self.prev_energy = cur;
                let pair = |a: Option<u32>, b: Option<u32>| {
                    a.zip(b).and_then(|(a, b)| average_power(a, b, units, dt))
                };
                RaplPower {
                    package: average_power(p.pkg, c.pkg, units, dt),
                    cores: pair(p.pp0, c.pp0),
                    graphics: pair(p.pp1, c.pp1),
                    why: None,
                }
            }
            (None, Some(_)) => {
                self.prev_energy = cur;
                fail("measuring…")
            }
            (_, None) => fail("energy counter could not be read"),
        }
    }

    /// Measures the TSC rate since the previous call (the reference clock
    /// MPERF counts at).
    fn update_tsc_rate(&mut self) {
        let now = (rdtsc(), Instant::now());
        let dt = now.1.duration_since(self.prev_tsc.1).as_secs_f64();
        if dt >= MIN_INTERVAL_S {
            self.tsc_mhz = Some(now.0.wrapping_sub(self.prev_tsc.0) as f64 / dt / 1e6);
            self.prev_tsc = now;
        }
    }

    /// Busy MHz of one core from summed APERF/MPERF deltas of its CPUs.
    fn core_mhz(&mut self, core: &CoreCpus) -> Option<f64> {
        let reference = self.tsc_mhz.or(self.base_mhz)?;
        let (mut da, mut dm) = (0u64, 0u64);
        for &cpu in &core.cpus {
            let Some(cur) = self.read_aperf_mperf(cpu) else {
                continue;
            };
            if let Some(prev) = self.prev_am.insert(cpu, cur) {
                // Counters only move forward; skip a thread if they were reset.
                if let (Some(a), Some(m)) = (cur.0.checked_sub(prev.0), cur.1.checked_sub(prev.1)) {
                    da += a;
                    dm += m;
                }
            }
        }
        busy_mhz(reference, da, dm)
    }
}

/// Windows speed/load/power source.
pub struct WindowsPerf {
    cores: Vec<CoreCpus>,
    labels: Vec<String>,
    /// Highest rated MHz (P-core base): reference for `% Processor Performance`.
    rated_max: Option<f64>,
    pdh: Option<Pdh>,
    intel: Option<IntelPerf>,
    intel_error: Option<String>,
    pdh_error: Option<String>,
}

impl WindowsPerf {
    /// Opens the counters and takes baseline readings.
    pub fn new() -> Self {
        let cores = core_cpu_sets().unwrap_or_default();
        let topo: Vec<CoreTopology> = cores
            .iter()
            .map(|c| CoreTopology {
                group: c.cpus[0].0,
                cpu: c.cpus[0].1,
                efficiency_class: c.efficiency_class,
            })
            .collect();
        let labels = core_labels(&topo);
        let logical: usize = cores.iter().map(|c| c.cpus.len()).sum();
        let rated_max = rated_mhz_per_cpu(logical)
            .and_then(|r| r.into_iter().max())
            .filter(|&m| m > 0)
            .map(f64::from);
        let (pdh, pdh_error) = match Pdh::open(&[
            r"\Processor Information(*)\% Processor Time",
            r"\Processor Information(*)\% Processor Performance",
        ]) {
            Ok(p) => {
                let _ = p.collect();
                (Some(p), None)
            }
            Err(s) => (
                None,
                Some(format!(
                    "processor performance counters unavailable (PDH 0x{s:08X})"
                )),
            ),
        };
        let (intel, intel_error) = match IntelPerf::new(&cores) {
            Ok(i) => (Some(i), None),
            Err(e) => (None, Some(e)),
        };
        WindowsPerf {
            cores,
            labels,
            rated_max,
            pdh,
            intel,
            intel_error,
            pdh_error,
        }
    }
}

impl PerfSource for WindowsPerf {
    fn sample(&mut self) -> PerfSample {
        let mut s = PerfSample::default();

        // PDH load and performance percentage.
        let mut load: HashMap<(u16, u32), f64> = HashMap::new();
        let mut perf: HashMap<(u16, u32), f64> = HashMap::new();
        if let Some(pdh) = &self.pdh {
            if pdh.collect().is_ok() {
                for (name, v) in pdh.values(0) {
                    if let Some(cpu) = parse_instance(&name) {
                        load.insert(cpu, v.clamp(0.0, 100.0));
                    }
                }
                // Whole-CPU load = mean of the logical CPUs. The `_Total`
                // instance is not used: over short windows it is derived from
                // idle time and was observed reporting 0 % (a clamped
                // negative value) while individual CPUs were 20-38 % busy.
                s.total_load_pct = mean_load(load.values().copied());
                s.load_source = Some("Windows performance counters (% Processor Time)");
                for (name, v) in pdh.values(1) {
                    if let Some(cpu) = parse_instance(&name) {
                        perf.insert(cpu, v);
                    }
                }
            }
        }

        // PawnIO power and APERF/MPERF speed.
        let mut msr_mhz: Vec<Option<f64>> = vec![None; self.cores.len()];
        if let Some(intel) = &mut self.intel {
            let RaplPower {
                package: pkg,
                cores: cores_w,
                graphics,
                why,
            } = intel.power();
            s.graphics_w = graphics;
            if graphics.is_none() {
                s.graphics_unavailable =
                    Some(why.clone().unwrap_or_else(|| {
                        "this CPU does not report graphics-plane energy".into()
                    }));
            }
            s.package_w = pkg;
            s.cores_w = cores_w;
            s.power_unavailable = why;
            if pkg.is_some() {
                s.power_source = Some("Intel RAPL via PawnIO");
            }
            s.limits = intel.limits;
            s.limits_source = intel
                .limits
                .any()
                .then_some("CPU power-limit registers via PawnIO");
            s.max_turbo_mhz = intel.max_turbo_mhz;
            match intel.core_voltage() {
                Ok(v) => {
                    s.voltage_v = Some(v);
                    s.voltage_source = Some("IA32_PERF_STATUS via PawnIO");
                }
                Err(e) => s.voltage_unavailable = Some(e),
            }
            intel.update_tsc_rate();
            s.reference_mhz = intel.tsc_mhz;
            for (i, core) in self.cores.iter().enumerate() {
                msr_mhz[i] = intel.core_mhz(core);
            }
        } else {
            s.power_unavailable = self.intel_error.clone();
            s.graphics_unavailable = self.intel_error.clone();
            s.voltage_unavailable = self.intel_error.clone();
        }
        s.base_mhz = self.rated_max;

        let use_msr = msr_mhz.iter().any(Option::is_some);
        for (i, core) in self.cores.iter().enumerate() {
            let mhz = if use_msr {
                msr_mhz[i]
            } else {
                core.cpus
                    .iter()
                    // 0 % = the CPU was idle all window: no clock measured.
                    .filter_map(|c| {
                        Some(self.rated_max? * perf.get(c).filter(|&&p| p > 0.0)? / 100.0)
                    })
                    .reduce(f64::max)
            };
            let load_pct = core
                .cpus
                .iter()
                .filter_map(|c| load.get(c).copied())
                .reduce(f64::max);
            s.cores.push(CoreActivity {
                label: self.labels[i].clone(),
                mhz,
                load_pct,
            });
        }
        s.compute_average();
        if use_msr {
            s.speed_source = Some("APERF/MPERF via PawnIO");
        } else if s.avg_mhz.is_some() {
            s.speed_source = Some("Windows % Processor Performance");
        } else {
            s.speed_unavailable = Some(
                self.pdh_error
                    .clone()
                    .unwrap_or_else(|| "measuring…".into()),
            );
        }
        s
    }

    fn notes(&self) -> Vec<String> {
        match &self.intel_error {
            Some(e) => vec![format!("CPU power and limits unavailable: {e}")],
            None => Vec::new(),
        }
    }
}

impl Default for WindowsPerf {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names() {
        assert_eq!(parse_instance("0,12"), Some((0, 12)));
        assert_eq!(parse_instance("1,3"), Some((1, 3)));
        assert_eq!(parse_instance("0,_Total"), None);
        assert_eq!(parse_instance("_Total"), None);
    }

    #[test]
    fn total_load_is_mean_of_cpus() {
        assert_eq!(mean_load([0.0, 25.0, 50.0, 25.0].into_iter()), Some(25.0));
        assert_eq!(mean_load(std::iter::empty()), None);
    }

    /// Two samples on the real machine: load is always available (no admin
    /// needed); speed comes from one of the two sources; power is either a
    /// number or an explanation.
    #[test]
    fn live_two_samples() {
        let mut p = WindowsPerf::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let s = p.sample();
        assert!(!s.cores.is_empty());
        assert!(s.total_load_pct.is_some());
        assert!(s.cores.iter().all(|c| c.load_pct.is_some()));
        assert!(s.speed_source.is_some() || s.speed_unavailable.is_some());
        assert!(s.package_w.is_some() || s.power_unavailable.is_some());
        for c in &s.cores {
            if let Some(m) = c.mhz {
                assert!((100.0..10_000.0).contains(&m), "{m}");
            }
        }
    }

    /// Diagnostic (run elevated, ideally under load):
    /// `cargo test --release diag_reference_clocks -- --ignored --nocapture`.
    /// Measures, over identical 1 s windows, the APERF/MPERF ratio, the
    /// Windows `% Processor Performance` value, the MPERF tick rate and the
    /// TSC rate, to establish which reference clock each one uses.
    #[test]
    #[ignore = "diagnostic: requires administrator rights and PawnIO"]
    fn diag_reference_clocks() {
        #[allow(unused_unsafe)]
        let rdtsc = || unsafe { std::arch::x86_64::_rdtsc() };
        let cores = core_cpu_sets().unwrap();
        let intel = IntelPerf::new(&cores).expect("needs PawnIO + admin");
        println!("platform-info nominal = {:?} MHz", intel.base_mhz);
        let pdh = Pdh::open(&[r"\Processor Information(*)\% Processor Performance"]).unwrap();
        pdh.collect().unwrap();
        let read_all = || -> Vec<(u64, u64)> {
            cores
                .iter()
                .map(|c| intel.read_aperf_mperf(c.cpus[0]).unwrap())
                .collect()
        };
        let (mut am0, mut tsc0, mut t0) = (read_all(), rdtsc(), Instant::now());
        for round in 1..=6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            pdh.collect().unwrap();
            let am1 = read_all();
            let (tsc1, t1) = (rdtsc(), Instant::now());
            let perf: HashMap<(u16, u32), f64> = pdh
                .values(0)
                .into_iter()
                .filter_map(|(n, v)| Some((parse_instance(&n)?, v)))
                .collect();
            let dt = t1.duration_since(t0).as_secs_f64();
            println!(
                "round {round}: TSC rate {:.1} MHz",
                (tsc1 - tsc0) as f64 / dt / 1e6
            );
            for (i, c) in cores.iter().enumerate() {
                let da = am1[i].0.wrapping_sub(am0[i].0) as f64;
                let dm = am1[i].1.wrapping_sub(am0[i].1) as f64;
                let pp = perf.get(&c.cpus[0]).copied().unwrap_or(f64::NAN);
                let ratio = da / dm;
                println!(
                    "  core {i} class {}: A/M {:.3}  MPERF rate {:.0} MHz  PDH perf {:.1}%  PDH/(100*A/M) {:.3}",
                    c.efficiency_class,
                    ratio,
                    dm / dt / 1e6,
                    pp,
                    pp / (100.0 * ratio)
                );
            }
            (am0, tsc0, t0) = (am1, tsc1, t1);
        }
    }
}
