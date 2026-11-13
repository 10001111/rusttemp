//! Windows CPU topology, caches and microcode revision.

use std::ptr;

use windows_sys::Win32::System::Power::{
    CallNtPowerInformation, PROCESSOR_POWER_INFORMATION, ProcessorInformation,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{
    CacheData, CacheInstruction, CacheTrace, GetLogicalProcessorInformationEx,
    LOGICAL_PROCESSOR_RELATIONSHIP, RelationCache, RelationProcessorCore, RelationProcessorPackage,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};

use super::{CpuInfo, group_caches};
use crate::sensors::platform::intel_dts::CoreTopology;

/// Calls `GetLogicalProcessorInformationEx` and invokes `f` on each record.
fn for_each_record(
    relation: LOGICAL_PROCESSOR_RELATIONSHIP,
    mut f: impl FnMut(&SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX),
) -> Result<(), String> {
    let mut len = 0u32;
    // SAFETY: size query with a null buffer.
    unsafe { GetLogicalProcessorInformationEx(relation, ptr::null_mut(), &mut len) };
    if len == 0 {
        return Err(format!(
            "CPU topology query failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut buf: Vec<u64> = vec![0; (len as usize).div_ceil(8)];
    let base = buf.as_mut_ptr().cast::<u8>();
    // SAFETY: buffer of at least `len` bytes, 8-byte aligned.
    let ok = unsafe {
        GetLogicalProcessorInformationEx(
            relation,
            base.cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(format!(
            "CPU topology query failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut offset = 0usize;
    while offset < len as usize {
        // SAFETY: records are laid out back to back; each `Size` covers its record.
        let rec = unsafe {
            &*base
                .add(offset)
                .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()
        };
        if rec.Size == 0 {
            break;
        }
        if rec.Relationship == relation {
            f(rec);
        }
        offset += rec.Size as usize;
    }
    Ok(())
}

/// Cores or packages: `(group, lowest logical CPU, efficiency class)` each.
pub fn processor_records(
    relation: LOGICAL_PROCESSOR_RELATIONSHIP,
) -> Result<Vec<CoreTopology>, String> {
    let mut out = Vec::new();
    for_each_record(relation, |rec| {
        // SAFETY: `Processor` is the active member for core/package records.
        let p = unsafe { &rec.Anonymous.Processor };
        if p.GroupCount >= 1 {
            // GroupMask is a variable-length array; the first entry suffices.
            let gm = p.GroupMask[0];
            if gm.Mask != 0 {
                out.push(CoreTopology {
                    group: gm.Group,
                    cpu: gm.Mask.trailing_zeros(),
                    efficiency_class: p.EfficiencyClass,
                });
            }
        }
    })?;
    Ok(out)
}

/// One physical core: every logical CPU `(group, index)` it contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreCpus {
    pub cpus: Vec<(u16, u32)>,
    pub efficiency_class: u8,
}

/// All physical cores with their logical CPUs, in OS enumeration order.
pub fn core_cpu_sets() -> Result<Vec<CoreCpus>, String> {
    let mut out = Vec::new();
    for_each_record(RelationProcessorCore, |rec| {
        // SAFETY: `Processor` is the active member for core records.
        let p = unsafe { &rec.Anonymous.Processor };
        if p.GroupCount >= 1 {
            let gm = p.GroupMask[0];
            let cpus = (0..usize::BITS)
                .filter(|b| gm.Mask & (1usize << b) != 0)
                .map(|b| (gm.Group, b))
                .collect::<Vec<_>>();
            if !cpus.is_empty() {
                out.push(CoreCpus {
                    cpus,
                    efficiency_class: p.EfficiencyClass,
                });
            }
        }
    })?;
    Ok(out)
}

/// Cache instances as `(level, kind, size KiB)`.
fn cache_instances() -> Result<Vec<(u8, &'static str, u64)>, String> {
    let mut out = Vec::new();
    for_each_record(RelationCache, |rec| {
        // SAFETY: `Cache` is the active member for cache records.
        let c = unsafe { &rec.Anonymous.Cache };
        let kind = match c.Type {
            t if t == CacheData => "data",
            t if t == CacheInstruction => "instruction",
            t if t == CacheTrace => return,
            _ => "unified",
        };
        out.push((c.Level, kind, c.CacheSize as u64 / 1024));
    })?;
    Ok(out)
}

/// Decodes the registry `Update Revision` value. Windows stores either 4
/// bytes (the revision, little-endian) or 8 bytes (revision in the upper
/// 32 bits).
pub fn decode_update_revision(bytes: &[u8]) -> Option<u32> {
    let rev = match bytes.len() {
        4 => u32::from_le_bytes(bytes.try_into().ok()?),
        8 => {
            let hi = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
            if hi != 0 {
                hi
            } else {
                u32::from_le_bytes(bytes[0..4].try_into().ok()?)
            }
        }
        _ => return None,
    };
    (rev != 0).then_some(rev)
}

fn microcode_revision() -> Option<u32> {
    let key: Vec<u16> = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"
        .encode_utf16()
        .chain([0])
        .collect();
    let value: Vec<u16> = "Update Revision".encode_utf16().chain([0]).collect();
    let mut buf = [0u8; 16];
    let mut len = buf.len() as u32;
    // SAFETY: NUL-terminated strings; buffer and length match.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 {
        return None;
    }
    decode_update_revision(&buf[..len as usize])
}

/// Highest rated ("base") frequency in MHz across logical processors, from
/// `CallNtPowerInformation(ProcessorInformation)` — the value Task Manager
/// shows as "Base speed". Used when CPUID leaf 0x16 is unavailable (e.g.
/// hidden by the hypervisor when Memory Integrity / VBS is enabled).
fn rated_mhz(logical: usize) -> Option<u32> {
    rated_mhz_per_cpu(logical)?
        .into_iter()
        .max()
        .filter(|&m| m > 0)
}

/// Rated MHz of each logical processor (index = processor number).
pub fn rated_mhz_per_cpu(logical: usize) -> Option<Vec<u32>> {
    let mut buf = vec![PROCESSOR_POWER_INFORMATION::default(); logical.max(1)];
    let bytes = (buf.len() * std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()) as u32;
    // SAFETY: output buffer of `bytes` bytes; no input buffer.
    let status = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            ptr::null(),
            0,
            buf.as_mut_ptr().cast(),
            bytes,
        )
    };
    if status != 0 {
        return None;
    }
    Some(buf.iter().map(|p| p.MaxMhz).collect())
}

/// Topology, caches and microcode from Windows APIs.
pub fn detect() -> CpuInfo {
    let mut info = CpuInfo {
        sources: vec!["windows topology"],
        ..Default::default()
    };
    if let Ok(cores) = processor_records(RelationProcessorCore) {
        info.physical_cores = Some(cores.len());
        let max = cores.iter().map(|c| c.efficiency_class).max();
        let min = cores.iter().map(|c| c.efficiency_class).min();
        if max != min {
            let p = cores
                .iter()
                .filter(|c| Some(c.efficiency_class) == max)
                .count();
            info.performance_cores = Some(p);
            info.efficiency_cores = Some(cores.len() - p);
        }
    }
    if let Ok(pk) = processor_records(RelationProcessorPackage) {
        info.packages = Some(pk.len());
    }
    if let Ok(inst) = cache_instances() {
        info.caches = group_caches(&inst);
    }
    info.logical_processors = std::thread::available_parallelism().ok().map(|n| n.get());
    if let Some(mhz) = info.logical_processors.and_then(rated_mhz) {
        info.base_mhz = Some(mhz);
        info.base_mhz_source = Some("Windows power information (rated speed)");
        info.sources.push("windows power information");
    }
    if let Some(rev) = microcode_revision() {
        info.microcode = Some(format!("0x{rev:x}"));
        info.sources.push("windows registry");
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_revision_formats() {
        // Observed on the i7-13620H test machine: 22 41 00 00.
        assert_eq!(decode_update_revision(&[0x22, 0x41, 0, 0]), Some(0x4122));
        // 8-byte form: revision in the upper DWORD.
        assert_eq!(
            decode_update_revision(&[0, 0, 0, 0, 0x28, 0x41, 0, 0]),
            Some(0x4128)
        );
        assert_eq!(decode_update_revision(&[0; 4]), None);
        assert_eq!(decode_update_revision(&[1, 2, 3]), None);
    }

    #[test]
    fn live_core_sets_cover_all_logical_cpus() {
        let sets = core_cpu_sets().unwrap();
        let total: usize = sets.iter().map(|c| c.cpus.len()).sum();
        assert_eq!(total, std::thread::available_parallelism().unwrap().get());
        let per = rated_mhz_per_cpu(total).unwrap();
        assert_eq!(per.len(), total);
    }

    #[test]
    fn live_topology() {
        let i = detect();
        let cores = i.physical_cores.unwrap();
        assert!(cores >= 1 && cores <= i.logical_processors.unwrap());
        assert!(!i.caches.is_empty());
        assert!(i.base_mhz.is_some_and(|m| (100..20_000).contains(&m)));
    }
}
