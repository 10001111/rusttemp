//! Linux speed, load and power from `/proc` and sysfs (root-relative paths).
//!
//! - Load: `/proc/stat` lines `cpu` and `cpuN` (user nice system idle iowait
//!   irq softirq steal ...); busy = user+nice+system+irq+softirq+steal,
//!   total = busy+idle+iowait.
//! - Speed: `/sys/devices/system/cpu/cpuN/cpufreq/scaling_cur_freq` (kHz).
//! - Topology: `cpuN/topology/{physical_package_id,core_id}`; hybrid split
//!   from `/sys/devices/cpu_core/cpus` and `/sys/devices/cpu_atom/cpus`.
//! - Power: `/sys/class/powercap/intel-rapl:N` zones named `package-*`
//!   (`energy_uj`, `max_energy_range_uj`), sub-zone `core`, limits
//!   `constraint_K_name` (`long_term` = PL1, `short_term` = PL2) and
//!   `constraint_K_power_limit_uw`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{CoreActivity, MIN_INTERVAL_S, PerfSample, PerfSource, PowerLimits};
use crate::cpuinfo::linux::parse_cpu_list;
use crate::sensors::platform::intel_dts::{CoreTopology, core_labels};

/// Busy and total jiffies of one `/proc/stat` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

/// Parses `/proc/stat` into the aggregate line and per-CPU lines.
pub fn parse_proc_stat(text: &str) -> (Option<CpuTimes>, HashMap<u32, CpuTimes>) {
    let mut total = None;
    let mut per = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let Some(name) = it.next() else { continue };
        let Some(rest) = name.strip_prefix("cpu") else {
            continue;
        };
        let v: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
        if v.len() < 4 {
            continue;
        }
        let get = |i: usize| v.get(i).copied().unwrap_or(0);
        let busy = get(0) + get(1) + get(2) + get(5) + get(6) + get(7);
        let times = CpuTimes {
            busy,
            total: busy + get(3) + get(4),
        };
        if rest.is_empty() {
            total = Some(times);
        } else if let Ok(n) = rest.parse() {
            per.insert(n, times);
        }
    }
    (total, per)
}

/// Load percent between two readings.
pub fn load_pct(prev: CpuTimes, cur: CpuTimes) -> Option<f64> {
    let dt = cur.total.checked_sub(prev.total)?;
    let db = cur.busy.checked_sub(prev.busy)?;
    (dt > 0).then(|| (db as f64 * 100.0 / dt as f64).clamp(0.0, 100.0))
}

fn read(path: &Path) -> io::Result<String> {
    fs::read_to_string(path).map(|s| s.trim().to_string())
}

fn read_u64(path: &Path) -> Option<u64> {
    read(path).ok()?.parse().ok()
}

struct Core {
    label: String,
    cpus: Vec<u32>,
}

struct RaplZone {
    energy: PathBuf,
    max_range: Option<u64>,
    core_energy: Option<PathBuf>,
}

/// Linux speed/load/power source.
pub struct LinuxPerf {
    cpu_dir: PathBuf,
    proc_stat: PathBuf,
    cores: Vec<Core>,
    zones: Vec<RaplZone>,
    limits: PowerLimits,
    max_turbo_mhz: Option<f64>,
    base_mhz: Option<f64>,
    prev_stat: Option<(Option<CpuTimes>, HashMap<u32, CpuTimes>)>,
    prev_energy: Option<(Vec<u64>, Option<u64>, Instant)>,
    power_note: Option<String>,
}

impl LinuxPerf {
    /// Discovers cores and RAPL zones below `root` and takes a baseline reading.
    pub fn new(root: &Path) -> Self {
        let cpu_dir = root.join("sys/devices/system/cpu");
        let cores = discover_cores(root, &cpu_dir);
        let (zones, limits) = discover_rapl(&root.join("sys/class/powercap"));
        let khz_max = |file: &str| -> Option<f64> {
            fs::read_dir(&cpu_dir)
                .ok()?
                .flatten()
                .filter_map(|e| read_u64(&e.path().join("cpufreq").join(file)))
                .max()
                .map(|k| k as f64 / 1000.0)
        };
        let mut s = LinuxPerf {
            proc_stat: root.join("proc/stat"),
            max_turbo_mhz: khz_max("cpuinfo_max_freq"),
            base_mhz: khz_max("base_frequency"),
            cpu_dir,
            cores,
            zones,
            limits,
            prev_stat: None,
            prev_energy: None,
            power_note: None,
        };
        s.prev_stat = s.read_stat();
        s.prev_energy = s.read_energy();
        s
    }

    fn read_stat(&self) -> Option<(Option<CpuTimes>, HashMap<u32, CpuTimes>)> {
        read(&self.proc_stat).ok().map(|t| parse_proc_stat(&t))
    }

    /// Energy (µJ) of each package zone and the summed core sub-zones.
    fn read_energy(&mut self) -> Option<(Vec<u64>, Option<u64>, Instant)> {
        if self.zones.is_empty() {
            self.power_note = Some("no RAPL powercap zones (intel-rapl) found".into());
            return None;
        }
        let mut pkg = Vec::new();
        let mut core_sum: Option<u64> = Some(0);
        for z in &self.zones {
            match read(&z.energy) {
                Ok(v) => pkg.push(v.parse().ok()?),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    self.power_note = Some(
                        "RAPL energy counters are readable only by root (run with sudo)".into(),
                    );
                    return None;
                }
                Err(e) => {
                    self.power_note = Some(format!("cannot read {}: {e}", z.energy.display()));
                    return None;
                }
            }
            core_sum = match (&z.core_energy, core_sum) {
                (Some(p), Some(acc)) => read_u64(p).map(|v| acc + v),
                _ => None,
            };
        }
        Some((pkg, core_sum, Instant::now()))
    }
}

fn discover_cores(root: &Path, cpu_dir: &Path) -> Vec<Core> {
    let mut by_core: BTreeMap<(u64, u64), Vec<u32>> = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(cpu_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(n) = name.strip_prefix("cpu").and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let topo = e.path().join("topology");
            let (Some(pkg), Some(core)) = (
                read_u64(&topo.join("physical_package_id")),
                read_u64(&topo.join("core_id")),
            ) else {
                continue;
            };
            by_core.entry((pkg, core)).or_default().push(n);
        }
    }
    let p_cpus = read(&root.join("sys/devices/cpu_core/cpus"))
        .ok()
        .map(|s| parse_cpu_list(&s));
    let e_cpus = read(&root.join("sys/devices/cpu_atom/cpus"))
        .ok()
        .map(|s| parse_cpu_list(&s));
    let hybrid = p_cpus.is_some() && e_cpus.is_some();
    let mut cores: Vec<(Vec<u32>, u8)> = by_core
        .into_values()
        .map(|mut cpus| {
            cpus.sort_unstable();
            let class = match &p_cpus {
                Some(p) if hybrid && p.contains(&cpus[0]) => 1,
                _ => 0,
            };
            (cpus, class)
        })
        .collect();
    // Order by first logical CPU, matching how the OS enumerates cores.
    cores.sort_by_key(|(cpus, _)| cpus[0]);
    let topo: Vec<CoreTopology> = cores
        .iter()
        .map(|(cpus, class)| CoreTopology {
            group: 0,
            cpu: cpus[0],
            efficiency_class: *class,
        })
        .collect();
    cores
        .into_iter()
        .zip(core_labels(&topo))
        .map(|((cpus, _), label)| Core { label, cpus })
        .collect()
}

fn discover_rapl(dir: &Path) -> (Vec<RaplZone>, PowerLimits) {
    let mut zones = Vec::new();
    let mut limits = PowerLimits::default();
    let Ok(entries) = fs::read_dir(dir) else {
        return (zones, limits);
    };
    let mut names: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("intel-rapl:") && n.matches(':').count() == 1)
        })
        .collect();
    names.sort();
    for z in names {
        if !read(&z.join("name")).is_ok_and(|n| n.starts_with("package")) {
            continue;
        }
        // Sub-zone named "core" (e.g. intel-rapl:0:0).
        let core_energy = fs::read_dir(&z).ok().and_then(|it| {
            it.flatten()
                .map(|e| e.path())
                .find(|p| p.is_dir() && read(&p.join("name")).is_ok_and(|n| n == "core"))
                .map(|p| p.join("energy_uj"))
        });
        // Limits from the first package zone.
        if zones.is_empty() {
            for k in 0..4 {
                let Ok(name) = read(&z.join(format!("constraint_{k}_name"))) else {
                    continue;
                };
                let w = read_u64(&z.join(format!("constraint_{k}_power_limit_uw")))
                    .map(|uw| uw as f64 / 1e6);
                match name.as_str() {
                    "long_term" => limits.pl1_w = w,
                    "short_term" => limits.pl2_w = w,
                    _ => {}
                }
            }
            if let Some(en) = read_u64(&z.join("enabled")) {
                limits.pl1_enabled = limits.pl1_w.map(|_| en == 1);
                limits.pl2_enabled = limits.pl2_w.map(|_| en == 1);
            }
        }
        zones.push(RaplZone {
            energy: z.join("energy_uj"),
            max_range: read_u64(&z.join("max_energy_range_uj")),
            core_energy,
        });
    }
    (zones, limits)
}

/// Energy delta in µJ with wrap at `max_range` (if known).
fn energy_delta_uj(prev: u64, cur: u64, max_range: Option<u64>) -> Option<u64> {
    if cur >= prev {
        Some(cur - prev)
    } else {
        max_range.map(|m| m - prev + cur)
    }
}

impl PerfSource for LinuxPerf {
    fn sample(&mut self) -> PerfSample {
        let mut s = PerfSample {
            limits: self.limits,
            base_mhz: self.base_mhz,
            max_turbo_mhz: self.max_turbo_mhz,
            limits_source: self.limits.any().then_some("powercap constraints"),
            voltage_unavailable: Some(
                "not exposed through sysfs; it needs processor register (MSR) access, \
                 which RustTemp does not implement on Linux"
                    .into(),
            ),
            ..Default::default()
        };

        // Load.
        let cur_stat = self.read_stat();
        let loads: HashMap<u32, f64> = match (&self.prev_stat, &cur_stat) {
            (Some((pt, pp)), Some((ct, cp))) => {
                s.total_load_pct = pt.zip(*ct).and_then(|(a, b)| load_pct(a, b));
                s.load_source = Some("/proc/stat");
                cp.iter()
                    .filter_map(|(n, c)| pp.get(n).and_then(|p| load_pct(*p, *c)).map(|l| (*n, l)))
                    .collect()
            }
            _ => HashMap::new(),
        };
        if cur_stat.is_some() {
            self.prev_stat = cur_stat;
        }

        // Speed.
        let mut any_speed = false;
        for core in &self.cores {
            let mhz = core
                .cpus
                .iter()
                .filter_map(|c| {
                    read_u64(
                        &self
                            .cpu_dir
                            .join(format!("cpu{c}/cpufreq/scaling_cur_freq")),
                    )
                })
                .max()
                .map(|k| k as f64 / 1000.0);
            any_speed |= mhz.is_some();
            let load = core
                .cpus
                .iter()
                .filter_map(|c| loads.get(c).copied())
                .reduce(f64::max);
            s.cores.push(CoreActivity {
                label: core.label.clone(),
                mhz,
                load_pct: load,
            });
        }
        if any_speed {
            s.speed_source = Some("cpufreq scaling_cur_freq");
        } else {
            s.speed_unavailable = Some("cpufreq does not report current frequencies".into());
        }
        s.compute_average();

        // Power.
        match (self.prev_energy.take(), self.read_energy()) {
            (Some((pp, pc, pt)), Some((cp, cc, ct))) => {
                let dt = ct.duration_since(pt).as_secs_f64();
                if dt >= MIN_INTERVAL_S {
                    let pkg: Option<u64> = pp
                        .iter()
                        .zip(&cp)
                        .zip(&self.zones)
                        .map(|((a, b), z)| energy_delta_uj(*a, *b, z.max_range))
                        .sum();
                    s.package_w = pkg.map(|uj| uj as f64 / 1e6 / dt);
                    s.cores_w = pc
                        .zip(cc)
                        .and_then(|(a, b)| energy_delta_uj(a, b, None))
                        .map(|uj| uj as f64 / 1e6 / dt);
                    s.power_source = Some("powercap intel-rapl");
                    self.prev_energy = Some((cp, cc, ct));
                } else {
                    s.power_unavailable = Some("measuring…".into());
                    self.prev_energy = Some((pp, pc, pt));
                }
            }
            (None, Some(cur)) => {
                s.power_unavailable = Some("measuring…".into());
                self.prev_energy = Some(cur);
            }
            (_, None) => {
                s.power_unavailable = self.power_note.clone();
            }
        }
        s
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
    fn proc_stat_parsing_and_load() {
        let a = "cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 50 0 50 400 0 0 0 0\ncpu1 50 0 50 400 0 0 0 0\nintr 1 2\n";
        let b = "cpu  300 0 100 1000 0 0 0 0 0 0\ncpu0 250 0 50 400 0 0 0 0\ncpu1 50 0 50 600 0 0 0 0\n";
        let (ta, pa) = parse_proc_stat(a);
        let (tb, pb) = parse_proc_stat(b);
        assert_eq!(
            ta,
            Some(CpuTimes {
                busy: 200,
                total: 1000
            })
        );
        assert_eq!(load_pct(ta.unwrap(), tb.unwrap()), Some(50.0));
        assert_eq!(load_pct(pa[&0], pb[&0]), Some(100.0));
        assert_eq!(load_pct(pa[&1], pb[&1]), Some(0.0));
        assert_eq!(load_pct(pb[&0], pa[&0]), None); // counter went backwards
    }

    #[test]
    fn energy_wrap() {
        assert_eq!(energy_delta_uj(10, 30, None), Some(20));
        assert_eq!(energy_delta_uj(90, 10, Some(100)), Some(20));
        assert_eq!(energy_delta_uj(90, 10, None), None);
    }

    /// 2 P-cores with HT (cpu0-3) + 2 E-cores (cpu4,5), RAPL readable.
    fn tree() -> TempDir {
        let t = TempDir::new().unwrap();
        let r = t.path();
        for (cpu, core, khz) in [
            (0, 0, 4_000_000),
            (1, 0, 3_900_000),
            (2, 4, 2_000_000),
            (3, 4, 2_100_000),
            (4, 8, 3_000_000),
            (5, 9, 1_000_000),
        ] {
            let b = format!("sys/devices/system/cpu/cpu{cpu}");
            write(r, &format!("{b}/topology/physical_package_id"), "0");
            write(r, &format!("{b}/topology/core_id"), &core.to_string());
            write(
                r,
                &format!("{b}/cpufreq/scaling_cur_freq"),
                &khz.to_string(),
            );
            write(r, &format!("{b}/cpufreq/cpuinfo_max_freq"), "4900000");
            write(r, &format!("{b}/cpufreq/base_frequency"), "2400000");
        }
        write(r, "sys/devices/cpu_core/cpus", "0-3");
        write(r, "sys/devices/cpu_atom/cpus", "4-5");
        write(
            r,
            "proc/stat",
            "cpu  0 0 0 0 0 0 0 0\ncpu0 0 0 0 0\ncpu1 0 0 0 0\ncpu2 0 0 0 0\ncpu3 0 0 0 0\ncpu4 0 0 0 0\ncpu5 0 0 0 0\n",
        );
        t
    }

    #[test]
    fn hybrid_speed_and_load() {
        let t = tree();
        let r = t.path();
        let mut p = LinuxPerf::new(r);
        let labels: Vec<&str> = p.cores.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Core 0 (P-core)",
                "Core 1 (P-core)",
                "Core 2 (E-core)",
                "Core 3 (E-core)"
            ]
        );

        // Advance counters: cpu0 fully busy, others idle.
        write(
            r,
            "proc/stat",
            "cpu  100 0 0 500 0 0 0 0\ncpu0 100 0 0 0\ncpu1 0 0 0 100\ncpu2 0 0 0 100\n\
             cpu3 0 0 0 100\ncpu4 0 0 0 100\ncpu5 0 0 0 100\n",
        );
        let s = p.sample();
        assert_eq!(s.cores[0].mhz, Some(4000.0)); // max of cpu0/cpu1
        assert_eq!(s.cores[0].load_pct, Some(100.0));
        assert_eq!(s.cores[1].mhz, Some(2100.0));
        assert_eq!(s.cores[2].load_pct, Some(0.0));
        assert_eq!(s.total_load_pct, Some(100.0 / 6.0));
        assert_eq!(s.avg_mhz, Some((4000.0 + 2100.0 + 3000.0 + 1000.0) / 4.0));
        assert_eq!((s.base_mhz, s.max_turbo_mhz), (Some(2400.0), Some(4900.0)));
        assert_eq!(s.speed_source, Some("cpufreq scaling_cur_freq"));
        assert_eq!(s.package_w, None);
    }

    /// Powercap zone names contain ':' which Windows file names cannot, so
    /// this fixture only exists on Unix.
    #[cfg(unix)]
    #[test]
    fn rapl_power_and_limits() {
        let t = tree();
        let r = t.path();
        let z = "sys/class/powercap/intel-rapl:0";
        write(r, &format!("{z}/name"), "package-0");
        write(r, &format!("{z}/energy_uj"), "1000000");
        write(r, &format!("{z}/max_energy_range_uj"), "262143328850");
        write(r, &format!("{z}/enabled"), "1");
        write(r, &format!("{z}/constraint_0_name"), "long_term");
        write(r, &format!("{z}/constraint_0_power_limit_uw"), "35000000");
        write(r, &format!("{z}/constraint_1_name"), "short_term");
        write(r, &format!("{z}/constraint_1_power_limit_uw"), "115000000");
        write(r, &format!("{z}/intel-rapl:0:0/name"), "core");
        write(r, &format!("{z}/intel-rapl:0:0/energy_uj"), "500000");
        // A non-package zone (psys) must be ignored.
        write(r, "sys/class/powercap/intel-rapl:1/name", "psys");
        write(r, "sys/class/powercap/intel-rapl:1/energy_uj", "0");
        let mut p = LinuxPerf::new(r);
        assert_eq!(p.limits.pl1_w, Some(35.0));
        assert_eq!(p.limits.pl2_w, Some(115.0));
        assert_eq!(p.limits.pl1_enabled, Some(true));
        // +2 J package, +1 J cores.
        write(r, "sys/class/powercap/intel-rapl:0/energy_uj", "3000000");
        write(
            r,
            "sys/class/powercap/intel-rapl:0/intel-rapl:0:0/energy_uj",
            "1500000",
        );
        std::thread::sleep(std::time::Duration::from_millis(120));
        let s = p.sample();
        let w = s.package_w.unwrap();
        let cw = s.cores_w.unwrap();
        assert!(w > 1.0 && w <= 2.0 / 0.12, "{w}");
        assert!((w / cw - 2.0).abs() < 1e-9);
        assert_eq!(s.power_source, Some("powercap intel-rapl"));
        assert_eq!(s.limits.pl2_w, Some(115.0));
    }

    #[test]
    fn missing_everything() {
        let t = TempDir::new().unwrap();
        let mut p = LinuxPerf::new(t.path());
        let s = p.sample();
        assert!(s.cores.is_empty());
        assert_eq!(s.package_w, None);
        assert!(s.power_unavailable.as_deref().unwrap().contains("no RAPL"));
        assert!(!s.has_data());
    }
}
