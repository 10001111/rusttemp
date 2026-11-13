//! Microarchitecture names from CPUID family/model.
//!
//! Sources:
//! - Intel: Linux kernel `arch/x86/include/asm/intel-family.h` (family 6
//!   model numbers).
//! - AMD: Linux kernel `arch/x86/kernel/cpu/amd.c` (Zen generation model
//!   ranges).
//!
//! Only models listed here get a name. Anything else is reported as
//! unavailable rather than guessed. Where one model number covers several
//! product generations (e.g. Intel 0x8E / 0x9E), the name says so.

/// Instruction-set architecture of the CPU RustTemp runs on.
pub fn instruction_set() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86-64"
    } else if cfg!(target_arch = "aarch64") {
        "ARM64 (AArch64)"
    } else if cfg!(target_arch = "x86") {
        "x86 (32-bit)"
    } else {
        std::env::consts::ARCH
    }
}

/// Microarchitecture name for a CPU, if the model is known.
pub fn microarchitecture(vendor_id: &str, family: u32, model: u32) -> Option<&'static str> {
    match vendor_id {
        "GenuineIntel" if family == 6 => intel_family6(model),
        "AuthenticAMD" | "HygonGenuine" => amd(family, model),
        _ => None,
    }
}

fn intel_family6(model: u32) -> Option<&'static str> {
    Some(match model {
        0x2A | 0x2D => "Sandy Bridge",
        0x3A | 0x3E => "Ivy Bridge",
        0x3C | 0x3F | 0x45 | 0x46 => "Haswell",
        0x3D | 0x47 | 0x4F | 0x56 => "Broadwell",
        0x4E | 0x5E => "Skylake",
        0x55 => "Skylake-SP / Cascade Lake / Cooper Lake",
        0x8E | 0x9E => "Kaby Lake family (Kaby / Coffee / Whiskey / Amber Lake)",
        0xA5 | 0xA6 => "Comet Lake",
        0x7D | 0x7E => "Ice Lake",
        0x6A | 0x6C => "Ice Lake-SP",
        0xA7 => "Rocket Lake",
        0x8C | 0x8D => "Tiger Lake",
        0x97 | 0x9A => "Alder Lake",
        0xBE => "Alder Lake-N",
        0xB7 | 0xBA | 0xBF => "Raptor Lake",
        0xAA | 0xAC => "Meteor Lake",
        0xBD => "Lunar Lake",
        0xB5 | 0xC5 | 0xC6 => "Arrow Lake",
        0x8F => "Sapphire Rapids",
        0xCF => "Emerald Rapids",
        _ => return None,
    })
}

fn amd(family: u32, model: u32) -> Option<&'static str> {
    Some(match (family, model) {
        (0x15, _) => "Bulldozer family",
        (0x16, _) => "Jaguar / Puma",
        (0x17, 0x00..=0x2F | 0x50..=0x5F) => "Zen / Zen+",
        (0x17, 0x30..=0x4F | 0x60..=0x7F | 0x90..=0x91 | 0xA0..=0xAF) => "Zen 2",
        (0x19, 0x00..=0x0F | 0x20..=0x5F) => "Zen 3",
        (0x19, 0x10..=0x1F | 0x60..=0xAF) => "Zen 4",
        (0x1A, 0x00..=0x2F | 0x40..=0x4F | 0x60..=0x7F) => "Zen 5",
        _ => return None,
    })
}

/// GPU architecture for a PCI vendor/device ID, if the device is known.
///
/// Source: Linux kernel `include/drm/intel/i915_pciids.h` (Intel device
/// ID groups). Only device IDs listed here get a name.
pub fn gpu_architecture(vendor: u32, device: u32) -> Option<&'static str> {
    match (vendor, device) {
        // INTEL_RPLP_IDS: Raptor Lake-P integrated graphics.
        (
            0x8086,
            0xA720 | 0xA721 | 0xA7A0 | 0xA7A1 | 0xA7A8 | 0xA7A9 | 0xA7AA | 0xA7AB | 0xA7AC | 0xA7AD,
        ) => Some("Xe-LP (Gen12, Raptor Lake-P graphics)"),
        // INTEL_RPLS_IDS: Raptor Lake-S integrated graphics.
        (0x8086, 0xA780..=0xA783 | 0xA788 | 0xA789 | 0xA78A | 0xA78B) => {
            Some("Xe-LP (Gen12, Raptor Lake-S graphics)")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn gpu_architecture_table() {
        // The development machine's Intel UHD Graphics.
        assert_eq!(
            super::gpu_architecture(0x8086, 0xA7A8),
            Some("Xe-LP (Gen12, Raptor Lake-P graphics)")
        );
        assert_eq!(super::gpu_architecture(0x8086, 0x1234), None);
        assert_eq!(super::gpu_architecture(0x10DE, 0xA7A8), None);
    }

    use super::*;

    #[test]
    fn known_intel_models() {
        // The development machine: i7-13620H, family 6 model 186 (0xBA).
        assert_eq!(
            microarchitecture("GenuineIntel", 6, 186),
            Some("Raptor Lake")
        );
        assert_eq!(
            microarchitecture("GenuineIntel", 6, 0x9A),
            Some("Alder Lake")
        );
        assert_eq!(
            microarchitecture("GenuineIntel", 6, 0x8C),
            Some("Tiger Lake")
        );
        assert!(
            microarchitecture("GenuineIntel", 6, 0x9E)
                .unwrap()
                .contains("Coffee")
        );
    }

    #[test]
    fn known_amd_models() {
        assert_eq!(microarchitecture("AuthenticAMD", 0x19, 0x21), Some("Zen 3")); // Ryzen 5000 desktop
        assert_eq!(microarchitecture("AuthenticAMD", 0x19, 0x61), Some("Zen 4")); // Ryzen 7000 desktop
        assert_eq!(microarchitecture("AuthenticAMD", 0x17, 0x71), Some("Zen 2")); // Ryzen 3000 desktop
        assert_eq!(
            microarchitecture("AuthenticAMD", 0x17, 0x08),
            Some("Zen / Zen+")
        );
        assert_eq!(microarchitecture("AuthenticAMD", 0x1A, 0x44), Some("Zen 5"));
    }

    #[test]
    fn unknown_models_are_not_guessed() {
        assert_eq!(microarchitecture("GenuineIntel", 6, 0xFF), None);
        assert_eq!(microarchitecture("GenuineIntel", 15, 0x02), None);
        assert_eq!(microarchitecture("AuthenticAMD", 0x19, 0xF0), None);
        assert_eq!(microarchitecture("SomeVendor", 6, 0xBA), None);
    }

    #[test]
    fn instruction_set_is_named() {
        assert!(!instruction_set().is_empty());
        #[cfg(target_arch = "x86_64")]
        assert_eq!(instruction_set(), "x86-64");
    }
}
