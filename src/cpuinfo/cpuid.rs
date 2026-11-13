//! CPUID-based identification (x86-64).
//!
//! Reference: Intel SDM Vol. 2A "CPUID"; AMD APM Vol. 3.
//! - Leaf 0: vendor id (EBX, EDX, ECX).
//! - Leaf 1 EAX: stepping[3:0], model[7:4], family[11:8], extended
//!   model[19:16], extended family[27:20]. Display family adds the extended
//!   family when family = 0xF; display model prepends the extended model when
//!   family is 0x6 or 0xF.
//! - Leaves 0x80000002..4: 48-byte brand string.
//! - Leaf 0x16 (Intel): base and maximum frequency in MHz (0 = not reported).

use super::{CpuInfo, tidy_brand, vendor_name};

/// Decoded family / model / stepping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
}

/// Decodes CPUID leaf 1 EAX.
pub fn decode_signature(eax: u32) -> Signature {
    let stepping = eax & 0xF;
    let base_model = (eax >> 4) & 0xF;
    let base_family = (eax >> 8) & 0xF;
    let ext_model = (eax >> 16) & 0xF;
    let ext_family = (eax >> 20) & 0xFF;
    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    let model = if base_family == 0x6 || base_family == 0xF {
        (ext_model << 4) | base_model
    } else {
        base_model
    };
    Signature {
        family,
        model,
        stepping,
    }
}

/// Assembles the vendor id from leaf 0 registers.
pub fn decode_vendor(ebx: u32, edx: u32, ecx: u32) -> String {
    let mut b = Vec::with_capacity(12);
    for r in [ebx, edx, ecx] {
        b.extend_from_slice(&r.to_le_bytes());
    }
    String::from_utf8_lossy(&b).into_owned()
}

/// Assembles the brand string from leaves 0x80000002..4 (EAX, EBX, ECX, EDX each).
pub fn decode_brand(regs: &[[u32; 4]; 3]) -> Option<String> {
    let mut b = Vec::with_capacity(48);
    for leaf in regs {
        for r in leaf {
            b.extend_from_slice(&r.to_le_bytes());
        }
    }
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let s = tidy_brand(&String::from_utf8_lossy(&b[..end]));
    (!s.is_empty()).then_some(s)
}

/// Reads identity from CPUID on this machine.
#[cfg(target_arch = "x86_64")]
pub fn detect() -> CpuInfo {
    use std::arch::x86_64::__cpuid;
    #[allow(unused_unsafe)] // `__cpuid` is safe on newer toolchains.
    let cpuid = |leaf: u32| unsafe { __cpuid(leaf) };

    let l0 = cpuid(0);
    let vendor_id = decode_vendor(l0.ebx, l0.edx, l0.ecx);
    let mut info = CpuInfo {
        vendor: Some(vendor_name(&vendor_id)),
        vendor_id: Some(vendor_id),
        sources: vec!["cpuid"],
        ..Default::default()
    };
    if l0.eax >= 1 {
        let sig = decode_signature(cpuid(1).eax);
        info.family = Some(sig.family);
        info.model = Some(sig.model);
        info.stepping = Some(sig.stepping);
    }
    if l0.eax >= 0x16 {
        let f = cpuid(0x16);
        info.base_mhz = (f.eax & 0xFFFF != 0).then_some(f.eax & 0xFFFF);
        info.max_mhz = (f.ebx & 0xFFFF != 0).then_some(f.ebx & 0xFFFF);
        info.base_mhz_source = info.base_mhz.map(|_| "CPUID leaf 0x16");
        info.max_mhz_source = info.max_mhz.map(|_| "CPUID leaf 0x16");
    }
    if cpuid(0x8000_0000).eax >= 0x8000_0004 {
        let r = |leaf| {
            let x = cpuid(leaf);
            [x.eax, x.ebx, x.ecx, x.edx]
        };
        info.brand = decode_brand(&[r(0x8000_0002), r(0x8000_0003), r(0x8000_0004)]);
    }
    info
}

/// CPUID is unavailable on this architecture.
#[cfg(not(target_arch = "x86_64"))]
pub fn detect() -> CpuInfo {
    CpuInfo::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_i7_13620h() {
        // Windows reports ProcessorId BFEBFBFF000B06A2 => leaf1 EAX 0x000B06A2,
        // "Family 6 Model 186 Stepping 2".
        assert_eq!(
            decode_signature(0x000B_06A2),
            Signature {
                family: 6,
                model: 186,
                stepping: 2
            }
        );
    }

    #[test]
    fn signature_amd_zen() {
        // Ryzen 7 5800X: EAX 0x00A20F12 => family 0x19 (25), model 0x21 (33), stepping 2.
        assert_eq!(
            decode_signature(0x00A2_0F12),
            Signature {
                family: 25,
                model: 33,
                stepping: 2
            }
        );
    }

    #[test]
    fn signature_old_family_ignores_extended_model() {
        // Family 5 (Pentium): extended model bits must not be applied.
        assert_eq!(decode_signature(0x0001_0543).model, 4);
    }

    #[test]
    fn vendor_and_brand() {
        // "GenuineIntel" = EBX "Genu", EDX "ineI", ECX "ntel".
        assert_eq!(
            decode_vendor(0x756E_6547, 0x4965_6E69, 0x6C65_746E),
            "GenuineIntel"
        );
        let text = b"  13th Gen Intel(R) Core(TM) i7-13620H\0\0\0\0\0\0\0\0\0\0";
        let mut regs = [[0u32; 4]; 3];
        for (i, chunk) in text.chunks(4).enumerate() {
            regs[i / 4][i % 4] = u32::from_le_bytes(chunk.try_into().unwrap());
        }
        assert_eq!(
            decode_brand(&regs).as_deref(),
            Some("13th Gen Intel(R) Core(TM) i7-13620H")
        );
        assert_eq!(decode_brand(&[[0; 4]; 3]), None);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn live_cpuid_has_vendor_and_signature() {
        let i = detect();
        assert!(i.vendor_id.as_deref().is_some_and(|v| v.len() == 12));
        assert!(i.family.is_some());
    }
}
