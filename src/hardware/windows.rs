//! Windows graphics adapter enumeration.
//!
//! - `D3DKMTEnumAdapters2` / `D3DKMTQueryAdapterInfo` (gdi32, no admin):
//!   LUID, adapter type flags, PCI address, vendor/device IDs, memory segment
//!   sizes, adapter name.
//! - SetupAPI (display device class): driver version, matched by PCI
//!   location.
//!
//! Each query may fail independently; missing data stays `None`.

use std::ffi::c_void;
use std::ptr;

use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTERADDRESS, D3DKMT_ADAPTERINFO, D3DKMT_ADAPTERREGISTRYINFO, D3DKMT_ADAPTERTYPE,
    D3DKMT_CLOSEADAPTER, D3DKMT_ENUMADAPTERS2, D3DKMT_QUERY_DEVICE_IDS, D3DKMT_QUERYADAPTERINFO,
    D3DKMT_SEGMENTSIZEINFO, D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo,
    KMTQAITYPE_ADAPTERADDRESS, KMTQAITYPE_ADAPTERREGISTRYINFO, KMTQAITYPE_ADAPTERTYPE,
    KMTQAITYPE_GETSEGMENTSIZE, KMTQAITYPE_PHYSICALADAPTERDEVICEIDS, KMTQUERYADAPTERINFOTYPE,
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DICS_FLAG_GLOBAL, DIGCF_PRESENT, DIREG_DRV, GUID_DEVCLASS_DISPLAY, SP_DEVINFO_DATA,
    SPDRP_LOCATION_INFORMATION, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
    SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDevRegKey,
};
use windows_sys::Win32::System::Registry::{KEY_READ, RegCloseKey, RegQueryValueExW};

use super::gpu::{AdapterFlags, PciAddress, RawAdapter};

/// Queries one adapter property into a default-initialized `T`.
fn query<T: Default>(h: u32, ty: KMTQUERYADAPTERINFOTYPE) -> Option<T> {
    let mut out = T::default();
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: h,
        Type: ty,
        pPrivateDriverData: (&mut out as *mut T).cast::<c_void>(),
        PrivateDriverDataSize: std::mem::size_of::<T>() as u32,
    };
    // SAFETY: the buffer is a properly sized, writable `T`.
    (unsafe { D3DKMTQueryAdapterInfo(&mut q) } == 0).then_some(out)
}

fn wide_to_string(s: &[u16]) -> Option<String> {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    let t = String::from_utf16_lossy(&s[..end]).trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// Enumerates all graphics adapters known to the Windows graphics kernel.
pub fn enumerate() -> Result<Vec<RawAdapter>, String> {
    let mut e = D3DKMT_ENUMADAPTERS2::default();
    // SAFETY: first call with a null array returns the adapter count.
    let s = unsafe { D3DKMTEnumAdapters2(&mut e) };
    if s != 0 {
        return Err(format!(
            "D3DKMTEnumAdapters2 failed (NTSTATUS 0x{:08X})",
            s as u32
        ));
    }
    let mut list = vec![D3DKMT_ADAPTERINFO::default(); e.NumAdapters as usize];
    e.pAdapters = list.as_mut_ptr();
    // SAFETY: `list` holds `NumAdapters` entries.
    let s = unsafe { D3DKMTEnumAdapters2(&mut e) };
    if s != 0 {
        return Err(format!(
            "D3DKMTEnumAdapters2 failed (NTSTATUS 0x{:08X})",
            s as u32
        ));
    }
    list.truncate(e.NumAdapters as usize);
    let versions = driver_versions();

    let mut out = Vec::with_capacity(list.len());
    for a in &list {
        let h = a.hAdapter;
        let flags = query::<D3DKMT_ADAPTERTYPE>(h, KMTQAITYPE_ADAPTERTYPE)
            // SAFETY: `Value` is the plain u32 view of the bitfield union.
            .map(|t| AdapterFlags::from_d3dkmt(unsafe { t.Anonymous.Value }));
        let pci = query::<D3DKMT_ADAPTERADDRESS>(h, KMTQAITYPE_ADAPTERADDRESS)
            .filter(|p| p.BusNumber != u32::MAX)
            .map(|p| PciAddress {
                domain: 0,
                bus: p.BusNumber,
                device: p.DeviceNumber,
                function: p.FunctionNumber,
            });
        let ids = query::<D3DKMT_QUERY_DEVICE_IDS>(h, KMTQAITYPE_PHYSICALADAPTERDEVICEIDS)
            .map(|q| q.DeviceIds);
        let seg = query::<D3DKMT_SEGMENTSIZEINFO>(h, KMTQAITYPE_GETSEGMENTSIZE);
        let reg = query::<D3DKMT_ADAPTERREGISTRYINFO>(h, KMTQAITYPE_ADAPTERREGISTRYINFO);
        let name = reg.as_ref().and_then(|r| wide_to_string(&r.AdapterString));

        out.push(RawAdapter {
            name: name.or_else(|| {
                (ids.map(|d| d.VendorID) == Some(0x1414))
                    .then(|| "Microsoft Basic Render Driver".into())
            }),
            vendor_id: ids.map(|d| d.VendorID),
            device_id: ids.map(|d| d.DeviceID),
            subsys_vendor_id: ids.map(|d| d.SubVendorID),
            subsys_id: ids.map(|d| d.SubSystemID),
            revision: ids.map(|d| d.RevisionID),
            luid: Some((a.AdapterLuid.HighPart, a.AdapterLuid.LowPart)),
            flags,
            dedicated_memory_bytes: seg.map(|s| s.DedicatedVideoMemorySize),
            shared_memory_bytes: seg.map(|s| s.SharedSystemMemorySize),
            driver_version: pci.and_then(|p| {
                versions
                    .iter()
                    .find(|(loc, _)| *loc == (p.bus, p.device, p.function))
                    .map(|(_, v)| v.clone())
            }),
            pci,
            driver: None,
            boot_vga: None,
            source: "Windows graphics kernel (D3DKMT)",
        });

        let close = D3DKMT_CLOSEADAPTER { hAdapter: h };
        // SAFETY: handle returned by D3DKMTEnumAdapters2, closed once.
        unsafe { D3DKMTCloseAdapter(&close) };
    }
    Ok(out)
}

/// Parses SetupAPI's location string, e.g. "PCI bus 1, device 0, function 0".
pub fn parse_location(s: &str) -> Option<(u32, u32, u32)> {
    let nums: Vec<u32> = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    (s.starts_with("PCI bus") && nums.len() == 3).then(|| (nums[0], nums[1], nums[2]))
}

/// `(bus, device, function) → driver version` for display-class devices.
fn driver_versions() -> Vec<((u32, u32, u32), String)> {
    let mut out = Vec::new();
    // SAFETY: standard SetupAPI enumeration; the list is destroyed below.
    let set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVCLASS_DISPLAY,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT,
        )
    };
    if set == -1 {
        return out;
    }
    for i in 0.. {
        let mut info = SP_DEVINFO_DATA {
            cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: valid set and correctly sized struct.
        if unsafe { SetupDiEnumDeviceInfo(set, i, &mut info) } == 0 {
            break;
        }
        let mut buf = [0u16; 256];
        let mut size = 0u32;
        // SAFETY: buffer and its byte length are passed together.
        let ok = unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                set,
                &info,
                SPDRP_LOCATION_INFORMATION,
                ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                (buf.len() * 2) as u32,
                &mut size,
            )
        };
        let Some(loc) = (ok != 0)
            .then(|| wide_to_string(&buf))
            .flatten()
            .and_then(|s| parse_location(&s))
        else {
            continue;
        };
        // SAFETY: opens the device's driver key read-only; closed below.
        let key =
            unsafe { SetupDiOpenDevRegKey(set, &info, DICS_FLAG_GLOBAL, 0, DIREG_DRV, KEY_READ) };
        if key.is_null() || key as isize == -1 {
            continue;
        }
        let name: Vec<u16> = "DriverVersion".encode_utf16().chain([0]).collect();
        let mut val = [0u16; 128];
        let mut len = (val.len() * 2) as u32;
        // SAFETY: buffer and byte length passed together; key closed after.
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                val.as_mut_ptr().cast(),
                &mut len,
            )
        };
        // SAFETY: key opened above.
        unsafe { RegCloseKey(key) };
        if status == 0 {
            if let Some(v) = wide_to_string(&val) {
                out.push((loc, v));
            }
        }
    }
    // SAFETY: set created above.
    unsafe { SetupDiDestroyDeviceInfoList(set) };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_strings() {
        assert_eq!(
            parse_location("PCI bus 0, device 2, function 0"),
            Some((0, 2, 0))
        );
        assert_eq!(
            parse_location("PCI bus 1, device 0, function 0"),
            Some((1, 0, 0))
        );
        assert_eq!(parse_location("@System32\\drivers\\pci.sys"), None);
        assert_eq!(parse_location("PCI bus 1"), None);
    }

    /// Live: every adapter has a LUID; the software renderer is present.
    #[test]
    fn live_enumeration() {
        let list = enumerate().expect("D3DKMT enumeration");
        assert!(!list.is_empty());
        assert!(list.iter().all(|a| a.luid.is_some()));
        assert!(list.iter().any(|a| a.vendor_id == Some(0x1414)));
    }
}
