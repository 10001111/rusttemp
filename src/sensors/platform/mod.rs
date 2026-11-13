//! Platform-specific sensor access.
//!
//! - [`sysfs`]: Linux `/sys/class/hwmon` and `/sys/class/thermal`. Compiled on
//!   every platform because it is plain file reading under a configurable root,
//!   which also makes it testable everywhere with mock trees.
//! - [`intel_dts`]: Intel digital thermal sensor register decoding (pure
//!   arithmetic, tested everywhere).
//! - `pawnio` (Windows): Intel package/core temperatures via the PawnIO driver.
//! - `windows` (Windows): ACPI thermal zones via PDH performance counters.
//! - [`composite`]: combines several backends.

pub mod composite;
pub mod intel_dts;
#[cfg(windows)]
pub mod pawnio;
pub mod sysfs;
#[cfg(windows)]
pub mod windows;

use super::SensorBackend;
use crate::errors::Result;

/// Backend name used on Windows.
pub const WINDOWS_BACKEND: &str = "windows";

/// Creates the default backend for the operating system RustTemp runs on.
pub fn default_backend() -> Result<Box<dyn SensorBackend>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(sysfs::SysfsBackend::new("/")?))
    }
    #[cfg(windows)]
    {
        let mut parts: Vec<Box<dyn SensorBackend>> = Vec::new();
        let mut notes = Vec::new();
        match pawnio::IntelDtsBackend::new() {
            Ok(b) => parts.push(Box::new(b)),
            Err(why) => notes.push(format!("CPU core temperatures unavailable: {why}")),
        }
        match windows::ThermalZoneBackend::new() {
            Ok(b) => parts.push(Box::new(b)),
            Err(e) => notes.push(format!("ACPI thermal zones unavailable: {e}")),
        }
        Ok(Box::new(composite::CompositeBackend::new(
            WINDOWS_BACKEND,
            parts,
            notes,
        )))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(crate::errors::Error::UnsupportedPlatform(
            std::env::consts::OS,
        ))
    }
}
