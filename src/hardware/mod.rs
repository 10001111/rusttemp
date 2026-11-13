//! Hardware identification: what devices exist (separate from what their
//! sensors can measure).

pub mod gpu;
pub mod linux;
pub mod tables;
#[cfg(windows)]
pub mod windows;

use std::path::Path;

use gpu::GpuDevice;

/// Enumerates and classifies graphics devices. With `sysfs_root`, only the
/// Linux-style DRM tree under that root is used (diagnostics / tests).
pub fn enumerate_gpus(sysfs_root: Option<&Path>) -> Result<Vec<GpuDevice>, String> {
    if let Some(root) = sysfs_root {
        return Ok(linux::enumerate(root)
            .into_iter()
            .map(gpu::identify)
            .collect());
    }
    #[cfg(windows)]
    {
        windows::enumerate().map(|l| l.into_iter().map(gpu::identify).collect())
    }
    #[cfg(target_os = "linux")]
    {
        Ok(linux::enumerate(Path::new("/"))
            .into_iter()
            .map(gpu::identify)
            .collect())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(format!(
            "graphics device enumeration is not implemented on {}",
            std::env::consts::OS
        ))
    }
}
