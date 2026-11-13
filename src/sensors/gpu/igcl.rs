//! Intel Graphics Control Library (IGCL) bindings, loaded at runtime from
//! `System32\ControlLib.dll` (installed with Intel graphics drivers).
//!
//! Struct layouts mirror `igcl_api.h` (intel/drivers.gpu.control-library,
//! API 1.1). Offsets were printed with MSVC from the real header and are
//! asserted below. Booleans are read as `u8`, so an out-of-range byte from
//! the driver can never become undefined behaviour.
//!
//! Validated on Intel UHD Graphics (Raptor Lake-P) with driver 32.0.101.7076:
//! - **Valid:** clock, activity counters, throttle flags.
//! - **Not supported** by the driver: temperature, VRAM telemetry.
//! - **Not used:** the GPU energy counter equals the CPU package energy, and
//!   the voltage fails plausibility (see docs/HARDWARE_MONITOR_PLAN.md §A2).

use std::ffi::c_void;
use std::ptr;

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE, LUID};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

/// `CTL_MAKE_VERSION(1, 1)`.
const CTL_IMPL_VERSION: u32 = 0x0001_0001;
const CTL_INIT_FLAG_USE_LEVEL_ZERO: u32 = 1;
const CTL_DATA_TYPE_FLOAT: u32 = 8;
const CTL_DATA_TYPE_DOUBLE: u32 = 9;

#[repr(C)]
struct CtlInitArgs {
    size: u32,
    version: u8,
    _pad: [u8; 3],
    app_version: u32,
    flags: u32,
    supported_version: u32,
    application_uid: [u8; 16],
}

#[repr(C)]
struct CtlDeviceAdapterProperties {
    size: u32,
    version: u8,
    _pad0: [u8; 3],
    p_device_id: *mut c_void,
    device_id_size: u32,
    device_type: u32,
    supported_subfunction_flags: u32,
    _pad1: u32,
    driver_version: u64,
    firmware_version: [u64; 3],
    pci_vendor_id: u32,
    pci_device_id: u32,
    rev_id: u32,
    num_eus_per_sub_slice: u32,
    num_sub_slices_per_slice: u32,
    num_slices: u32,
    name: [u8; 100],
    graphics_adapter_properties: u32,
    frequency: u32,
    pci_subsys_id: u16,
    pci_subsys_vendor_id: u16,
    adapter_bdf: [u8; 3],
    _pad2: u8,
    num_xe_cores: u32,
    reserved: [u8; 108],
}

/// `ctl_oc_telemetry_item_t`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CtlTelemetryItem {
    supported: u8,
    _pad0: [u8; 3],
    units: u32,
    data_type: u32,
    _pad1: u32,
    value: u64,
}

impl CtlTelemetryItem {
    /// The value as f64 when supported and of a floating-point type.
    fn get(&self) -> Option<f64> {
        if self.supported == 0 {
            return None;
        }
        match self.data_type {
            CTL_DATA_TYPE_DOUBLE => Some(f64::from_bits(self.value)),
            CTL_DATA_TYPE_FLOAT => Some(f32::from_bits(self.value as u32) as f64),
            // Integer types: 0..7 (signed/unsigned 8..64 bit).
            4 => Some(self.value as u32 as i32 as f64),
            5 => Some(self.value as u32 as f64),
            6 => Some(self.value as i64 as f64),
            7 => Some(self.value as f64),
            _ => None,
        }
    }
}

/// `ctl_power_telemetry_t` (1024 bytes; only the leading fields are named).
#[repr(C)]
struct CtlPowerTelemetry {
    size: u32,
    version: u8,
    _pad0: [u8; 3],
    time_stamp: CtlTelemetryItem,
    gpu_energy_counter: CtlTelemetryItem,
    gpu_voltage: CtlTelemetryItem,
    gpu_current_clock_frequency: CtlTelemetryItem,
    gpu_current_temperature: CtlTelemetryItem,
    global_activity_counter: CtlTelemetryItem,
    render_compute_activity_counter: CtlTelemetryItem,
    media_activity_counter: CtlTelemetryItem,
    gpu_power_limited: u8,
    gpu_temperature_limited: u8,
    gpu_current_limited: u8,
    gpu_voltage_limited: u8,
    gpu_utilization_limited: u8,
    _pad1: [u8; 3],
    rest: [u8; 1024 - 208],
}

const _: () = assert!(std::mem::size_of::<CtlInitArgs>() == 36);
const _: () = assert!(std::mem::size_of::<CtlDeviceAdapterProperties>() == 320);
const _: () = assert!(std::mem::size_of::<CtlTelemetryItem>() == 24);
const _: () = assert!(std::mem::size_of::<CtlPowerTelemetry>() == 1024);

type Handle = *mut c_void;
type FnInit = unsafe extern "C" fn(*mut CtlInitArgs, *mut Handle) -> u32;
type FnClose = unsafe extern "C" fn(Handle) -> u32;
type FnEnum = unsafe extern "C" fn(Handle, *mut u32, *mut Handle) -> u32;
type FnProps = unsafe extern "C" fn(Handle, *mut CtlDeviceAdapterProperties) -> u32;
type FnTelemetry = unsafe extern "C" fn(Handle, *mut CtlPowerTelemetry) -> u32;

/// One IGCL device.
pub struct IgclDevice {
    handle: Handle,
    pub luid: (i32, u32),
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    /// Execution units (EU per subslice × subslices per slice × slices).
    pub execution_units: Option<u32>,
}

/// Instantaneous telemetry from one device.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct IgclTelemetry {
    pub clock_mhz: Option<f64>,
    pub temperature_c: Option<f64>,
    pub voltage_v: Option<f64>,
    /// Seconds any engine was busy (monotonic counter).
    pub global_activity_s: Option<f64>,
    /// Timestamp in seconds (monotonic).
    pub timestamp_s: Option<f64>,
    pub power_limited: bool,
    pub temperature_limited: bool,
}

/// An initialized IGCL session.
pub struct Igcl {
    lib: HMODULE,
    api: Handle,
    close: FnClose,
    telemetry: FnTelemetry,
    pub devices: Vec<IgclDevice>,
}

impl Igcl {
    /// Loads ControlLib.dll from System32 and enumerates devices.
    pub fn open() -> Result<Self, String> {
        let name: Vec<u16> = "ControlLib.dll".encode_utf16().chain([0]).collect();
        // SAFETY: NUL-terminated name, loaded from System32 only.
        let lib =
            unsafe { LoadLibraryExW(name.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if lib.is_null() {
            return Err("Intel Graphics Control Library (ControlLib.dll) is not installed".into());
        }
        let sym = |n: &std::ffi::CStr| {
            // SAFETY: `lib` is loaded; `n` is NUL-terminated.
            unsafe { GetProcAddress(lib, n.as_ptr().cast()) }
        };
        let (Some(init), Some(close), Some(en), Some(props), Some(tel)) = (
            sym(c"ctlInit"),
            sym(c"ctlClose"),
            sym(c"ctlEnumerateDevices"),
            sym(c"ctlGetDeviceProperties"),
            sym(c"ctlPowerTelemetryGet"),
        ) else {
            // SAFETY: loaded above, not used afterwards.
            unsafe { FreeLibrary(lib) };
            return Err("ControlLib.dll does not export the expected functions".into());
        };
        // SAFETY: signatures match igcl_api.h (CTL_APICALL = cdecl on x64).
        let (init, close, en, props, telemetry) = unsafe {
            (
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnInit>(init),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnClose>(close),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnEnum>(en),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnProps>(props),
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FnTelemetry>(tel),
            )
        };
        let mut args = CtlInitArgs {
            size: std::mem::size_of::<CtlInitArgs>() as u32,
            version: 0,
            _pad: [0; 3],
            app_version: CTL_IMPL_VERSION,
            flags: CTL_INIT_FLAG_USE_LEVEL_ZERO,
            supported_version: 0,
            application_uid: [0; 16],
        };
        let mut api: Handle = ptr::null_mut();
        // SAFETY: valid args and out-pointer.
        let r = unsafe { init(&mut args, &mut api) };
        if r != 0 {
            // SAFETY: loaded above, not used afterwards.
            unsafe { FreeLibrary(lib) };
            return Err(format!("ctlInit failed (0x{r:08X})"));
        }
        let mut s = Igcl {
            lib,
            api,
            close,
            telemetry,
            devices: Vec::new(),
        };
        let mut count = 0u32;
        // SAFETY: count query with a null array.
        if unsafe { en(api, &mut count, ptr::null_mut()) } != 0 || count == 0 {
            return Ok(s);
        }
        let mut handles = vec![ptr::null_mut::<c_void>(); count as usize];
        // SAFETY: `handles` holds `count` entries.
        if unsafe { en(api, &mut count, handles.as_mut_ptr()) } != 0 {
            return Ok(s);
        }
        handles.truncate(count as usize);
        for h in handles {
            let mut luid = LUID::default();
            // SAFETY: all-zero is a valid bit pattern for this plain-data struct.
            let mut p: CtlDeviceAdapterProperties = unsafe { std::mem::zeroed() };
            p.size = std::mem::size_of::<CtlDeviceAdapterProperties>() as u32;
            p.version = 2;
            p.p_device_id = (&mut luid as *mut LUID).cast();
            p.device_id_size = std::mem::size_of::<LUID>() as u32;
            // SAFETY: valid device handle and properties buffer.
            if unsafe { props(h, &mut p) } != 0 {
                continue;
            }
            let end = p.name.iter().position(|&c| c == 0).unwrap_or(p.name.len());
            let eus = p
                .num_eus_per_sub_slice
                .checked_mul(p.num_sub_slices_per_slice)
                .and_then(|x| x.checked_mul(p.num_slices))
                .filter(|&n| n > 0);
            s.devices.push(IgclDevice {
                handle: h,
                luid: (luid.HighPart, luid.LowPart),
                name: String::from_utf8_lossy(&p.name[..end]).trim().to_string(),
                vendor_id: p.pci_vendor_id,
                device_id: p.pci_device_id,
                execution_units: eus,
            });
        }
        Ok(s)
    }

    /// Reads instantaneous telemetry for one device.
    pub fn telemetry(&self, dev: &IgclDevice) -> Result<IgclTelemetry, String> {
        // SAFETY: all-zero is valid for this plain-data struct.
        let mut t: CtlPowerTelemetry = unsafe { std::mem::zeroed() };
        t.size = std::mem::size_of::<CtlPowerTelemetry>() as u32;
        t.version = 1;
        // SAFETY: valid device handle and 1024-byte buffer.
        let r = unsafe { (self.telemetry)(dev.handle, &mut t) };
        if r != 0 {
            return Err(format!("ctlPowerTelemetryGet failed (0x{r:08X})"));
        }
        Ok(IgclTelemetry {
            clock_mhz: t.gpu_current_clock_frequency.get(),
            temperature_c: t.gpu_current_temperature.get(),
            voltage_v: t.gpu_voltage.get(),
            global_activity_s: t.global_activity_counter.get(),
            timestamp_s: t.time_stamp.get(),
            power_limited: t.gpu_power_limited != 0,
            temperature_limited: t.gpu_temperature_limited != 0,
        })
    }
}

impl Drop for Igcl {
    fn drop(&mut self) {
        // SAFETY: API handle and library released exactly once.
        unsafe {
            (self.close)(self.api);
            FreeLibrary(self.lib);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// Offsets printed by MSVC from igcl_api.h.
    #[test]
    fn layouts_match_the_c_header() {
        assert_eq!(offset_of!(CtlInitArgs, app_version), 8);
        assert_eq!(offset_of!(CtlInitArgs, application_uid), 20);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, p_device_id), 8);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, driver_version), 32);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, pci_vendor_id), 64);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, num_slices), 84);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, name), 88);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, frequency), 192);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, adapter_bdf), 200);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, num_xe_cores), 204);
        assert_eq!(offset_of!(CtlDeviceAdapterProperties, reserved), 208);
        assert_eq!(offset_of!(CtlTelemetryItem, units), 4);
        assert_eq!(offset_of!(CtlTelemetryItem, value), 16);
        assert_eq!(offset_of!(CtlPowerTelemetry, time_stamp), 8);
        assert_eq!(offset_of!(CtlPowerTelemetry, gpu_voltage), 56);
        assert_eq!(
            offset_of!(CtlPowerTelemetry, gpu_current_clock_frequency),
            80
        );
        assert_eq!(offset_of!(CtlPowerTelemetry, gpu_current_temperature), 104);
        assert_eq!(offset_of!(CtlPowerTelemetry, global_activity_counter), 128);
        assert_eq!(offset_of!(CtlPowerTelemetry, gpu_power_limited), 200);
        assert_eq!(offset_of!(CtlPowerTelemetry, gpu_utilization_limited), 204);
    }

    #[test]
    fn item_decoding() {
        let mut i = CtlTelemetryItem {
            supported: 1,
            _pad0: [0; 3],
            units: 0,
            data_type: CTL_DATA_TYPE_DOUBLE,
            _pad1: 0,
            value: 350.0f64.to_bits(),
        };
        assert_eq!(i.get(), Some(350.0));
        i.data_type = CTL_DATA_TYPE_FLOAT;
        i.value = 1.5f32.to_bits() as u64;
        assert_eq!(i.get(), Some(1.5));
        i.supported = 0;
        assert_eq!(i.get(), None);
    }

    /// Live: on the development machine IGCL lists the Intel UHD Graphics.
    /// On systems without ControlLib.dll the error must explain why.
    #[test]
    fn live_open() {
        match Igcl::open() {
            Ok(igcl) => {
                for d in &igcl.devices {
                    assert_eq!(d.vendor_id, 0x8086);
                    let t = igcl.telemetry(d).expect("telemetry");
                    if let Some(c) = t.clock_mhz {
                        assert!((0.0..5000.0).contains(&c), "{c}");
                    }
                }
            }
            Err(e) => assert!(!e.is_empty()),
        }
    }
}
