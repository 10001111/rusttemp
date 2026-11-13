# Vendored PawnIO module

`IntelMSR.bin` is the official, signed PawnIO module that RustTemp loads into
the PawnIO driver to read Intel thermal MSRs on Windows. It is embedded in the
Windows binary with `include_bytes!`.

| Item | Value |
|---|---|
| Source | <https://github.com/namazso/PawnIO.Modules>, release `0.2.11` (2026-08-30), asset `release_0_2_11.zip` |
| Release zip SHA-256 | `43608cb89bc84247fef1368a139013f7d043e17db6d6c8dfc9b46bf0905a81f4` (matches the digest GitHub publishes) |
| `IntelMSR.bin` SHA-256 | `d6ed85d65ab17a22f813ef98207d6d537155ee2ded5976a21cb48413c9b92e5f` |
| License | LGPL-2.1-or-later (see `COPYING`, copied from the release) |
| Module source | `IntelMSR.p` in the same repository |

What the module allows:
- **Reads:** only a whitelist of MSRs. The list includes `IA32_THERM_STATUS`
  (0x19C), `IA32_PACKAGE_THERM_STATUS` (0x1B1) and `MSR_TEMPERATURE_TARGET`
  (0x1A2), which are the only registers RustTemp reads.
- **Writes:** the module permits a few, but RustTemp never calls its write
  function.

To update the module:
1. Download a newer release.
2. Verify the zip against the digest on the release page.
3. Replace `IntelMSR.bin` and `COPYING`.
4. Update the hashes here and in `src/sensors/platform/pawnio.rs`.
