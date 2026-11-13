//! CPU identification: brand, model, core counts, caches, clocks.
//!
//! Sources, in order of preference:
//! - **CPUID** (x86-64, any OS): vendor, brand string, family/model/stepping,
//!   base/max frequency (leaf 0x16, when the CPU reports it).
//! - **Windows**: core/package/cache topology from
//!   `GetLogicalProcessorInformationEx`; microcode revision from the registry
//!   value `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0\Update Revision`.
//! - **Linux**: `/proc/cpuinfo`, `/sys/devices/system/cpu/*` (topology,
//!   caches, cpufreq, hybrid core lists).
//!
//! Every field is optional except the logical CPU count: anything the system
//! does not report is shown as unavailable, never guessed.

pub mod cpuid;
pub mod linux;
#[cfg(windows)]
pub mod windows;

use std::path::Path;

use serde::Serialize;

/// One cache level/type, e.g. "L2 Unified: 6 × 1.25 MiB + 1 × 2 MiB".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheGroup {
    /// 1, 2, 3, ...
    pub level: u8,
    /// `data`, `instruction` or `unified`.
    pub kind: &'static str,
    /// `(size in KiB, number of instances)`, largest first. Serialized as
    /// `[{"size_kib": .., "count": ..}]`.
    #[serde(serialize_with = "serialize_instances")]
    pub instances: Vec<(u64, u32)>,
}

fn serialize_instances<S: serde::Serializer>(v: &[(u64, u32)], s: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Instance {
        size_kib: u64,
        count: u32,
    }
    s.collect_seq(
        v.iter()
            .map(|&(size_kib, count)| Instance { size_kib, count }),
    )
}

impl CacheGroup {
    /// Total size in KiB.
    pub fn total_kib(&self) -> u64 {
        self.instances.iter().map(|(s, n)| s * *n as u64).sum()
    }
}

/// Identity and topology of the processor(s).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct CpuInfo {
    /// Raw vendor id, e.g. `GenuineIntel`.
    pub vendor_id: Option<String>,
    /// Manufacturer name, e.g. `Intel`.
    pub vendor: Option<String>,
    /// Marketing name, e.g. `13th Gen Intel(R) Core(TM) i7-13620H`.
    pub brand: Option<String>,
    pub family: Option<u32>,
    pub model: Option<u32>,
    pub stepping: Option<u32>,
    /// Microcode revision, e.g. `0x4122`.
    pub microcode: Option<String>,
    pub packages: Option<usize>,
    pub physical_cores: Option<usize>,
    pub logical_processors: Option<usize>,
    /// Performance cores on hybrid CPUs.
    pub performance_cores: Option<usize>,
    /// Efficiency cores on hybrid CPUs.
    pub efficiency_cores: Option<usize>,
    pub caches: Vec<CacheGroup>,
    pub base_mhz: Option<u32>,
    pub max_mhz: Option<u32>,
    /// Where `base_mhz` came from.
    pub base_mhz_source: Option<&'static str>,
    /// Where `max_mhz` came from.
    pub max_mhz_source: Option<&'static str>,
    /// True when the OS reports running inside a virtual machine (Linux
    /// `hypervisor` CPU flag). Topology, caches and microcode then describe
    /// the virtual CPU, which may differ from the physical processor.
    pub virtual_machine: bool,
    /// Where the data came from.
    pub sources: Vec<&'static str>,
}

/// Human-readable manufacturer for a CPUID vendor id.
pub fn vendor_name(vendor_id: &str) -> String {
    match vendor_id.trim() {
        "GenuineIntel" => "Intel".into(),
        "AuthenticAMD" => "AMD".into(),
        "HygonGenuine" => "Hygon".into(),
        "CentaurHauls" => "Centaur / VIA".into(),
        "  Shanghai  " | "Shanghai" => "Zhaoxin".into(),
        other => other.to_string(),
    }
}

/// Collapses repeated whitespace in brand strings.
pub fn tidy_brand(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Groups `(level, kind, size_kib)` cache instances.
pub fn group_caches(instances: &[(u8, &'static str, u64)]) -> Vec<CacheGroup> {
    let mut groups: Vec<CacheGroup> = Vec::new();
    for &(level, kind, size) in instances {
        let g = match groups
            .iter_mut()
            .find(|g| g.level == level && g.kind == kind)
        {
            Some(g) => g,
            None => {
                groups.push(CacheGroup {
                    level,
                    kind,
                    instances: Vec::new(),
                });
                groups.last_mut().unwrap()
            }
        };
        match g.instances.iter_mut().find(|(s, _)| *s == size) {
            Some((_, n)) => *n += 1,
            None => g.instances.push((size, 1)),
        }
    }
    let kind_order = |k: &str| match k {
        "data" => 0,
        "instruction" => 1,
        _ => 2,
    };
    for g in &mut groups {
        g.instances.sort_by_key(|i| std::cmp::Reverse(i.0));
    }
    groups.sort_by_key(|g| (g.level, kind_order(g.kind)));
    groups
}

/// Formats a size in KiB as KiB/MiB.
pub fn format_kib(kib: u64) -> String {
    if kib >= 1024 && kib % 256 == 0 {
        let mib = kib as f64 / 1024.0;
        if mib.fract() == 0.0 {
            format!("{mib:.0} MiB")
        } else {
            format!("{mib} MiB")
        }
    } else {
        format!("{kib} KiB")
    }
}

impl CpuInfo {
    /// One-line summary for headers.
    pub fn summary(&self) -> String {
        let mut parts = vec![
            self.brand
                .clone()
                .unwrap_or_else(|| "unknown CPU model".into()),
        ];
        if let Some(v) = &self.vendor {
            parts.push(v.clone());
        }
        match (self.physical_cores, self.logical_processors) {
            (Some(c), Some(t)) => parts.push(format!("{c} cores / {t} threads")),
            (None, Some(t)) => parts.push(format!("{t} threads")),
            _ => {}
        }
        parts.join(" · ")
    }

    /// Fills fields missing in `self` from `other`.
    fn merge(&mut self, other: CpuInfo) {
        macro_rules! fill {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = other.$f; } )* };
        }
        fill!(
            vendor_id,
            vendor,
            brand,
            family,
            model,
            stepping,
            microcode,
            packages,
            physical_cores,
            logical_processors,
            performance_cores,
            efficiency_cores,
            base_mhz,
            max_mhz,
            base_mhz_source,
            max_mhz_source
        );
        if self.caches.is_empty() {
            self.caches = other.caches;
        }
        self.virtual_machine |= other.virtual_machine;
        for s in other.sources {
            if !self.sources.contains(&s) {
                self.sources.push(s);
            }
        }
    }
}

/// Detects the CPU. With `sysfs_root`, only the Linux-style files under that
/// root are used (diagnostics/testing), so results are reproducible.
pub fn detect(sysfs_root: Option<&Path>) -> CpuInfo {
    if let Some(root) = sysfs_root {
        return linux::from_root(root);
    }
    let mut info = cpuid::detect();
    #[cfg(windows)]
    info.merge(windows::detect());
    #[cfg(target_os = "linux")]
    info.merge(linux::from_root(Path::new("/")));
    if info.logical_processors.is_none() {
        info.logical_processors = std::thread::available_parallelism().ok().map(|n| n.get());
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendors() {
        assert_eq!(vendor_name("GenuineIntel"), "Intel");
        assert_eq!(vendor_name("AuthenticAMD"), "AMD");
        assert_eq!(vendor_name("Weird"), "Weird");
    }

    #[test]
    fn brand_whitespace() {
        assert_eq!(
            tidy_brand("  Intel(R) Core(TM)   i7-13620H  "),
            "Intel(R) Core(TM) i7-13620H"
        );
    }

    #[test]
    fn cache_grouping_matches_i7_13620h_layout() {
        let mut inst = Vec::new();
        inst.extend(std::iter::repeat_n((2, "unified", 1280), 6));
        inst.push((2, "unified", 2048));
        inst.extend(std::iter::repeat_n((1, "data", 48), 6));
        inst.extend(std::iter::repeat_n((1, "data", 32), 4));
        inst.push((3, "unified", 24 * 1024));
        let g = group_caches(&inst);
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].level, g[0].kind), (1, "data"));
        assert_eq!(g[0].instances, [(48, 6), (32, 4)]);
        assert_eq!(g[1].instances, [(2048, 1), (1280, 6)]);
        assert_eq!(g[1].total_kib(), 9728); // what WMI reports as L2CacheSize
        assert_eq!(g[2].total_kib(), 24_576);
    }

    #[test]
    fn kib_formatting() {
        assert_eq!(format_kib(48), "48 KiB");
        assert_eq!(format_kib(1280), "1.25 MiB");
        assert_eq!(format_kib(2048), "2 MiB");
        assert_eq!(format_kib(24_576), "24 MiB");
        assert_eq!(format_kib(1000), "1000 KiB");
    }

    #[test]
    fn merge_keeps_existing_values() {
        let mut a = CpuInfo {
            brand: Some("A".into()),
            sources: vec!["cpuid"],
            ..Default::default()
        };
        let b = CpuInfo {
            brand: Some("B".into()),
            physical_cores: Some(4),
            sources: vec!["os"],
            ..Default::default()
        };
        a.merge(b);
        assert_eq!(a.brand.as_deref(), Some("A"));
        assert_eq!(a.physical_cores, Some(4));
        assert_eq!(a.sources, ["cpuid", "os"]);
    }

    #[test]
    fn summary_line() {
        let i = CpuInfo {
            brand: Some("X1".into()),
            vendor: Some("Intel".into()),
            physical_cores: Some(10),
            logical_processors: Some(16),
            ..Default::default()
        };
        assert_eq!(i.summary(), "X1 · Intel · 10 cores / 16 threads");
        assert_eq!(CpuInfo::default().summary(), "unknown CPU model");
    }

    #[test]
    fn detect_on_this_machine_reports_threads() {
        let i = detect(None);
        assert!(i.logical_processors.unwrap_or(0) >= 1);
    }
}
