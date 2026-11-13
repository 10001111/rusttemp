//! Linux graphics adapter enumeration from `/sys/class/drm` (root-relative,
//! so it can be tested with mock trees and used with `--sysfs-root`).
//!
//! For each `cardN` (not connector entries such as `card0-HDMI-A-1`):
//! - `device/vendor`, `device/device`, `device/subsystem_vendor`,
//!   `device/subsystem_device`, `device/revision` (hex)
//! - `device/uevent`: `PCI_SLOT_NAME=0000:00:02.0`, `DRIVER=i915`
//! - `device/boot_vga`
//! - `device/mem_info_vram_total` (amdgpu, bytes)

use std::fs;
use std::path::Path;

use super::gpu::{PciAddress, RawAdapter};

fn read(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn hex(p: &Path) -> Option<u32> {
    read(p).and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
}

/// Parses a PCI slot name such as `0000:00:02.0`.
pub fn parse_slot(s: &str) -> Option<PciAddress> {
    let (domain, rest) = s.split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (device, function) = rest.split_once('.')?;
    Some(PciAddress {
        domain: u32::from_str_radix(domain, 16).ok()?,
        bus: u32::from_str_radix(bus, 16).ok()?,
        device: u32::from_str_radix(device, 16).ok()?,
        function: u32::from_str_radix(function, 16).ok()?,
    })
}

/// `cardN` directories below `root`, sorted by N (connector entries such as
/// `card0-eDP-1` are skipped).
pub fn card_dirs(root: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(root.join("sys/class/drm")) else {
        return Vec::new();
    };
    let mut cards: Vec<(u32, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix("card")?.parse::<u32>().ok()?;
            Some((n, e.path()))
        })
        .collect();
    cards.sort_by_key(|(n, _)| *n);
    cards.into_iter().map(|(_, p)| p).collect()
}

/// Enumerates DRM cards below `root` (same order as [`card_dirs`]).
pub fn enumerate(root: &Path) -> Vec<RawAdapter> {
    card_dirs(root)
        .into_iter()
        .map(|card| {
            let dev = card.join("device");
            let uevent = read(&dev.join("uevent")).unwrap_or_default();
            let field = |k: &str| {
                uevent
                    .lines()
                    .find_map(|l| l.strip_prefix(k).map(str::to_string))
            };
            RawAdapter {
                name: None,
                vendor_id: hex(&dev.join("vendor")),
                device_id: hex(&dev.join("device")),
                subsys_vendor_id: hex(&dev.join("subsystem_vendor")),
                subsys_id: hex(&dev.join("subsystem_device")),
                revision: hex(&dev.join("revision")),
                pci: field("PCI_SLOT_NAME=").and_then(|s| parse_slot(&s)),
                luid: None,
                flags: None,
                dedicated_memory_bytes: read(&dev.join("mem_info_vram_total"))
                    .and_then(|s| s.parse().ok()),
                shared_memory_bytes: None,
                driver: field("DRIVER="),
                driver_version: None,
                boot_vga: read(&dev.join("boot_vga")).map(|s| s == "1"),
                source: "/sys/class/drm",
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::gpu::{GpuKind, identify};
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn slot_names() {
        let p = parse_slot("0000:01:00.0").unwrap();
        assert_eq!((p.bus, p.device, p.function), (1, 0, 0));
        assert_eq!(parse_slot("0000:c1:00.1").unwrap().bus, 0xc1);
        assert_eq!(parse_slot("garbage"), None);
    }

    #[test]
    fn intel_igpu_plus_amd_card() {
        let t = TempDir::new().unwrap();
        let r = t.path();
        write(r, "sys/class/drm/card0/device/vendor", "0x8086\n");
        write(r, "sys/class/drm/card0/device/device", "0xa7a8\n");
        write(r, "sys/class/drm/card0/device/boot_vga", "1\n");
        write(
            r,
            "sys/class/drm/card0/device/uevent",
            "DRIVER=i915\nPCI_SLOT_NAME=0000:00:02.0\n",
        );
        write(r, "sys/class/drm/card1/device/vendor", "0x1002\n");
        write(r, "sys/class/drm/card1/device/device", "0x73df\n");
        write(
            r,
            "sys/class/drm/card1/device/uevent",
            "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n",
        );
        write(
            r,
            "sys/class/drm/card1/device/mem_info_vram_total",
            "12868124672\n",
        );
        // Connector entries are not cards.
        write(r, "sys/class/drm/card0-eDP-1/status", "connected\n");
        let list = enumerate(r);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].driver.as_deref(), Some("i915"));
        assert_eq!(list[0].boot_vga, Some(true));
        assert_eq!(identify(list[0].clone()).kind, GpuKind::Integrated);
        assert_eq!(list[1].dedicated_memory_bytes, Some(12_868_124_672));
        assert_eq!(list[1].pci.unwrap().bus, 3);
        // AMD card with one supporting signal only: unclassified, not guessed.
        assert_eq!(identify(list[1].clone()).kind, GpuKind::Unknown);
    }

    #[test]
    fn no_drm_directory() {
        let t = TempDir::new().unwrap();
        assert!(enumerate(t.path()).is_empty());
    }
}
