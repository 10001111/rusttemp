//! Windows backend: ACPI thermal zones via PDH performance counters.
//!
//! Counter object "Thermal Zone Information" (the same data WMI exposes as
//! `Win32_PerfFormattedData_Counters_ThermalZoneInformation`):
//! - `High Precision Temperature`: tenths of a kelvin (preferred).
//! - `Temperature`: whole kelvin (fallback for systems without the former).
//!
//! These counters are readable without administrator rights. They report ACPI
//! firmware thermal zones, whose physical location is decided by the firmware
//! and is not guaranteed to be the CPU, so every sensor from this backend is
//! classified as [`SensorKind::AcpiThermalZone`]. Per-core CPU temperatures on
//! Windows require reading model-specific registers through a kernel driver,
//! which RustTemp does not implement.

use std::ptr;

use windows_sys::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_NO_INSTANCE, PDH_CSTATUS_VALID_DATA,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PDH_NO_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};

use crate::errors::{Error, Result};
use crate::sensors::{
    HardwareLimits, Reading, SensorBackend, SensorInfo, SensorKind, decikelvin_to_celsius,
    kelvin_to_celsius,
};

/// `PDH_FMT_NOSCALE` from pdh.h (not exported by windows-sys): return the raw
/// counter value without applying the counter's default scale factor.
const PDH_FMT_NOSCALE: u32 = 0x0000_1000;

const HIGH_PRECISION_COUNTER: &str = r"\Thermal Zone Information(*)\High Precision Temperature";
const KELVIN_COUNTER: &str = r"\Thermal Zone Information(*)\Temperature";

#[derive(Clone, Copy)]
enum Unit {
    DeciKelvin,
    Kelvin,
}

/// RAII wrapper around a PDH query with one wildcard counter.
struct PdhQuery {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    unit: Unit,
    path: &'static str,
}

impl PdhQuery {
    fn open(path: &'static str, unit: Unit) -> std::result::Result<Self, u32> {
        let mut query: PDH_HQUERY = ptr::null_mut();
        // SAFETY: valid out-pointer; null data source means "real-time data".
        let status = unsafe { PdhOpenQueryW(ptr::null(), 0, &mut query) };
        if status != 0 {
            return Err(status);
        }
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut counter: PDH_HCOUNTER = ptr::null_mut();
        // SAFETY: `query` is open, `wide` is NUL-terminated and outlives the call.
        let status = unsafe { PdhAddEnglishCounterW(query, wide.as_ptr(), 0, &mut counter) };
        if status != 0 {
            // SAFETY: query handle obtained above.
            unsafe { PdhCloseQuery(query) };
            return Err(status);
        }
        Ok(PdhQuery {
            query,
            counter,
            unit,
            path,
        })
    }

    /// Collects one sample and returns `(instance name, reading)` pairs.
    fn sample(&self) -> std::result::Result<Vec<(String, Reading)>, u32> {
        // SAFETY: query handle is valid for the lifetime of `self`.
        let status = unsafe { PdhCollectQueryData(self.query) };
        if status == PDH_NO_DATA {
            return Ok(Vec::new());
        }
        if status != 0 {
            return Err(status);
        }

        let format = PDH_FMT_DOUBLE | PDH_FMT_NOSCALE;
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        // SAFETY: a null buffer with size 0 asks PDH for the required size.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                self.counter,
                format,
                &mut size,
                &mut count,
                ptr::null_mut(),
            )
        };
        if status == PDH_NO_DATA || status == PDH_CSTATUS_NO_INSTANCE {
            return Ok(Vec::new());
        }
        if status != PDH_MORE_DATA {
            return if status == 0 {
                Ok(Vec::new())
            } else {
                Err(status)
            };
        }

        // u64 storage guarantees the 8-byte alignment the item structs need.
        let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        // SAFETY: `buf` holds at least `size` bytes, suitably aligned.
        let status = unsafe {
            PdhGetFormattedCounterArrayW(self.counter, format, &mut size, &mut count, items)
        };
        if status != 0 {
            return Err(status);
        }

        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: PDH wrote `count` items at the start of the buffer; names
            // point into the same buffer and are NUL-terminated.
            let item = unsafe { &*items.add(i) };
            let name = unsafe { wide_cstr_to_string(item.szName) };
            if name == "_Total" {
                continue;
            }
            let reading = match item.FmtValue.CStatus {
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA => {
                    // SAFETY: PDH_FMT_DOUBLE selects the `doubleValue` union member.
                    let raw = unsafe { item.FmtValue.Anonymous.doubleValue };
                    Reading::from_celsius(match self.unit {
                        Unit::DeciKelvin => decikelvin_to_celsius(raw),
                        Unit::Kelvin => kelvin_to_celsius(raw),
                    })
                }
                s => Reading::Unavailable(format!("counter status 0x{s:08X}")),
            };
            out.push((name, reading));
        }
        Ok(out)
    }
}

impl Drop for PdhQuery {
    fn drop(&mut self) {
        // SAFETY: query handle is valid and closed exactly once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Reads a NUL-terminated UTF-16 string.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
unsafe fn wide_cstr_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: caller guarantees NUL termination.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` elements were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// ACPI thermal zones reported by Windows performance counters.
pub struct ThermalZoneBackend {
    query: PdhQuery,
    sensors: Vec<SensorInfo>,
}

impl ThermalZoneBackend {
    /// Opens the thermal-zone counters and enumerates the zones.
    pub fn new() -> Result<Self> {
        let query = PdhQuery::open(HIGH_PRECISION_COUNTER, Unit::DeciKelvin)
            .or_else(|_| PdhQuery::open(KELVIN_COUNTER, Unit::Kelvin))
            .map_err(|status| {
                Error::Platform(format!(
                    "Windows thermal zone performance counters are unavailable \
                     (PDH status 0x{status:08X})"
                ))
            })?;
        let zones = query.sample().map_err(pdh_error)?;
        let sensors = zones
            .into_iter()
            .map(|(name, _)| SensorInfo {
                id: format!("acpi-tz:{name}"),
                label: name.clone(),
                chip: "ACPI thermal zone".into(),
                kind: SensorKind::AcpiThermalZone,
                source: query.path.replace('*', &name),
                limits: HardwareLimits::default(),
            })
            .collect();
        Ok(ThermalZoneBackend { query, sensors })
    }
}

fn pdh_error(status: u32) -> Error {
    Error::Platform(format!(
        "reading Windows thermal zone counters failed (PDH status 0x{status:08X})"
    ))
}

impl SensorBackend for ThermalZoneBackend {
    fn name(&self) -> &'static str {
        "windows-acpi-thermal-zone"
    }

    fn sensors(&self) -> &[SensorInfo] {
        &self.sensors
    }

    fn read(&mut self) -> Vec<Reading> {
        match self.query.sample() {
            Ok(samples) => self
                .sensors
                .iter()
                .map(|s| {
                    samples
                        .iter()
                        .find(|(name, _)| *name == s.label)
                        .map(|(_, r)| r.clone())
                        .unwrap_or_else(|| {
                            Reading::Unavailable("thermal zone no longer reported".into())
                        })
                })
                .collect(),
            Err(status) => {
                let msg = format!("counter collection failed (PDH status 0x{status:08X})");
                vec![Reading::Unavailable(msg); self.sensors.len()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the real OS counters. Passes on machines with zero zones too;
    /// it checks the backend contract, not specific hardware.
    #[test]
    fn backend_contract_on_this_machine() {
        let mut b = ThermalZoneBackend::new().expect("PDH thermal zone object should exist");
        let readings = b.read();
        assert_eq!(readings.len(), b.sensors().len());
        for s in b.sensors() {
            assert_eq!(s.kind, SensorKind::AcpiThermalZone);
            assert!(!s.kind.is_cpu());
        }
    }

    #[test]
    fn wide_string_conversion() {
        let w: Vec<u16> = "\\_TZ.THRM\0".encode_utf16().collect();
        assert_eq!(unsafe { wide_cstr_to_string(w.as_ptr()) }, "\\_TZ.THRM");
        assert_eq!(unsafe { wide_cstr_to_string(ptr::null()) }, "");
    }
}
