//! Graphics device identification and integrated / discrete classification.
//!
//! Identification only answers "what devices exist". It does not touch
//! sensors. Classification combines several independent signals and
//! records each one as [`Evidence`]. Vendor, model name or memory size are
//! never enough on their own; ambiguous devices are reported as
//! [`GpuKind::Unknown`].

use std::fmt;

use serde::Serialize;

use super::tables;

/// Adapter type flags reported by Windows (`D3DKMT_ADAPTERTYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct AdapterFlags {
    pub render_supported: bool,
    pub display_supported: bool,
    pub software_device: bool,
    /// The boot display device.
    pub post_device: bool,
    /// Declared by its driver as the discrete half of a hybrid system.
    pub hybrid_discrete: bool,
    /// Declared by its driver as the integrated half of a hybrid system.
    pub hybrid_integrated: bool,
    pub indirect_display_device: bool,
    pub paravirtualized: bool,
    /// Removable (e.g. an external GPU enclosure).
    pub detachable: bool,
    pub compute_only: bool,
}

impl AdapterFlags {
    /// Decodes the `D3DKMT_ADAPTERTYPE` bitfield (d3dkmthk.h bit order).
    pub fn from_d3dkmt(v: u32) -> Self {
        let bit = |n: u32| v & (1 << n) != 0;
        AdapterFlags {
            render_supported: bit(0),
            display_supported: bit(1),
            software_device: bit(2),
            post_device: bit(3),
            hybrid_discrete: bit(4),
            hybrid_integrated: bit(5),
            indirect_display_device: bit(6),
            paravirtualized: bit(7),
            detachable: bit(10),
            compute_only: bit(11),
        }
    }
}

/// PCI location of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PciAddress {
    pub domain: u32,
    pub bus: u32,
    pub device: u32,
    pub function: u32,
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04x}:{:02x}:{:02x}.{}",
            self.domain, self.bus, self.device, self.function
        )
    }
}

/// What the OS reports about one graphics adapter, before classification.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct RawAdapter {
    pub name: Option<String>,
    pub vendor_id: Option<u32>,
    pub device_id: Option<u32>,
    pub subsys_vendor_id: Option<u32>,
    pub subsys_id: Option<u32>,
    pub revision: Option<u32>,
    pub pci: Option<PciAddress>,
    /// Windows adapter LUID as `(high, low)`.
    pub luid: Option<(i32, u32)>,
    pub flags: Option<AdapterFlags>,
    /// Video memory dedicated to the adapter (for an iGPU, a RAM carve-out).
    pub dedicated_memory_bytes: Option<u64>,
    /// System memory the adapter may use.
    pub shared_memory_bytes: Option<u64>,
    /// Kernel driver name (Linux), e.g. `i915`, `amdgpu`.
    pub driver: Option<String>,
    pub driver_version: Option<String>,
    /// Linux `boot_vga`: the device the firmware used for boot output.
    pub boot_vga: Option<bool>,
    /// Where the identification data came from.
    pub source: &'static str,
}

/// Classification result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuKind {
    Integrated,
    Discrete,
    /// Not enough agreeing evidence.
    Unknown,
    /// A software renderer (e.g. Microsoft Basic Render Driver), not a GPU.
    Software,
}

impl GpuKind {
    pub fn describe(self) -> &'static str {
        match self {
            GpuKind::Integrated => "integrated",
            GpuKind::Discrete => "dedicated (discrete)",
            GpuKind::Unknown => "unclassified",
            GpuKind::Software => "software renderer",
        }
    }
}

/// How strongly a signal indicates a classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Supporting,
    Strong,
    Decisive,
}

/// One observation used for classification.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    pub signal: String,
    pub strength: Strength,
    pub indicates: GpuKind,
}

/// A classified graphics device.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GpuDevice {
    #[serde(flatten)]
    pub raw: RawAdapter,
    pub vendor: String,
    pub kind: GpuKind,
    pub evidence: Vec<Evidence>,
    /// GPU architecture from the device-ID table, if known.
    pub architecture: Option<&'static str>,
}

impl GpuDevice {
    /// Display name: adapter string, or vendor + device ID.
    pub fn display_name(&self) -> String {
        match (&self.raw.name, self.raw.device_id) {
            (Some(n), _) => n.clone(),
            (None, Some(d)) => format!("{} device 0x{d:04X}", self.vendor),
            _ => format!("{} graphics device", self.vendor),
        }
    }
}

/// PCI vendor IDs.
pub const VENDOR_INTEL: u32 = 0x8086;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_MICROSOFT: u32 = 0x1414;

/// Vendor name for a PCI vendor ID.
pub fn vendor_name(id: Option<u32>) -> String {
    match id {
        Some(VENDOR_INTEL) => "Intel".into(),
        Some(VENDOR_AMD) | Some(0x1022) => "AMD".into(),
        Some(VENDOR_NVIDIA) => "NVIDIA".into(),
        Some(VENDOR_MICROSOFT) => "Microsoft".into(),
        Some(0x5143) => "Qualcomm".into(),
        Some(0x106B) => "Apple".into(),
        Some(0x15AD) => "VMware".into(),
        Some(0x1AF4) => "Red Hat (virtio)".into(),
        Some(v) => format!("vendor 0x{v:04X}"),
        None => "unknown vendor".into(),
    }
}

const MIB: u64 = 1024 * 1024;

/// Collects evidence and decides the kind.
///
/// Rules (see docs/HARDWARE_MONITOR_PLAN.md §B3):
/// 1. A decisive signal wins (software device / no PCI address on Windows).
/// 2. Otherwise integrated or discrete when at least one strong signal
///    agrees and none contradicts.
/// 3. Without strong signals, two agreeing supporting signals decide.
/// 4. Anything else is Unknown.
pub fn classify(raw: &RawAdapter) -> (GpuKind, Vec<Evidence>) {
    let mut ev = Vec::new();
    let mut add = |signal: String, strength: Strength, indicates: GpuKind| {
        ev.push(Evidence {
            signal,
            strength,
            indicates,
        })
    };

    if let Some(f) = raw.flags {
        if f.software_device {
            add(
                "Windows reports a software device".into(),
                Strength::Decisive,
                GpuKind::Software,
            );
        }
        if f.hybrid_integrated {
            add(
                "Windows reports the driver as the integrated GPU of a hybrid system (HybridIntegrated)"
                    .into(),
                Strength::Strong,
                GpuKind::Integrated,
            );
        }
        if f.hybrid_discrete {
            add(
                "Windows reports the driver as the discrete GPU of a hybrid system (HybridDiscrete)"
                    .into(),
                Strength::Strong,
                GpuKind::Discrete,
            );
        }
        if f.detachable {
            add(
                "Windows reports a detachable adapter (external GPU)".into(),
                Strength::Strong,
                GpuKind::Discrete,
            );
        }
    }
    if raw.vendor_id == Some(VENDOR_MICROSOFT) && raw.pci.is_none() {
        add(
            "Microsoft adapter without a PCI location (Basic Render Driver)".into(),
            Strength::Decisive,
            GpuKind::Software,
        );
    }
    if let Some(p) = raw.pci {
        if raw.vendor_id == Some(VENDOR_INTEL) && p.bus == 0 && p.device == 2 && p.function == 0 {
            add(
                format!("Intel device at PCI {p}, Intel's fixed integrated-graphics location"),
                Strength::Strong,
                GpuKind::Integrated,
            );
        }
    }
    if let (Some(d), Some(s)) = (raw.dedicated_memory_bytes, raw.shared_memory_bytes) {
        if d <= 512 * MIB && s >= d {
            add(
                format!(
                    "{} MiB dedicated memory with {} MiB shared system memory",
                    d / MIB,
                    s / MIB
                ),
                Strength::Supporting,
                GpuKind::Integrated,
            );
        }
    }
    if cfg!(any(target_arch = "x86_64", target_arch = "x86"))
        && raw.vendor_id == Some(VENDOR_NVIDIA)
        && raw.pci.is_some()
    {
        add(
            "NVIDIA GPU on an x86 PC (NVIDIA makes no current x86 integrated GPUs)".into(),
            Strength::Supporting,
            GpuKind::Discrete,
        );
    }
    if let (Some(p), Some(d)) = (raw.pci, raw.dedicated_memory_bytes) {
        if p.bus != 0 && d >= 2048 * MIB {
            add(
                format!("on PCI bus {} with {} MiB dedicated memory", p.bus, d / MIB),
                Strength::Supporting,
                GpuKind::Discrete,
            );
        }
    }

    let kind = decide(&ev);
    (kind, ev)
}

fn decide(ev: &[Evidence]) -> GpuKind {
    if let Some(e) = ev.iter().find(|e| e.strength == Strength::Decisive) {
        return e.indicates;
    }
    let count = |k: GpuKind, s: Strength| {
        ev.iter()
            .filter(|e| e.indicates == k && e.strength == s)
            .count()
    };
    let strong_i = count(GpuKind::Integrated, Strength::Strong);
    let strong_d = count(GpuKind::Discrete, Strength::Strong);
    match (strong_i > 0, strong_d > 0) {
        (true, false) => return GpuKind::Integrated,
        (false, true) => return GpuKind::Discrete,
        (true, true) => return GpuKind::Unknown,
        (false, false) => {}
    }
    let sup_i = count(GpuKind::Integrated, Strength::Supporting);
    let sup_d = count(GpuKind::Discrete, Strength::Supporting);
    if sup_i >= 2 && sup_d == 0 {
        GpuKind::Integrated
    } else if sup_d >= 2 && sup_i == 0 {
        GpuKind::Discrete
    } else {
        GpuKind::Unknown
    }
}

/// Classifies a raw adapter into a [`GpuDevice`].
pub fn identify(raw: RawAdapter) -> GpuDevice {
    let (kind, evidence) = classify(&raw);
    GpuDevice {
        vendor: vendor_name(raw.vendor_id),
        architecture: raw
            .vendor_id
            .zip(raw.device_id)
            .and_then(|(v, d)| tables::gpu_architecture(v, d)),
        raw,
        kind,
        evidence,
    }
}

/// Re-identifies a device after a vendor library reported its dedicated
/// memory size (e.g. NVML on Linux, where sysfs does not report VRAM).
pub fn with_dedicated_memory(dev: &GpuDevice, bytes: u64) -> GpuDevice {
    let mut raw = dev.raw.clone();
    raw.dedicated_memory_bytes = Some(bytes);
    identify(raw)
}

/// Parses a Windows adapter LUID from a performance-counter instance name
/// such as `pid_10336_luid_0x00000000_0x0001087d_phys_0_eng_0_engtype_3d`.
pub fn parse_luid(instance: &str) -> Option<(i32, u32)> {
    let rest = &instance[instance.find("luid_0x")? + 7..];
    let (high, rest) = rest.split_once("_0x")?;
    let low = rest.get(..8)?;
    Some((
        u32::from_str_radix(high, 16).ok()? as i32,
        u32::from_str_radix(low, 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pci(bus: u32, device: u32, function: u32) -> Option<PciAddress> {
        Some(PciAddress {
            domain: 0,
            bus,
            device,
            function,
        })
    }

    /// Intel UHD Graphics exactly as Probe A reported it on the i7-13620H.
    fn intel_uhd() -> RawAdapter {
        RawAdapter {
            name: Some("Intel(R) UHD Graphics".into()),
            vendor_id: Some(0x8086),
            device_id: Some(0xA7A8),
            pci: pci(0, 2, 0),
            luid: Some((0, 0x1087d)),
            flags: Some(AdapterFlags::from_d3dkmt(0x0000_232b)),
            dedicated_memory_bytes: Some(128 * MIB),
            shared_memory_bytes: Some(8004 * MIB),
            ..Default::default()
        }
    }

    /// Microsoft Basic Render Driver as Probe A reported it.
    fn basic_render() -> RawAdapter {
        RawAdapter {
            vendor_id: Some(0x1414),
            device_id: Some(0x008C),
            pci: None,
            flags: Some(AdapterFlags::from_d3dkmt(0x0000_0105)),
            dedicated_memory_bytes: Some(0),
            shared_memory_bytes: Some(8004 * MIB),
            ..Default::default()
        }
    }

    #[test]
    fn d3dkmt_flags() {
        let f = AdapterFlags::from_d3dkmt(0x0000_232b);
        assert!(f.render_supported && f.display_supported && f.post_device && f.hybrid_integrated);
        assert!(!f.software_device && !f.hybrid_discrete && !f.detachable);
        let s = AdapterFlags::from_d3dkmt(0x0000_0105);
        assert!(s.software_device && s.render_supported && !s.display_supported);
    }

    #[test]
    fn probe_a_devices() {
        let d = identify(intel_uhd());
        assert_eq!(d.kind, GpuKind::Integrated);
        assert_eq!(d.vendor, "Intel");
        assert!(d.evidence.len() >= 3, "{:?}", d.evidence);
        assert!(d.architecture.is_some());
        assert_eq!(identify(basic_render()).kind, GpuKind::Software);
    }

    #[test]
    fn hybrid_laptop_with_nvidia() {
        let nv = RawAdapter {
            vendor_id: Some(VENDOR_NVIDIA),
            device_id: Some(0x28E0),
            pci: pci(1, 0, 0),
            flags: Some(AdapterFlags {
                hybrid_discrete: true,
                render_supported: true,
                ..Default::default()
            }),
            dedicated_memory_bytes: Some(6144 * MIB),
            shared_memory_bytes: Some(8000 * MIB),
            ..Default::default()
        };
        assert_eq!(classify(&nv).0, GpuKind::Discrete);
        assert_eq!(classify(&intel_uhd()).0, GpuKind::Integrated);
    }

    #[test]
    fn desktop_dgpu_without_hybrid_flags() {
        // Desktop AMD card: no hybrid flags, bus 3, 8 GiB.
        let amd = RawAdapter {
            vendor_id: Some(VENDOR_AMD),
            pci: pci(3, 0, 0),
            flags: Some(AdapterFlags {
                render_supported: true,
                display_supported: true,
                post_device: true,
                ..Default::default()
            }),
            dedicated_memory_bytes: Some(8192 * MIB),
            shared_memory_bytes: Some(16000 * MIB),
            ..Default::default()
        };
        // Only one supporting signal (AMD also makes integrated GPUs): not
        // enough to claim "discrete".
        let (kind, ev) = classify(&amd);
        assert_eq!(kind, GpuKind::Unknown, "{ev:?}");
        // Desktop NVIDIA card: location/memory + "NVIDIA on x86" agree.
        let nv = RawAdapter {
            vendor_id: Some(VENDOR_NVIDIA),
            ..amd.clone()
        };
        #[cfg(target_arch = "x86_64")]
        assert_eq!(classify(&nv).0, GpuKind::Discrete);
        // NVIDIA vendor alone (no location/memory evidence) is not enough.
        let nv_bare = RawAdapter {
            vendor_id: Some(VENDOR_NVIDIA),
            pci: pci(0, 0, 0),
            ..Default::default()
        };
        assert_eq!(classify(&nv_bare).0, GpuKind::Unknown);
    }

    #[test]
    fn external_gpu_is_discrete() {
        let egpu = RawAdapter {
            vendor_id: Some(VENDOR_NVIDIA),
            pci: pci(60, 0, 0),
            flags: Some(AdapterFlags {
                detachable: true,
                ..Default::default()
            }),
            dedicated_memory_bytes: Some(8192 * MIB),
            ..Default::default()
        };
        assert_eq!(classify(&egpu).0, GpuKind::Discrete);
    }

    #[test]
    fn ambiguous_apu_is_unknown_not_guessed() {
        // Desktop AMD APU, no hybrid flags, large BIOS carve-out on bus 5.
        let apu = RawAdapter {
            vendor_id: Some(VENDOR_AMD),
            pci: pci(5, 0, 0),
            flags: Some(AdapterFlags::default()),
            dedicated_memory_bytes: Some(4096 * MIB),
            shared_memory_bytes: Some(16000 * MIB),
            ..Default::default()
        };
        assert_eq!(classify(&apu).0, GpuKind::Unknown);
        // Vendor and memory alone never decide.
        let bare = RawAdapter {
            vendor_id: Some(VENDOR_INTEL),
            dedicated_memory_bytes: Some(128 * MIB),
            shared_memory_bytes: Some(8000 * MIB),
            ..Default::default()
        };
        assert_eq!(classify(&bare).0, GpuKind::Unknown);
    }

    #[test]
    fn conflicting_strong_signals_are_unknown() {
        let mut odd = intel_uhd();
        odd.flags.as_mut().unwrap().hybrid_discrete = true;
        assert_eq!(classify(&odd).0, GpuKind::Unknown);
    }

    #[test]
    fn luid_from_counter_instances() {
        assert_eq!(
            parse_luid("pid_10336_luid_0x00000000_0x0001087d_phys_0_eng_0_engtype_3d"),
            Some((0, 0x1087d))
        );
        assert_eq!(
            parse_luid("luid_0x00000000_0x00010C2A_phys_0"),
            Some((0, 0x10c2a))
        );
        assert_eq!(parse_luid("_Total"), None);
    }

    #[test]
    fn vendor_memory_enrichment_can_confirm_discrete() {
        // Linux NVIDIA card: no VRAM in sysfs, so one supporting signal only.
        let raw = RawAdapter {
            vendor_id: Some(VENDOR_NVIDIA),
            pci: pci(1, 0, 0),
            driver: Some("nvidia".into()),
            ..Default::default()
        };
        let d = identify(raw);
        assert_eq!(d.kind, GpuKind::Unknown);
        #[cfg(target_arch = "x86_64")]
        assert_eq!(with_dedicated_memory(&d, 8 << 30).kind, GpuKind::Discrete);
    }

    #[test]
    fn pci_display() {
        assert_eq!(pci(0, 2, 0).unwrap().to_string(), "0000:00:02.0");
        assert_eq!(vendor_name(Some(0x10DE)), "NVIDIA");
        assert_eq!(vendor_name(Some(0xABCD)), "vendor 0xABCD");
    }
}
