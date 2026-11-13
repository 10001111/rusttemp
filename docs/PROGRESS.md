# Project Progress Tracker

Last updated: 2026-11-12, after the Windows CPU-temperature (PawnIO) and time-display work.

Statuses used: Not Started, In Progress, Implemented, Tested, Verified, Blocked.
**Verified** means automated tests pass *and* the behaviour was seen working on
real hardware or a real OS.

| ID | Requirement | Status | Evidence | Remaining Work |
|---|---|---|---|---|
| FR-01 | Sensor discovery | **Verified** (Windows) / **Tested** (Linux) | **Windows (i7-13620H, Win 11, HVCI on, elevated):** discovered Package id 0, 6 P-cores (logical CPUs 0,2,…,10) and 4 E-cores (12–15), matching the 6P+4E design, plus ACPI `\_TZ.THRM`. TjMax read as 100 °C (Intel spec: 100 °C). Non-elevated: falls back to ACPI with an "access denied" note. **Linux:** `sysfs::tests`, `classify::tests`, e2e `sensors_*`; WSL real `/sys` gives 0 sensors; symlinked mock tree classified correctly | Linux hardware check (coretemp, k10temp); AMD on Windows |
| FR-02 | Temperature monitoring | **Verified** | Windows elevated: live per-core monitoring at 1 s; under 6 s of all-thread load, temperatures rose 63 → 92 °C and fell back to 69 °C. Per-sensor UPDATED column (last valid reading, local time). Linux: e2e monitoring tests, live WSL runs | None |
| FR-03 | Temperature statistics | **Verified** | `stats::tests`; e2e `monitoring_tracks_changing_values`; real Windows run (Package min 69 / avg 79.7 / max 92 over 3 samples) | None |
| FR-04 | Thermal warnings | **Verified** | `thresholds::tests`; e2e custom thresholds; real run shows NORMAL/ELEVATED/HIGH per core. TjMax shown separately as a hardware-reported limit | None |
| FR-05 | Sensor inspection | **Verified** | `--sensors` on Windows (elevated and not) shows id, chip, kind, source (MSR + logical CPU), TjMax, notes | None |
| FR-06 | CLI functionality | **Verified** | `cli::tests` incl. `--utc`; e2e usage-error, help and time-label tests | None |
| FR-08 | CPU identification (`--info`, header line) | **Verified** (Windows) / **Tested** (Linux) | Windows: manufacturer Intel, model "13th Gen Intel(R) Core(TM) i7-13620H", family 6 / model 186 / stepping 2, microcode 0x4122, 10 cores (6P+4E) / 16 threads / 1 package, all equal to the registry and WMI values. L2 9.5 MiB and L3 24 MiB equal WMI `L2CacheSize` 9728 KiB and `L3CacheSize` 24576 KiB. Base 2400 MHz equals WMI `MaxClockSpeed`. Max MHz correctly reported unavailable (the hypervisor zeroes CPUID 0x16). Linux: `cpuinfo::linux` tests (hybrid tree, VM flag, missing files), e2e `info_*`; inside WSL the VM is detected and the placeholder microcode dropped | Max turbo frequency on VBS systems; Linux bare-metal check |
| FR-09 | CPU health (throttling, headroom, session) | **Verified** (Windows Intel) / **Tested** (Linux counters) | Windows elevated, real hardware: flags read from package + 10 cores over 14 samples idle→load→idle. Power-limit throttling active on all sensors in every sample. Thermal and PROCHOT not active but logged since boot. Critical never. Headroom 9–16 °C to TjMax 100 °C. Session shares computed (≥HIGH 93 % for the package). Decoding: `intel_dts::tests::throttle_flags`; summary: `health::tests`; Linux counters: `sysfs::tests::throttle_counters`, e2e `info_shows_identity_limit_and_throttle_counters` | AMD flags on Windows; Linux on real hardware; no third-party cross-check of flags yet |
| FR-10 | CPU speed, load and power (Part 3) | **Verified** (Windows Intel) / **Tested** (Linux) | Windows elevated on the i7-13620H: package and core power from RAPL (7 W idle, ~37 W short burst, ~18–20 W sustained under full load). PL1/PL2 82 W and TDP 45 W read from MSRs. Max turbo 4900 MHz (matches Intel spec). Per-core clocks = measured TSC rate (2918.4 MHz) × APERF/MPERF; a diagnostic over identical 1 s windows showed MPERF ticking at the TSC rate and Windows `% Processor Performance` using a ~2400 MHz reference for all cores (agreement within ~1.5 %). Under full load: P-cores 1405–1424 MHz, E-cores 1509 MHz, exactly matching the diagnostic's independent figures. Without admin: load and approximate clocks from PDH, with power "unavailable (PawnIO access denied…)". Linux: `perf::linux` tests (proc/stat load, cpufreq speed, hybrid labels, powercap power + limits on Unix), e2e `speed_and_power_section_from_mock_tree` | AMD on Windows; Linux bare-metal check; effective (firmware-enforced) power limit not readable |
| FR-07 | JSON output | **Verified** | `output::tests`; e2e JSON tests incl. `timestamp_local` and `notes`; PowerShell `ConvertFrom-Json` over NDJSON from the elevated load test | None |

## "What's left" items from the first report

| # | Item | Status |
|---|---|---|
| 1 | Verify Linux backend on real hardware | **Open.** No Linux machine with sensors is available. |
| 2 | Windows CPU temperatures | **Done for Intel.** PawnIO backend, verified on the i7-13620H. AMD not implemented. No cross-check against a third-party tool yet (see REPORT §9). |
| 3 | Local time and per-sensor update times in the text view | **Done.** Local time with explicit offset by default, `--utc` option, UPDATED column, `timestamp_local` in JSON. |

## Definition of Done

| Item | Met? | Evidence / reason |
|---|---|---|
| Compiles successfully | ✅ | Windows MSVC debug and release; Linux musl |
| Hardware sensor discovery works on the target platform | ✅ Windows / ⚠️ Linux | Windows verified on hardware; Linux only on mock trees and sensor-less WSL |
| Sensor identification sufficiently reliable | ✅ | CPUID + OS topology on Windows; driver/label rules on Linux; ACPI never promoted to CPU |
| Temperature conversion correct | ✅ | DTS decode unit tests; TjMax matches spec; package ≈ hottest core; ACPI equals WMI |
| Real temperature readings displayed when available | ✅ | Real Intel package and core readings on Windows |
| Missing sensors handled correctly | ✅ | Unit + e2e tests; non-admin and no-PawnIO paths give notes, not errors |
| Monitoring operates continuously | ✅ | Windows + WSL live runs |
| Configurable refresh intervals work | ✅ | Tests + manual runs |
| Temperature statistics correct | ✅ | See FR-03 |
| Thermal warnings work as specified | ✅ | See FR-04 |
| CLI commands operate correctly | ✅ | See FR-06 |
| JSON output valid | ✅ | See FR-07 |
| Automated tests pass | ✅ | Windows: 110 unit (+1 opt-in diagnostic) + 19 e2e. Linux (WSL): 101 unit + 20 e2e |
| Clippy: no unresolved warnings | ✅ | `-D warnings`, all targets, Windows and Linux |
| Formatting checks pass | ✅ | `cargo fmt --check` |
| Documentation reflects actual functionality | ✅ | README per-platform verified/unverified statements |
| Known hardware limitations documented | ✅ | README §11 |
| No fabricated readings / unsupported claims | ✅ | No assumed TjMax; implausible values rejected; untested generations and AMD stated as such |

**Overall:** ready to use on **Windows with an Intel CPU**, from an
administrator terminal with PawnIO installed. Linux and AMD remain unverified
or unimplemented.
