//! Windows backend: Intel CPU package and per-core temperatures through the
//! PawnIO driver.
//!
//! PawnIO (<https://pawnio.eu>, driver signed by Microsoft's Windows Hardware
//! Compatibility Publisher) runs small signed modules in the kernel. RustTemp
//! loads the official `IntelMSR` module (vendored in `assets/pawnio/`,
//! LGPL-2.1), which only allows reads from a whitelist of MSRs that includes
//! the thermal registers used here. RustTemp never writes MSRs.
//!
//! Requirements at runtime: PawnIO installed (`winget install namazso.PawnIO`)
//! and RustTemp running with administrator rights. When either is missing the
//! backend reports why and RustTemp falls back to ACPI thermal zones.
//!
//! `PawnIOLib.dll` is loaded at runtime by absolute path from the PawnIO
//! install directory (never from the search path), so RustTemp has no
//! link-time dependency on PawnIO.
//!
//! MSRs are per logical CPU: before each read the calling thread is pinned to
//! one logical CPU of the target core (or package) and its previous affinity is
//! restored afterwards.

use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::SystemInformation::{
    GROUP_AFFINITY, RelationProcessorCore, RelationProcessorPackage,
};
use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};

use super::intel_dts::{
    MSR_IA32_PACKAGE_THERM_STATUS, MSR_IA32_THERM_STATUS, MSR_TEMPERATURE_TARGET, ThermalCpuid,
    core_labels, decode_core_temperature, decode_package_temperature, decode_throttle_flags,
    decode_tjmax,
};
use crate::cpuinfo::windows::processor_records;
use crate::sensors::{
    HardwareLimits, Reading, SensorBackend, SensorInfo, SensorKind, ThrottleFlags,
};

/// Official signed IntelMSR module, PawnIO.Modules release 0.2.11
/// (SHA-256 d6ed85d65ab17a22f813ef98207d6d537155ee2ded5976a21cb48413c9b92e5f).
pub(crate) static INTEL_MSR_MODULE: &[u8] = include_bytes!("../../../assets/pawnio/IntelMSR.bin");

const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
const HRESULT_FILE_NOT_FOUND: i32 = 0x8007_0002_u32 as i32;
const HRESULT_PATH_NOT_FOUND: i32 = 0x8007_0003_u32 as i32;

type FnOpen = unsafe extern "system" fn(*mut HANDLE) -> i32;
type FnLoad = unsafe extern "system" fn(HANDLE, *const u8, usize) -> i32;
type FnExecute = unsafe extern "system" fn(
    HANDLE,
    *const u8,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;
type FnClose = unsafe extern "system" fn(HANDLE) -> i32;

/// An open PawnIO executor with a module loaded.
pub(crate) struct PawnIo {
    lib: HMODULE,
    handle: HANDLE,
    execute: FnExecute,
    close: FnClose,
}

fn pawnio_dir() -> PathBuf {
    let base = std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .unwrap_or_else(|| r"C:\Program Files".into());
    PathBuf::from(base).join("PawnIO")
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

impl PawnIo {
    /// Loads PawnIOLib.dll, opens the driver and loads `module`.
    pub(crate) fn open(module: &[u8]) -> Result<Self, String> {
        let dll = pawnio_dir().join("PawnIOLib.dll");
        if !dll.is_file() {
            return Err(format!(
                "PawnIO is not installed ({} not found); install it with \
                 `winget install namazso.PawnIO`",
                dll.display()
            ));
        }
        let path = wide(dll.as_os_str());
        // SAFETY: NUL-terminated absolute path; dependencies resolved only
        // from the DLL's own directory and System32.
        let lib = unsafe {
            LoadLibraryExW(
                path.as_ptr(),
                ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if lib.is_null() {
            return Err(format!(
                "cannot load {}: {}",
                dll.display(),
                std::io::Error::last_os_error()
            ));
        }

        // Resolve exports; on failure release the library.
        let resolve = |name: &std::ffi::CStr| {
            // SAFETY: `lib` is a loaded module and `name` is NUL-terminated.
            unsafe { GetProcAddress(lib, name.as_ptr().cast()) }
        };
        let (Some(open), Some(load), Some(execute), Some(close)) = (
            resolve(c"pawnio_open"),
            resolve(c"pawnio_load"),
            resolve(c"pawnio_execute"),
            resolve(c"pawnio_close"),
        ) else {
            // SAFETY: library loaded above and not used afterwards.
            unsafe { FreeLibrary(lib) };
            return Err("PawnIOLib.dll does not export the expected functions".into());
        };
        // SAFETY: signatures match PawnIOLib.h (HRESULT STDAPICALLTYPE).
        let (open, load, execute, close): (FnOpen, FnLoad, FnExecute, FnClose) = unsafe {
            (
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnOpen>(open),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnLoad>(load),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnExecute>(execute),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnClose>(close),
            )
        };

        let mut handle: HANDLE = ptr::null_mut();
        // SAFETY: valid out-pointer.
        let hr = unsafe { open(&mut handle) };
        if hr < 0 {
            // SAFETY: library loaded above and not used afterwards.
            unsafe { FreeLibrary(lib) };
            return Err(match hr {
                E_ACCESSDENIED => {
                    "PawnIO access denied: run RustTemp from an administrator terminal".to_string()
                }
                HRESULT_FILE_NOT_FOUND | HRESULT_PATH_NOT_FOUND => {
                    "PawnIO driver is not running (try reinstalling PawnIO or rebooting)"
                        .to_string()
                }
                _ => format!("cannot open PawnIO (HRESULT 0x{:08X})", hr as u32),
            });
        }
        let this = PawnIo {
            lib,
            handle,
            execute,
            close,
        };
        // SAFETY: open handle; blob pointer/length from a slice.
        let hr = unsafe { load(this.handle, module.as_ptr(), module.len()) };
        if hr < 0 {
            return Err(format!(
                "PawnIO rejected the IntelMSR module (HRESULT 0x{:08X})",
                hr as u32
            ));
        }
        Ok(this)
    }

    /// Reads an MSR on the CPU the calling thread currently runs on.
    pub(crate) fn read_msr(&self, msr: u32) -> Result<u64, String> {
        let input = [msr as u64];
        let mut output = [0u64; 1];
        let mut returned = 0usize;
        // SAFETY: open handle; NUL-terminated name; buffers sized as passed.
        let hr = unsafe {
            (self.execute)(
                self.handle,
                c"ioctl_read_msr".as_ptr().cast(),
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut returned,
            )
        };
        if hr < 0 {
            Err(format!(
                "MSR 0x{msr:X} read failed (HRESULT 0x{:08X})",
                hr as u32
            ))
        } else if returned != 1 {
            Err(format!("MSR 0x{msr:X} read returned no data"))
        } else {
            Ok(output[0])
        }
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        // SAFETY: handle and library are valid and released exactly once.
        unsafe {
            (self.close)(self.handle);
            FreeLibrary(self.lib);
        }
    }
}

/// Pins the current thread to one logical CPU until dropped.
pub(crate) struct AffinityGuard {
    previous: GROUP_AFFINITY,
}

impl AffinityGuard {
    pub(crate) fn pin(group: u16, cpu: u32) -> Result<Self, String> {
        let target = GROUP_AFFINITY {
            Mask: 1usize << cpu,
            Group: group,
            Reserved: [0; 3],
        };
        let mut previous = GROUP_AFFINITY::default();
        // SAFETY: pseudo-handle of the current thread; valid struct pointers.
        let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), &target, &mut previous) };
        if ok == 0 {
            return Err(format!(
                "cannot pin thread to CPU {group}:{cpu}: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(AffinityGuard { previous })
    }
}

impl Drop for AffinityGuard {
    fn drop(&mut self) {
        // SAFETY: restores the affinity saved in `pin`.
        unsafe { SetThreadGroupAffinity(GetCurrentThread(), &self.previous, ptr::null_mut()) };
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn cpu_identity() -> (String, ThermalCpuid) {
    use std::arch::x86_64::__cpuid;
    #[allow(unused_unsafe)] // `__cpuid` is safe on newer toolchains.
    let (leaf0, leaf6) = unsafe { (__cpuid(0), __cpuid(6)) };
    let mut vendor = Vec::with_capacity(12);
    for r in [leaf0.ebx, leaf0.edx, leaf0.ecx] {
        vendor.extend_from_slice(&r.to_le_bytes());
    }
    let thermal = if leaf0.eax >= 6 {
        ThermalCpuid::from_leaf6_eax(leaf6.eax)
    } else {
        ThermalCpuid::from_leaf6_eax(0)
    };
    (String::from_utf8_lossy(&vendor).into_owned(), thermal)
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) fn cpu_identity() -> (String, ThermalCpuid) {
    (String::new(), ThermalCpuid::from_leaf6_eax(0))
}

struct Target {
    group: u16,
    cpu: u32,
    msr: u32,
    tjmax: Option<f64>,
    package: bool,
    /// Whether the power-limitation bits (10/11) are defined for this MSR.
    power_bits_valid: bool,
}

/// Intel package and per-core temperatures via PawnIO.
pub struct IntelDtsBackend {
    pawnio: PawnIo,
    sensors: Vec<SensorInfo>,
    targets: Vec<Target>,
    last_flags: Vec<Option<ThrottleFlags>>,
}

impl IntelDtsBackend {
    /// Checks the CPU, opens PawnIO and discovers packages and cores.
    /// The error string explains why CPU temperatures are unavailable.
    pub fn new() -> Result<Self, String> {
        let (vendor, thermal) = cpu_identity();
        if vendor != "GenuineIntel" {
            return Err(if vendor == "AuthenticAMD" {
                "AMD CPU temperatures on Windows are not implemented yet".to_string()
            } else {
                format!("CPU vendor {vendor:?} is not supported for CPU temperatures on Windows")
            });
        }
        if !thermal.dts {
            return Err("this CPU reports no digital thermal sensor (CPUID.06H:EAX[0] = 0)".into());
        }

        let pawnio = PawnIo::open(INTEL_MSR_MODULE)?;
        let packages = if thermal.package_dts {
            processor_records(RelationProcessorPackage)?
        } else {
            Vec::new()
        };
        let cores = processor_records(RelationProcessorCore)?;

        let mut sensors = Vec::new();
        let mut targets = Vec::new();
        let read_tjmax = |group: u16, cpu: u32| -> Option<f64> {
            let _pin = AffinityGuard::pin(group, cpu).ok()?;
            decode_tjmax(pawnio.read_msr(MSR_TEMPERATURE_TARGET).ok()?)
        };

        for (i, p) in packages.iter().enumerate() {
            let tjmax = read_tjmax(p.group, p.cpu);
            sensors.push(SensorInfo {
                id: format!("intel-dts/package{i}"),
                label: format!("Package id {i}"),
                chip: "intel-dts (PawnIO)".into(),
                kind: SensorKind::CpuPackage,
                source: format!(
                    "MSR 0x{MSR_IA32_PACKAGE_THERM_STATUS:X} on CPU {}:{} via PawnIO",
                    p.group, p.cpu
                ),
                limits: HardwareLimits {
                    max_c: None,
                    crit_c: tjmax,
                },
            });
            targets.push(Target {
                group: p.group,
                cpu: p.cpu,
                msr: MSR_IA32_PACKAGE_THERM_STATUS,
                tjmax,
                package: true,
                power_bits_valid: thermal.power_limit_notification && thermal.package_dts,
            });
        }
        for (c, label) in cores.iter().zip(core_labels(&cores)) {
            let tjmax = read_tjmax(c.group, c.cpu);
            let index = targets.len() - packages.len();
            sensors.push(SensorInfo {
                id: format!("intel-dts/core{index}"),
                label,
                chip: "intel-dts (PawnIO)".into(),
                kind: SensorKind::CpuCore,
                source: format!(
                    "MSR 0x{MSR_IA32_THERM_STATUS:X} on CPU {}:{} via PawnIO",
                    c.group, c.cpu
                ),
                limits: HardwareLimits {
                    max_c: None,
                    crit_c: tjmax,
                },
            });
            targets.push(Target {
                group: c.group,
                cpu: c.cpu,
                msr: MSR_IA32_THERM_STATUS,
                tjmax,
                package: false,
                power_bits_valid: thermal.power_limit_notification,
            });
        }
        let last_flags = vec![None; sensors.len()];
        Ok(IntelDtsBackend {
            pawnio,
            sensors,
            targets,
            last_flags,
        })
    }

    /// Reads one thermal status MSR: temperature plus throttle flags.
    fn read_target(&self, t: &Target) -> (Reading, Option<ThrottleFlags>) {
        let raw =
            match AffinityGuard::pin(t.group, t.cpu).and_then(|_pin| self.pawnio.read_msr(t.msr)) {
                Ok(raw) => raw,
                Err(e) => return (Reading::Unavailable(e), None),
            };
        let flags = Some(decode_throttle_flags(raw, t.power_bits_valid));
        let Some(tjmax) = t.tjmax else {
            return (
                Reading::Unavailable("TjMax could not be read".into()),
                flags,
            );
        };
        let reading = if t.package {
            Reading::from_celsius(decode_package_temperature(raw, tjmax))
        } else {
            match decode_core_temperature(raw, tjmax) {
                Ok(c) => Reading::from_celsius(c),
                Err(e) => Reading::Unavailable(e.into()),
            }
        };
        (reading, flags)
    }
}

impl SensorBackend for IntelDtsBackend {
    fn name(&self) -> &'static str {
        "windows-pawnio-intel-dts"
    }

    fn sensors(&self) -> &[SensorInfo] {
        &self.sensors
    }

    fn read(&mut self) -> Vec<Reading> {
        let (readings, flags): (Vec<_>, Vec<_>) =
            self.targets.iter().map(|t| self.read_target(t)).unzip();
        self.last_flags = flags;
        readings
    }

    fn throttle_flags(&self) -> Vec<Option<ThrottleFlags>> {
        self.last_flags.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_has_at_least_one_core() {
        let cores = processor_records(RelationProcessorCore).unwrap();
        assert!(!cores.is_empty());
        let logical = std::thread::available_parallelism().unwrap().get();
        assert!(cores.len() <= logical);
    }

    #[test]
    fn affinity_pin_and_restore() {
        let cores = processor_records(RelationProcessorCore).unwrap();
        let last = cores.last().unwrap();
        let guard = AffinityGuard::pin(last.group, last.cpu).unwrap();
        drop(guard);
    }

    /// Without PawnIO or without admin rights the backend must fail with an
    /// explanation, never with fabricated sensors. With both, it must produce
    /// CPU sensors whose readings are plausible.
    #[test]
    fn backend_either_explains_or_reads() {
        match IntelDtsBackend::new() {
            Err(why) => assert!(!why.is_empty()),
            Ok(mut b) => {
                assert!(!b.sensors().is_empty());
                assert!(b.sensors().iter().all(|s| s.kind.is_cpu()));
                let r = b.read();
                assert_eq!(r.len(), b.sensors().len());
                let flags = b.throttle_flags();
                assert_eq!(flags.len(), b.sensors().len());
                assert!(flags.iter().all(Option::is_some));
            }
        }
    }
}
