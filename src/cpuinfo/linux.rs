//! Linux CPU identification from `/proc/cpuinfo` and `/sys/devices/system/cpu`.
//!
//! All paths are relative to a root directory so the parser can be tested
//! with mock trees on any OS and used with `--sysfs-root`.
//!
//! - `/proc/cpuinfo`: `vendor_id`, `model name`, `cpu family`, `model`,
//!   `stepping`, `microcode`, `physical id`, `core id`, one block per CPU.
//! - `/sys/devices/system/cpu/cpuN/cache/indexK/{level,type,size,shared_cpu_list}`.
//! - `/sys/devices/system/cpu/cpuN/cpufreq/{base_frequency,cpuinfo_max_freq}` (kHz).
//! - `/sys/devices/cpu_core/cpus`, `/sys/devices/cpu_atom/cpus`: P/E logical
//!   CPU lists on Intel hybrid CPUs.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use super::{CpuInfo, group_caches, tidy_brand, vendor_name};

/// Parses a CPU list such as `0-3,8,10-11`.
pub fn parse_cpu_list(s: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for part in s.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                    out.extend(a..=b);
                }
            }
            None => {
                if let Ok(n) = part.trim().parse() {
                    out.insert(n);
                }
            }
        }
    }
    out
}

/// Parses a sysfs cache size such as `48K` or `24M` into KiB.
pub fn parse_cache_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        'K' => (&s[..s.len() - 1], 1),
        'M' => (&s[..s.len() - 1], 1024),
        'G' => (&s[..s.len() - 1], 1024 * 1024),
        _ => (s, 1),
    };
    num.parse::<u64>().ok().map(|n| n * mul)
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

#[derive(Default)]
struct Proc {
    processor: Option<u32>,
    physical_id: Option<u32>,
    core_id: Option<u32>,
}

/// Builds a [`CpuInfo`] from the files below `root`.
pub fn from_root(root: &Path) -> CpuInfo {
    let mut info = CpuInfo::default();
    let cpu_dir = root.join("sys/devices/system/cpu");

    // /proc/cpuinfo
    let mut procs: Vec<Proc> = Vec::new();
    if let Some(text) = read(&root.join("proc/cpuinfo")) {
        info.sources.push("/proc/cpuinfo");
        let mut cur = Proc::default();
        let mut started = false;
        for line in text.lines().chain(std::iter::once("")) {
            let Some((k, v)) = line.split_once(':') else {
                if line.trim().is_empty() && started {
                    procs.push(std::mem::take(&mut cur));
                    started = false;
                }
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "processor" => {
                    cur.processor = v.parse().ok();
                    started = true;
                }
                "physical id" => cur.physical_id = v.parse().ok(),
                "core id" => cur.core_id = v.parse().ok(),
                "vendor_id" if info.vendor_id.is_none() => {
                    info.vendor = Some(vendor_name(v));
                    info.vendor_id = Some(v.to_string());
                }
                "model name" if info.brand.is_none() => info.brand = Some(tidy_brand(v)),
                "cpu family" if info.family.is_none() => info.family = v.parse().ok(),
                "model" if info.model.is_none() => info.model = v.parse().ok(),
                "stepping" if info.stepping.is_none() => info.stepping = v.parse().ok(),
                // Hypervisors that hide the microcode revision report 0xffffffff.
                "microcode" if info.microcode.is_none() && v != "0xffffffff" => {
                    info.microcode = Some(v.to_string())
                }
                "flags" if v.split_whitespace().any(|f| f == "hypervisor") => {
                    info.virtual_machine = true
                }
                _ => {}
            }
        }
    }
    if !procs.is_empty() {
        info.logical_processors = Some(procs.len());
        if procs
            .iter()
            .all(|p| p.physical_id.is_some() && p.core_id.is_some())
        {
            let packages: HashSet<u32> = procs.iter().filter_map(|p| p.physical_id).collect();
            let cores: HashSet<(u32, u32)> = procs
                .iter()
                .map(|p| (p.physical_id.unwrap(), p.core_id.unwrap()))
                .collect();
            info.packages = Some(packages.len());
            info.physical_cores = Some(cores.len());

            // Hybrid split: count physical cores whose CPUs are in each list.
            let core_of = |cpu: u32| {
                procs
                    .iter()
                    .find(|p| p.processor == Some(cpu))
                    .map(|p| (p.physical_id.unwrap(), p.core_id.unwrap()))
            };
            let count = |list: &str| -> Option<usize> {
                let cpus = parse_cpu_list(&read(&root.join(list))?);
                let set: HashSet<_> = cpus.into_iter().filter_map(core_of).collect();
                Some(set.len())
            };
            if let (Some(p), Some(e)) = (
                count("sys/devices/cpu_core/cpus"),
                count("sys/devices/cpu_atom/cpus"),
            ) {
                info.performance_cores = Some(p);
                info.efficiency_cores = Some(e);
            }
        }
    }

    // Caches: unique instances by (level, type, shared_cpu_list).
    let mut seen = HashSet::new();
    let mut instances = Vec::new();
    let mut cpus: BTreeMap<u32, std::path::PathBuf> = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(&cpu_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(n) = name.strip_prefix("cpu").and_then(|s| s.parse::<u32>().ok()) {
                cpus.insert(n, e.path());
            }
        }
    }
    for dir in cpus.values() {
        let Ok(entries) = fs::read_dir(dir.join("cache")) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let (Some(level), Some(ty), Some(size), Some(shared)) = (
                read(&p.join("level")).and_then(|s| s.parse::<u8>().ok()),
                read(&p.join("type")),
                read(&p.join("size")).and_then(|s| parse_cache_size(&s)),
                read(&p.join("shared_cpu_list")),
            ) else {
                continue;
            };
            let kind = match ty.as_str() {
                "Data" => "data",
                "Instruction" => "instruction",
                _ => "unified",
            };
            if seen.insert((level, kind, parse_cpu_list(&shared))) {
                instances.push((level, kind, size));
            }
        }
    }
    if !instances.is_empty() {
        info.caches = group_caches(&instances);
        info.sources.push("sysfs cache");
    }

    // Frequencies (kHz -> MHz).
    let khz = |file: &str| -> Option<u32> {
        cpus.values()
            .filter_map(|d| read(&d.join("cpufreq").join(file))?.parse::<u64>().ok())
            .max()
            .map(|k| (k / 1000) as u32)
    };
    info.base_mhz = khz("base_frequency");
    info.max_mhz = khz("cpuinfo_max_freq");
    info.base_mhz_source = info.base_mhz.map(|_| "cpufreq base_frequency");
    info.max_mhz_source = info.max_mhz.map(|_| "cpufreq cpuinfo_max_freq");
    if info.base_mhz.is_some() || info.max_mhz.is_some() {
        info.sources.push("sysfs cpufreq");
    }
    info
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
    fn cpu_lists() {
        assert_eq!(
            parse_cpu_list("0-3,8,10-11\n")
                .into_iter()
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 8, 10, 11]
        );
        assert!(parse_cpu_list("").is_empty());
    }

    #[test]
    fn cache_sizes() {
        assert_eq!(parse_cache_size("48K"), Some(48));
        assert_eq!(parse_cache_size("24M\n"), Some(24 * 1024));
        assert_eq!(parse_cache_size("1280K"), Some(1280));
        assert_eq!(parse_cache_size("junk"), None);
    }

    /// Hybrid CPU: 2 P-cores with HT (cpu0-3) + 2 E-cores (cpu4-5).
    #[test]
    fn hybrid_intel_tree() {
        let t = TempDir::new().unwrap();
        let r = t.path();
        let mut cpuinfo = String::new();
        for (cpu, core) in [(0, 0), (1, 0), (2, 4), (3, 4), (4, 8), (5, 9)] {
            cpuinfo.push_str(&format!(
                "processor\t: {cpu}\nvendor_id\t: GenuineIntel\ncpu family\t: 6\nmodel\t\t: 186\n\
                 model name\t: 13th Gen Intel(R) Core(TM) i7-13620H\nstepping\t: 2\n\
                 microcode\t: 0x4122\nphysical id\t: 0\ncore id\t\t: {core}\n\n"
            ));
        }
        write(r, "proc/cpuinfo", &cpuinfo);
        write(r, "sys/devices/cpu_core/cpus", "0-3\n");
        write(r, "sys/devices/cpu_atom/cpus", "4-5\n");
        for cpu in 0..6 {
            let base = format!("sys/devices/system/cpu/cpu{cpu}");
            let (l2, shared) = if cpu < 4 {
                ("1280K", format!("{}-{}", cpu / 2 * 2, cpu / 2 * 2 + 1))
            } else {
                ("2048K", "4-5".to_string())
            };
            write(r, &format!("{base}/cache/index0/level"), "1");
            write(r, &format!("{base}/cache/index0/type"), "Data");
            write(r, &format!("{base}/cache/index0/size"), "48K");
            write(r, &format!("{base}/cache/index0/shared_cpu_list"), &shared);
            write(r, &format!("{base}/cache/index2/level"), "2");
            write(r, &format!("{base}/cache/index2/type"), "Unified");
            write(r, &format!("{base}/cache/index2/size"), l2);
            write(r, &format!("{base}/cache/index2/shared_cpu_list"), &shared);
            write(r, &format!("{base}/cache/index3/level"), "3");
            write(r, &format!("{base}/cache/index3/type"), "Unified");
            write(r, &format!("{base}/cache/index3/size"), "24576K");
            write(r, &format!("{base}/cache/index3/shared_cpu_list"), "0-5");
            write(r, &format!("{base}/cpufreq/base_frequency"), "2400000");
            write(
                r,
                &format!("{base}/cpufreq/cpuinfo_max_freq"),
                if cpu < 4 { "4900000" } else { "3600000" },
            );
        }

        let i = from_root(r);
        assert_eq!(i.vendor.as_deref(), Some("Intel"));
        assert_eq!(
            i.brand.as_deref(),
            Some("13th Gen Intel(R) Core(TM) i7-13620H")
        );
        assert_eq!(
            (i.family, i.model, i.stepping),
            (Some(6), Some(186), Some(2))
        );
        assert_eq!(i.microcode.as_deref(), Some("0x4122"));
        assert_eq!(i.logical_processors, Some(6));
        assert_eq!(i.physical_cores, Some(4));
        assert_eq!(i.packages, Some(1));
        assert_eq!(
            (i.performance_cores, i.efficiency_cores),
            (Some(2), Some(2))
        );
        assert_eq!((i.base_mhz, i.max_mhz), (Some(2400), Some(4900)));
        let l2 = i.caches.iter().find(|c| c.level == 2).unwrap();
        assert_eq!(l2.instances, [(2048, 1), (1280, 2)]);
        let l1 = i.caches.iter().find(|c| c.level == 1).unwrap();
        assert_eq!(l1.instances, [(48, 3)]); // shared lists 0-1, 2-3, 4-5
        let l3 = i.caches.iter().find(|c| c.level == 3).unwrap();
        assert_eq!(l3.instances, [(24 * 1024, 1)]);
    }

    #[test]
    fn virtual_machine_is_flagged_and_placeholder_microcode_dropped() {
        let t = TempDir::new().unwrap();
        write(
            t.path(),
            "proc/cpuinfo",
            "processor\t: 0\nvendor_id\t: GenuineIntel\nmicrocode\t: 0xffffffff\n\
             flags\t\t: fpu vme hypervisor lahf_lm\n\n",
        );
        let i = from_root(t.path());
        assert!(i.virtual_machine);
        assert_eq!(i.microcode, None);
    }

    #[test]
    fn missing_files_leave_fields_empty() {
        let t = TempDir::new().unwrap();
        let i = from_root(t.path());
        assert_eq!(i, CpuInfo::default());
    }

    #[test]
    fn cpuinfo_without_topology_fields() {
        // e.g. some ARM kernels: no vendor_id/physical id, only processor lines.
        let t = TempDir::new().unwrap();
        write(
            t.path(),
            "proc/cpuinfo",
            "processor\t: 0\nBogoMIPS\t: 108.00\n\nprocessor\t: 1\nBogoMIPS\t: 108.00\n",
        );
        let i = from_root(t.path());
        assert_eq!(i.logical_processors, Some(2));
        assert_eq!(i.physical_cores, None);
        assert_eq!(i.vendor, None);
    }
}
