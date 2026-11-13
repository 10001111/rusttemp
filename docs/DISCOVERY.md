# Phase 0: Discovery Report

Date: 2026-11-12

## 1. Existing environment

| Item | Finding |
|---|---|
| Host OS | Windows 11 Home Single Language 10.0.26300, x86_64 |
| CPU | 13th Gen Intel Core i7-13620H (10 cores / 16 threads) |
| Repository | Empty directory, no git repository. No prior work to preserve. |
| Rust toolchain | Not installed. Installed rustup at user level: **rustc 1.99.1, cargo 1.99.1** (stable-x86_64-pc-windows-msvc, with clippy and rustfmt). Added target `x86_64-unknown-linux-musl`. |
| C/C++ build tools | Visual Studio 2019 Build Tools (MSVC) present, so the MSVC Rust target links natively. |
| WSL | Ubuntu (WSL2, kernel 6.6.87.2-microsoft-standard-WSL2). No gcc/cc, no Rust. |
| Admin rights | Not elevated. |

## 2. Hardware interface research

### Linux (WSL2 Ubuntu on this machine)
- `/sys/class/hwmon/` contains only `hwmon0` (`AC1`) and `hwmon1` (`BAT1`, `in0_input` only). There are **no `temp*_input` files**.
- `/sys/class/thermal/` contains 16 `cooling_device*` entries and **no `thermal_zone*`**.
- Neither `coretemp` nor `k10temp` is loaded. WSL2 is a Hyper-V VM and does not pass through CPU thermal sensors.
- **Conclusion:** the Linux backend can be built and exercised here, but no real CPU temperature can be observed on Linux on this machine.

Linux interfaces used (kernel ABI: `Documentation/hwmon/sysfs-interface.rst`, `Documentation/ABI/testing/sysfs-class-thermal`):
- `hwmonN/name`: chip/driver name.
- `hwmonN/tempK_input`: millidegrees Celsius.
- `hwmonN/tempK_label`, `tempK_max`, `tempK_crit`: optional.
- Older drivers put these attributes under `hwmonN/device/`.
- `thermal_zoneN/type`, `temp` (m°C), `trip_point_K_type` / `trip_point_K_temp`.
- CPU identification uses the driver name together with its documented labels. See the table in the README and `src/sensors/classify.rs`.

### Windows (host)
| Interface | Admin? | Result on this machine |
|---|---|---|
| `Win32_PerfFormattedData_Counters_ThermalZoneInformation` (WMI) / PDH counter object "Thermal Zone Information" | No | One zone, `\_TZ.THRM`: `Temperature`=343 K, `HighPrecisionTemperature`=3432 dK (70.05 °C) |
| `root\wmi MSAcpi_ThermalZoneTemperature` | Yes | Access denied (not elevated) |
| CPU digital thermal sensor (`IA32_THERM_STATUS` / `IA32_PACKAGE_THERM_STATUS` MSRs) | Kernel driver | Not available to user-mode code |

`\_TZ.THRM` is an ACPI firmware thermal zone. Nothing exposed proves it measures the CPU, so it must not be labelled as a CPU sensor (Rule 3).

## 3. Technical feasibility

| Capability | Feasible here? |
|---|---|
| Linux sysfs backend implementation + automated tests | Yes. The backend is pure file reading and is testable with mock trees on any OS. |
| Linux backend on real CPU sensors | **No.** No Linux hardware with sensors is available. |
| Windows ACPI thermal zone readings (no admin) | Yes, via PDH. Verified. |
| Windows per-core CPU temperatures | **Revision 2: implemented via PawnIO**, at the user's request (PawnIO installed with the user's UAC approval; driver signed by Microsoft WHCP, works with HVCI). Original assessment: not without a kernel driver. Out of scope: installing third-party kernel drivers is a security risk. For example, the widely used WinRing0 driver has a known privilege-escalation vulnerability (CVE-2020-14979) and is flagged by Microsoft Defender. |
| macOS | No hardware available; not implemented. |
| Running Linux builds | Yes. Cross-compile to `x86_64-unknown-linux-musl` with `rust-lld` (no C toolchain needed) and run inside WSL. |

## 4. Recommended strategy
1. Implement the Linux sysfs backend as the primary backend, as the specification requires. Parameterise it by a root directory so it can be tested on any OS and pointed at captured trees (`--sysfs-root`).
2. Add a Windows backend for ACPI thermal zones through PDH. Classify those zones strictly as `acpi_thermal_zone`, never as CPU.
3. Keep classification conservative. Use only driver names and labels that the drivers document.
4. Validate on Windows hardware. Validate Linux via WSL with real (empty) sysfs, symlinked mock trees and cross-compiled test binaries.

## 5. Identified limitations
- No Linux machine with CPU sensors is available, so Linux CPU readings stay **unverified on hardware**.
- Windows exposes only ACPI zones without elevated or kernel access.
- Time zone resolution is unavailable without extra crates, so times are shown in UTC.

## 6. Architecture

```
src/
  main.rs              entry point, exit codes, broken-pipe handling
  lib.rs               module wiring
  app.rs               runs each command (sensors / once / monitor)
  cli.rs               clap definition → validated Config
  errors.rs            Error enum, exit codes
  time.rs              local/UTC/RFC 3339/duration formatting
  health.rs            CPU health summary (throttle flags, counters, headroom, session)
  perf/
    mod.rs             PerfSample/PerfSource/PerfStats, default_source()
    intel.rs           RAPL units/energy/limits, base & turbo ratios, APERF/MPERF math
    linux.rs           /proc/stat load, cpufreq speed, powercap power & limits
    windows.rs         PDH load/speed + PawnIO RAPL, limits, APERF/MPERF (cfg(windows))
  cpuinfo/
    mod.rs             CpuInfo model, cache grouping, detect()
    cpuid.rs           vendor, brand, family/model/stepping, leaf 0x16 clocks
    linux.rs           /proc/cpuinfo + sysfs topology, caches, cpufreq, VM flag
    windows.rs         topology, caches, registry microcode, rated MHz (cfg(windows))
  sensors/
    mod.rs             SensorInfo, SensorKind, Reading, SensorBackend trait, unit conversions
    classify.rs        CPU sensor identification rules
    platform/
      mod.rs           default_backend() per OS (Windows: composite of the two below)
      sysfs.rs         Linux hwmon + thermal zones (root-parameterised)
      intel_dts.rs     Intel DTS register decoding + P/E-core labels (pure, all OS)
      pawnio.rs        Intel package/core temps via PawnIO driver (cfg(windows))
      windows.rs       PDH thermal-zone counters (cfg(windows))
      composite.rs     concatenates backends, collects diagnostic notes
assets/pawnio/         vendored signed IntelMSR module + LGPL text + provenance
  monitoring/
    mod.rs             Monitor (sampling, run loop with stop channel), Snapshot
    stats.rs           per-sensor min/max/avg/counters
    thresholds.rs      Thresholds, Level
  terminal/mod.rs      text rendering, color/redraw detection, Windows VT enable
  output/mod.rs        JSON documents
tests/cli.rs           end-to-end tests of the binary against mock sysfs trees
```

Data flow: `SensorBackend::read()` → `Monitor::sample()` (updates `SensorStats`) → `Snapshot` → `terminal::render_*` or `output::*_json` → stdout.

Ctrl+C: the `ctrlc` handler sends on an `mpsc` channel. The loop waits with `recv_timeout(interval − work time)`, so it stops immediately instead of after the next tick.

## 7. Dependencies

| Crate | Purpose | Why not std |
|---|---|---|
| `clap` (derive) | Parsing, help, validation errors | Writing equivalent help and error messages by hand would be a large amount of error-prone code |
| `serde` + `serde_json` | JSON output | Correct escaping and float formatting |
| `ctrlc` | Ctrl+C on Unix (SIGINT) and Windows (console control handler) | std has no signal API |
| `windows-sys` (Windows only) | PDH and console bindings | Avoids hand-writing FFI struct layouts. Already in the graph via clap/ctrlc (same version), so it adds no new crate. |
| `tempfile` (dev only) | Temporary mock sysfs trees | Standard, safe cleanup |

Rejected: `anyhow`/`thiserror` (a small hand-written error enum is enough), `crossterm`/`ratatui` (three ANSI sequences plus one Win32 call do the job), `chrono` (only UTC formatting is needed), `sysinfo` (hides how sensors are classified, which Rule 3 needs to control).

## 8. Implementation roadmap and phase record

| Phase | Objective | Acceptance criteria | Verification | Status |
|---|---|---|---|---|
| 0 Discovery | Know the environment and interfaces | Report covers environment, feasibility, limits, architecture | This document | Complete |
| 1 Foundation | Crate, module skeleton, errors, toolchain | `cargo build` clean | Build log: no warnings | Complete |
| 2 Sensor discovery | Find sensors, classify CPU sensors | Correct ids/kinds on Intel/AMD/legacy/empty trees; Windows zones found | `sysfs::tests::*`, `classify::tests::*`, Windows contract test, WSL real `/sys` run | Complete (Linux hardware unverified) |
| 3 Temperature collection | Read and convert values; handle failures | m°C→°C, dK→°C exact; missing/invalid/implausible → Unavailable | `sensors::tests::*`, `sysfs::tests::*`, WMI cross-check | Complete |
| 4 Monitoring engine | Periodic sampling, stop handling | Interval respected; stops on count, signal or callback error | `monitoring::tests::*`; SIGINT test (Linux); CTRL_C_EVENT test (Windows) | Complete |
| 5 Terminal interface | Readable live display | CPU/other grouping, words for levels, no escapes when piped, in-place redraw on TTY | `terminal::tests::*`, manual runs | Complete |
| 6 Statistics & warnings | min/max/avg/duration, 4 levels | Correct values incl. unavailable samples; boundaries inclusive; invalid thresholds rejected | `stats::tests::*`, `thresholds::tests::*`, e2e `monitoring_tracks_changing_values` | Complete |
| 7 CLI | All documented commands | Each command works; bad input exits 2 with a message | `cli::tests::*`, e2e `invalid_arguments_*` | Complete |
| 8 JSON | Valid machine-readable output | Parses; nulls for unavailable; NDJSON one line per sample | `output::tests::*`, e2e JSON tests | Complete |
| 9 Testing & validation | Automated + practical checks | check/test/clippy/fmt clean on Windows and Linux targets | See REPORT.md §8 | Complete |
| 10 Documentation & release | README, tracker, report | Docs match behaviour | README, PROGRESS.md, REPORT.md | Complete. **Release readiness: not ready** (see REPORT.md) |
