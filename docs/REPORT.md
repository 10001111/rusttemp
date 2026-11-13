# RustTemp: Implementation Report

## Revision 4: CPU speed, load and power (2026-11-13)

**Added**: a **CPU SPEED & POWER** section in `--info`, `--once` and the
monitor, plus `perf` / `perf_session` in JSON. It contains:
- package and core power
- PL1/PL2/TDP
- average, base and max-turbo clock
- total load
- per-core speed and load table
- monitor session statistics: average and max power, peak clock, average load

**New modules**
- `src/perf/` with a `PerfSource` trait.
- `perf/intel.rs`: pure RAPL, limit and ratio decoding.
- `perf/linux.rs`: `/proc/stat`, cpufreq and powercap.
- `perf/windows.rs`: PDH load and speed, plus PawnIO RAPL/limits/APERF/MPERF.
- The PawnIO handle and affinity guard are shared within the crate.
- Windows core-to-logical-CPU sets and per-CPU rated MHz.

**Dependencies**: none added.

**Validation on real hardware** (i7-13620H, Windows 11, elevated)

1. **Measurement method, from a diagnostic over identical 1 s windows** (12
   windows at idle and under load):
   - TSC rate is 2918.4 MHz.
   - The MPERF rate on busy cores is about 2890–2960 MHz, which is the TSC
     rate. It is not the 2900 MHz `MSR_PLATFORM_INFO` ratio, and not the
     2400 MHz base.
   - Windows `% Processor Performance` ÷ (100 × APERF/MPERF) is 1.200 on
     P-cores and 1.208 on E-cores. So Windows uses a ~2400 MHz reference for
     all cores. Using the E-cores' own 1800 MHz rated speed would have been
     ~25% wrong.
   - **Fixes made as a result:**
     - The APERF/MPERF reference is now the measured TSC rate. turbostat's
       platform ratio would have been 0.6% low here.
     - The PDH reference is now the P-core rated speed for every core.
     - "Base" now shows the advertised 2400 MHz rather than the 2900 MHz
       register ratio.
2. **Final run:**
   - Base 2400 and max turbo 4900 MHz.
   - Under full load: P-cores 1405–1424 MHz and E-cores 1509 MHz, matching
     the diagnostic's TSC × ratio figures. Package power 18 W at 63 °C.
3. **Finding:** the power-limit registers say PL1/PL2 = 82 W. Yet the CPU
   sustained only ~18–20 W under full load (a 37 W burst lasted about 3 s)
   while running cool. A lower limit is being enforced elsewhere (firmware,
   Windows power mode or battery). The UI now states that register limits may
   not be the effective limits.
4. **Bugs found and fixed during validation:**
   - An idle logical CPU produced "0 MHz" from the Windows counter; it is now
     treated as no measurement and shown as `idle`.
   - Inside WSL, cores with no cpufreq data were mislabelled `idle`; they now
     show `--`.
   - PawnIO error text mentioned only temperatures; it is now generic.
   - Windows' `_Total` "% Processor Time" sometimes reported 0% while
     individual CPUs were 20–38% busy (a negative value from idle-time
     accounting over a short window, clipped to 0). Total load is now the
     mean of the per-CPU values. Afterwards it was consistent in repeated runs.
   - Linux powercap fixtures can't exist on Windows (`:` in file names), so
     that test is Unix-only and runs in WSL.

**Tests**: Windows 110 unit (+1 opt-in diagnostic) + 19 e2e; Linux (WSL)
101 unit + 20 e2e. Clippy (`-D warnings`) is clean on both targets, and fmt
is clean.

**Open items**
- AMD power and clocks on Windows.
- Reading the *effective* (firmware/MMIO) power limit.
- Linux bare-metal verification.

---

## Revision 3: CPU information and health (2026-11-12)

**Added**
- **`rusttemp --info`**:
  - Manufacturer, model, family/model/stepping, microcode, cores (P/E split),
    threads, packages, caches, base/max clock, thermal limit.
  - A CPU HEALTH section with plain-language explanations.
  - JSON via `--info --json`.
- **CPU line** in the `--once` and monitor headers.
- **CPU HEALTH** section in `--once` and in the monitor, which adds a
  "This session" line:
  - thermal, PROCHOT, critical-temperature and power-limit throttling, each
    shown as "now" and "logged"
  - Linux kernel throttle counters
  - hottest CPU sensor and its headroom to TjMax
  - share of session time at each warning level
- **JSON**: `cpu` and `health` in snapshots; per-sensor `headroom_c`,
  `throttle`, `stats.level_samples` and `stats.thermal_throttle_samples`.

**New modules**
- `src/cpuinfo/`: CPUID, Windows topology/registry/power information, Linux
  `/proc` + sysfs.
- `src/health.rs`: the health summary.
- `intel_dts::decode_throttle_flags`.
- Linux `read_throttle_counters`.

**Dependencies**: no new crates. windows-sys gained the `Registry` and `Power`
features.

**Verification on real hardware** (i7-13620H, Windows 11, VBS on):
- **Identity:** every field matches the registry and WMI. That includes the
  cache totals (9728 KiB L2, 24576 KiB L3) and the base clock (2400 MHz).
- **Max turbo clock:** shown as unavailable. CPUID leaf 0x16 is zeroed by the
  hypervisor; this was confirmed with a raw CPUID probe.
- **Health:** read elevated over 14 samples, idle → load → idle.
  - Power-limit throttling was active on all sensors in every sample.
  - Thermal and PROCHOT throttling were logged since boot, not active.
  - Critical temperature was never reached.
  - Headroom was 9–16 °C.
- **Accuracy issue found and fixed during validation:** inside WSL, Linux
  reports the VM's virtual CPU (8 cores, microcode 0xffffffff). RustTemp now
  detects the hypervisor flag, says so, and drops the placeholder microcode.

**Tests**: Windows 95 unit + 18 e2e; Linux (WSL) 88 unit + 19 e2e. Clippy
(`-D warnings`) is clean on both targets, and fmt is clean.

**Open items**
- AMD health flags on Windows.
- Max turbo clock on VBS systems.
- Linux bare-metal verification.
- Third-party cross-check of the throttle flags, e.g. compare "PROCHOT
  logged" with ThrottleStop or HWiNFO.

---

Revision 2 (Windows CPU temperatures, local time)

## 1. Executive summary
RustTemp now reads **real Intel CPU package and per-core temperatures on
Windows**, through the Microsoft-signed PawnIO driver and its official
`IntelMSR` module. This was verified on an i7-13620H laptop running Windows 11
with Memory Integrity enabled.

Times are now shown in local time with an explicit UTC offset (`--utc` to
switch), and each sensor shows when it last produced a valid reading.

All seven functional requirements are verified on Windows. Linux remains
tested-only, because no Linux hardware with sensors was available.

## 2. Functionality added in this revision
- **Windows Intel DTS backend** (`src/sensors/platform/pawnio.rs`):
  - Checks the CPU with CPUID (vendor, DTS, package DTS).
  - Loads `PawnIOLib.dll` at runtime by absolute path.
  - Embeds the signed IntelMSR module.
  - Gets cores and packages from the OS topology, with the P/E-core class.
  - Pins the reading thread to each core, reads `IA32_THERM_STATUS`,
    `IA32_PACKAGE_THERM_STATUS` and `MSR_TEMPERATURE_TARGET`, then restores
    affinity.
  - Never writes any register and never assumes a TjMax.
- **Register decoding** (`src/sensors/platform/intel_dts.rs`): pure,
  OS-independent, unit-tested.
- **Composite backend** (`composite.rs`): on Windows, combines Intel DTS with
  the ACPI zones.
- **Diagnostic notes.** The trait method `SensorBackend::notes` explains why a
  source is unavailable: not installed, access denied, AMD, and so on. Notes
  appear in text and JSON.
- **Time** (`src/time.rs`):
  - Local-time conversion through the OS (`SystemTimeToTzSpecificLocalTime` /
    `localtime_r`), so DST is handled by the OS.
  - `--utc` flag, an UPDATED column, and `timestamp_local` in JSON.

## 3. Dependencies
- No new crates.
- windows-sys gained the features `LibraryLoader`, `SystemInformation`,
  `Threading` and `Time`.
- `libc` (already in the graph through ctrlc/nix) is now a direct Unix
  dependency, used for `localtime_r`.
- New runtime requirement on Windows, for CPU temperatures only: the PawnIO
  driver (`winget install namazso.PawnIO`, version 2.2.0 used) and
  administrator rights.
- Vendored asset: `assets/pawnio/IntelMSR.bin` (LGPL-2.1). It was verified
  against the release digest; see `assets/pawnio/README.md`.

## 4. System changes made on the development machine
- Rust toolchain installed at user level (first revision).
- **PawnIO 2.2.0 installed via winget**, with the user approving the UAC prompt.
  The `PawnIO` kernel service is running. The driver is signed by Microsoft
  Windows Hardware Compatibility Publisher.

## 5. Test results
| Check | Windows (MSVC) | Linux (musl, in WSL2) |
|---|---|---|
| Unit tests | 69 passed | 64 passed |
| Integration tests | 14 passed | 15 passed (adds SIGINT) |
| `clippy --all-targets -D warnings` | clean | clean |
| `fmt --check` | clean | n/a |

### Practical validation, Windows, administrator terminal
| Check | Result |
|---|---|
| `--sensors` | 12 sensors: Package id 0, Core 0–5 (P-core) on logical CPUs 0,2,4,6,8,10, Core 6–9 (E-core) on 12–15, and ACPI `\_TZ.THRM`. TjMax 100 °C on every core. |
| Topology vs spec | i7-13620H = 6 P-cores (Hyper-Threading) + 4 E-cores = 16 threads. Matches. |
| TjMax vs spec | Intel lists 100 °C maximum junction temperature for the i7-13620H. Matches. |
| Package vs hottest core | Equal or within 1 °C in all 12 load-test samples. |
| Load response | Samples 2–3 idle at 63–64 °C. During 6 s of all-thread load (samples 4–10): 86 → 92 °C. Afterwards: 85 → 69 °C. |
| PawnIO unit tests (elevated) | 3/3 pass on the success path. |
| Non-elevated run | No error. The ACPI zone is shown with the note "PawnIO access denied: run RustTemp from an administrator terminal…". |

## 6. Outstanding issues
1. **Independent cross-check on Windows.** I planned an automated comparison
   against LibreHardwareMonitorLib 0.9.6 (downloaded and digest-verified). It
   was **blocked by this environment's safety policy**, because it would have
   run third-party code with administrator rights. Recommended: run
   HWiNFO or LibreHardwareMonitor next to `rusttemp` and compare core values.
   They read the same registers, so they should agree within 1–2 °C allowing
   for sampling time.
2. **AMD on Windows** is not implemented.
3. **Linux on real hardware** is unverified.
4. Untested Intel generations: decoding follows the SDM and the code checks
   CPUID, but only 13th-gen was observed.

## 7. Known limitations
See [README §11](../README.md#11-known-limitations).

## 8. Release readiness
- **Windows + Intel:** usable. The feature is verified on real hardware by
  internal consistency, specification match and load response. A comparison
  against a third-party tool is still recommended before calling it fully
  validated.
- **Linux:** not release-ready until it has been verified on hardware.
- **AMD on Windows, macOS:** not supported.
