# Development checklist: `cpu` / `gpu` hardware monitor

Legend:
- `[ ]` not started
- `[x]` implemented **and** verified
- `[!]` blocked, or needs investigation

An item is ticked only after it has been tested and verified. "Verified on
HW" means it was observed on the development machine: Intel Core i7-13620H +
Intel UHD Graphics, Windows 11, no dedicated GPU.

Plan and evidence: [docs/HARDWARE_MONITOR_PLAN.md](docs/HARDWARE_MONITOR_PLAN.md).

---

## Phase 1 — Discovery and technical research

- [x] **Inspect project and environment**
  - Where: [plan §A1](docs/HARDWARE_MONITOR_PLAN.md#a1-environment)
  - Verified by: PowerShell/CIM inventory, `cargo` / `rustc`, a PnP scan
    including hidden devices
  - Result: Windows 11, i7-13620H, Intel UHD only, Rust 1.99.1
    (MSRV 1.85 verified)
  - Limitation: no dGPU, no Linux hardware
- [x] **CPU identification capabilities**
  - Where: existing `src/cpuinfo/`
  - Verified by: values match the registry and WMI (previous revisions)
  - Result: all fields OK; max boost needs admin because the hypervisor hides
    CPUID leaf 0x16
- [x] **iGPU detection capability (D3DKMT)**
  - Verified by: Probe A
  - Result: Intel UHD has `HybridIntegrated` + `PostDevice` at PCI 0:2.0,
    128 MiB dedicated / 8004 MiB shared. The software adapter is flagged
    `SoftwareDevice`.
- [x] **iGPU telemetry capability (Intel IGCL)**
  - Verified by: Probe B (admin and non-admin)
  - Result: clock, activity counters and throttle flags work. Temperature is
    not supported. Voltage fails plausibility. The energy counter is the
    package counter (see next item).
- [x] **iGPU power source**
  - Verified by: Probe C, 7 windows side by side
  - Result: IGCL energy equals RAPL PKG to ±0.01 W. RAPL PP1 is the real
    graphics power (0.05–0.84 W).
  - Limitation: needs admin + PawnIO
- [x] **CPU voltage source**
  - Verified by: Probe C
  - Result: `IA32_PERF_STATUS[47:32]` gives 0.71–1.18 V, tracking the clock
    ratio
  - Limitation: admin + PawnIO
- [x] **Dedicated GPU interfaces researched**
  - Verified by: documentation review, and `nvml-wrapper` 0.13 checked on
    crates.io (maintained, ~8M downloads)
  - Limitation: can't be exercised here (no NVIDIA/AMD hardware)
- [x] **Windows GPU performance counters**
  - Verified by: `Get-Counter -ListSet 'GPU*'` and sample reads
  - Result: `GPU Engine` and `GPU Adapter Memory` work without admin. Their
    LUIDs match D3DKMT.
- [x] **Command-name availability**
  - Verified by: `Get-Command cpu, gpu`
  - Result: both free on this machine
- [x] **Feasibility report written**
  - Where: [plan Part A](docs/HARDWARE_MONITOR_PLAN.md#part-a--phase-1-discovery-and-feasibility)

## Phase 2 — Architecture and plan

- [x] **Command-interface design**: separate `cpu` / `gpu` binaries plus
  `rusttemp cpu|gpu` (plan §B1)
- [x] **Module structure**: identification (`hardware/`) separated from
  acquisition (`sensors/gpu/`) (plan §B2)
- [x] **Classification rule set**: evidence-based; Unknown when ambiguous
  (plan §B3)
- [x] **Dependency evaluation** (plan §B4)
- [x] **Sensor acquisition and resource strategy** (plan §B5)
- [x] **Testing strategy** (plan §B6)
- [x] **Architecture approved by the project owner** (2026-11-13). Decisions:
  - `nvml-wrapper` for NVIDIA
  - `--interval` in seconds for `cpu` / `gpu` (`rusttemp` keeps milliseconds)
  - AMD: basic support now, vendor sensors later

## Phase 3 — CPU monitoring (`cpu` report)

Already implemented and verified in RustTemp; reused by `cpu`:

- [x] **Manufacturer, model, physical/logical cores, base frequency**
  - Where: `src/cpuinfo/`
  - Verified by: matches WMI and the registry (REPORT rev. 3)
- [x] **Max advertised boost**
  - Where: `src/perf/windows.rs` (`MSR_TURBO_RATIO_LIMIT`)
  - Result: 4900 MHz, matching Intel's spec
  - Limitation: admin
- [x] **Current frequency, total + per-core utilization**
  - Where: `src/perf/`
  - Verified by: TSC × APERF/MPERF validated against a diagnostic
    (REPORT rev. 4)
  - Result: without admin, uses PDH (±1.5%)
- [x] **Package + per-core temperatures, session max, availability**
  - Where: `src/sensors/platform/pawnio.rs`, `monitoring/stats.rs`
  - Verified on HW (rev. 2)
  - Limitation: admin
- [x] **Package power and power limits**
  - Where: `src/perf/windows.rs` (RAPL)
  - Verified on HW (rev. 4)
  - Limitation: register limits may not be the enforced limits
- [x] **Thermal / power throttling status**
  - Where: `src/health.rs`, `intel_dts.rs`
  - Verified on HW (rev. 3)

New work:

- [x] **`Measurement<T>`** (measured / unavailable / not applicable, with
  source)
  - Where: `src/measurement.rs`; used throughout `src/report/cpu.rs`
  - Verified by: `measurement::tests` (zero ≠ unavailable, the three JSON
    forms, `from_option`); e2e `json_is_valid_and_marks_unavailable_values`
    (unavailable fields have no `value` key)
- [x] **CPU voltage** from `IA32_PERF_STATUS`
  - Where: `perf/intel.rs::decode_core_voltage` (plausible range 0.3–2.0 V),
    `perf/windows.rs::IntelPerf::core_voltage` (read on a P-core)
  - Verified by: unit test with raw values captured in Probe C. Elevated run
    on HW: 0.733–0.922 V at idle, 0.936 V under full load.
  - Limitation: admin + PawnIO. On Linux it is unavailable, with the reason.
- [x] **CPU architecture**: instruction set + microarchitecture table
  - Where: `src/hardware/tables.rs` (Linux `intel-family.h` / `amd.c`
    sources)
  - Verified by: `hardware::tables::tests` (Intel / AMD / unknown models are
    not guessed). HW shows "x86-64 · Raptor Lake".
  - Limitation: only models in the table are named; only Raptor Lake has
    been observed on HW.
- [x] **Power history** during a session
  - Where: `perf::PerfStats::package_w_history` (last 60 readings; missing
    readings are skipped, not stored as 0), plus a sparkline in
    `terminal/report.rs`
  - Verified by: `power_history_is_bounded_and_skips_missing_readings`,
    `sparkline_scales`. HW `cpu --watch`: `▁▁▃███`, 9.9 → 62.4 W over 6
    refreshes.
- [x] **`cpu` report and executable**
  - Where: `src/report/cpu.rs` (data model), `src/terminal/report.rs`
    (layout), `src/commands/cpu.rs`, `src/bin/cpu.rs`
  - Verified by: `report::cpu::tests` (elevated / unelevated / AMD-style),
    `terminal::report::tests`, `commands::cpu::tests`, and
    `tests/cpu_cli.rs` (5 e2e tests on a mock tree)
  - Result: Windows 131 unit + 19 + 5 e2e; Linux (WSL) 121 unit + 20 + 5 e2e
- [x] **Elevated validation of all CPU fields** on this machine
  - Verified by: real run. Every field was measured with its source:
    - max boost 4900 MHz
    - 63 °C idle → 94 °C under load, with thermal throttling shown as
      "YES, right now" at 94 °C
    - 11.2 W idle → 62.4 W under load
    - per-core clocks and temperatures
  - Unelevated: identity, frequency and load are measured. Admin-only fields
    read "needs administrator rights", with one explanatory note.
  - VM (WSL): VM note shown; Linux-specific wording for max boost.

## Phase 4 — Integrated GPU

- [x] **D3DKMT adapter enumeration**
  - Where: `src/hardware/windows.rs`
  - Verified by: `live_enumeration` (LUIDs, software adapter present); HW
    shows Intel UHD with VEN 8086 / DEV A7A8 at 0000:00:02.0, 128 MiB +
    8004 MiB
- [x] **SetupAPI driver version**
  - Where: `hardware/windows.rs::driver_versions`
  - Verified by: `location_strings`; HW shows `32.0.101.7076`, matching WMI
    `Win32_VideoController.DriverVersion`
- [x] **Evidence-based classification**
  - Where: `src/hardware/gpu.rs`
  - Verified by: 9 unit tests, using Probe A fixtures plus hybrid NVIDIA,
    desktop AMD (→ Unknown), desktop NVIDIA, eGPU, ambiguous APU,
    conflicting flags and software adapter
  - Result on HW: Intel UHD is integrated (3 signals); Basic Render is
    software
  - Limitation: desktop AMD dGPUs without hybrid flags are "Unknown" (still
    shown by `gpu`)
- [x] **Linux DRM enumeration**
  - Where: `src/hardware/linux.rs`
  - Verified by: mock-tree tests (i915 iGPU → integrated, amdgpu card →
    unknown, connector entries skipped)
  - Limitation: hardware-unverified
- [x] **PDH GPU utilization (busiest engine, summed over processes)**
  - Where: `sensors/gpu/windows.rs`
  - Verified by: unit tests, plus diagnostic `diag_igpu_utilization` against
    IGCL's busy counter over the same windows: idle 7.6 vs 6.5%, D3D11
    compute load 98.5 vs 95.7% (within 1–3 points)
- [x] **PDH adapter memory (shared / dedicated used)**
  - Verified by: unit tests; HW shows 1.04 GiB shared used, matching the
    `Get-Counter` sample from discovery
- [x] **IGCL provider**
  - Where: `sensors/gpu/igcl.rs`
  - Verified by: `layouts_match_the_c_header` (offsets printed by MSVC),
    `item_decoding`, `live_open`. Clock is 100–350 MHz at idle and
    1200–1400 MHz under D3D11 load; the power-limited flag is true under
    load.
- [x] **IGCL plausibility guards**
  - The energy counter is never used as GPU power.
  - Voltage is accepted only within 0.5–1.5 V. On HW it is rejected: 0.26 V
    at 100 MHz, 0.417 V at 1300 MHz.
- [x] **iGPU power from RAPL PP1**
  - Where: `perf/windows.rs` (`MSR_PP1_ENERGY_STATUS`), `report/gpu.rs`
  - Verified on HW elevated: 0.05 W idle → 10.3–10.8 W under GPU load →
    0.09 W after
  - Limitation: admin + PawnIO
- [x] **iGPU temperature: technically unavailable, with the reason**
  - Verified by: the `GpuSample::for_device` test; HW report wording
- [x] **iGPU section in `cpu`**
  - Where: `report/gpu.rs`, `terminal/report.rs::render_gpu_fields`
  - Verified by: e2e `integrated_gpu_not_detected_line` (exact text
    `Integrated GPU: Not detected.`), `integrated_gpu_detected_from_drm_tree`
    (an unclassified card is not shown as the iGPU), and real HW runs, both
    unelevated and elevated
- [x] **Cross-check PDH vs IGCL utilization under GPU load** (see above;
  load generator: a D3D11 compute shader in a scratch C program)
- [x] **iGPU clock validated under 3D load**: 100 → 1250–1400 MHz, within
  the driver's 1500 MHz rated frequency

## Phase 5 — Dedicated GPU

- [x] **`GpuSource` trait and mock sources**
  - Where: `sensors/gpu/mod.rs` (`GpuSource`: `devices`, `sample`, `notes`);
    Windows (`windows.rs`) and Linux (`linux.rs`) implementations
  - Verified by: Linux mock DRM trees in unit and e2e tests; synthetic
    devices in `report::gpu::tests`
- [x] **NVML provider**
  - Where: `sensors/gpu/nvidia.rs`
    - `nvml-wrapper` 0.13, optional Cargo feature `nvidia` (on by default)
    - library loaded by explicit path: `%SystemRoot%\System32\nvml.dll` /
      `libnvidia-ml.so.1`
  - Verified by:
    - `nvidia::tests::full_reading` and
      `unsupported_values_and_fanless_laptop_gpu`: synthetic readings mapped
      to every field
    - `opening_without_nvidia_driver_explains`: on this machine, a real
      attempt to load NVML gives the "NVIDIA NVML is not available" reason
  - Limitation: **no NVIDIA hardware**; mapping follows NVML's documented
    semantics (see the [!] item below)
- [x] **Throttle reasons → thermal / power throttling status**
  - Mapping:
    - `SW_POWER_CAP` / `HW_POWER_BRAKE` → power throttling
    - `SW_THERMAL` / `HW_THERMAL` → thermal throttling
  - Verified by: `full_reading` (each bit pattern), synthetic data only
- [x] **Fan percentage / RPM, VRAM used / free / total, power + limit,
  temperatures + thresholds**
  - Where: `nvidia::apply` (NVML), `linux::sample_card` (amdgpu hwmon),
    `terminal/report.rs::render_gpu_fields`
  - Verified by:
    - `amdgpu_and_i915` (mock sysfs)
    - e2e `unclassified_card_gets_its_own_section_with_sensors` (42%,
      61.0 °C, 1320 RPM, 123.00 W, 2.00 of 12.00 GiB; missing memory clock
      shown as Unavailable)
  - Fanless GPUs (NVML fan count 0) are shown as not applicable, never 0 RPM
- [x] **Generic PDH provider for AMD / other vendors** (utilization + memory
  only)
  - Where: `sensors/gpu/windows.rs`
  - Verified by:
    - `busiest_engine_summed_over_processes`, `memory_counters`,
      `engine_instances`
    - live on the Intel iGPU, through the same code path
  - Vendor-only values (clock, temperature, power, fan) are unavailable,
    with a reason (for AMD: "AMD vendor sensors are not implemented yet")
- [x] **Multiple dGPUs shown independently**
  - Verified by:
    - `report::gpu::tests::gpu_command_separates_kinds_and_never_merges`:
      two cards at 60 / 70 °C stay separate
    - `terminal::report::tests::two_dedicated_gpus_get_separate_numbered_sections`:
      "DEDICATED GPU 1 OF 2" / "2 OF 2", each with only its own values
- [x] **`Dedicated GPU: Not detected.`**
  - Verified by:
    - e2e `not_detected_without_any_gpu` and
      `integrated_gpu_is_never_shown_as_dedicated`
    - real HW: `gpu` prints exactly this line, run from a fresh shell after
      install (Phase 6)
- [x] **Missing driver / missing NVML handled gracefully**
  - Verified by:
    - e2e `nvidia_card_without_nvml_is_explained`
    - `opening_without_nvidia_driver_explains`
    - builds with `--no-default-features` say NVML support was not compiled
      in
  - Exit code 0, never invented values
- [!] **Real-hardware validation of the NVIDIA / AMD / Arc paths**: blocked,
  no such hardware available
  - Everything above for dGPUs is verified with synthetic data or mock
    sysfs trees only.
  - Desktop AMD cards without hybrid flags are shown as *unclassified*, by
    design (see [REFERENCE](docs/REFERENCE.md#classification-rules)).

## Phase 6 — CLI integration

- [x] **`src/bin/cpu.rs`** (verified with the `cpu` e2e tests and on HW)
- [x] **`src/bin/gpu.rs`, `rusttemp cpu|gpu` subcommands**
  - Where: `src/bin/gpu.rs`, `src/cli.rs` (`Command::{Cpu, Gpu}`,
    `args_conflicts_with_subcommands`), `src/main.rs`
  - Verified by:
    - `cli::tests::cpu_and_gpu_subcommands`
    - e2e `cpu_and_gpu_subcommands_run_the_same_reports`
    - HW: `rusttemp cpu --json` gives `command=cpu`
- [x] **`cpu` options:** `--watch`, `--interval SECONDS`, `--count`,
  `--json`, `--help`, `--no-color`, `--utc`
  - Verified by: `commands::cpu::tests`; e2e tests for watch + NDJSON,
    invalid arguments (`--interval` without `--watch` gives exit 2) and help
- [x] **`gpu` options, including `gpu --all`**
  - Verified by:
    - `commands::gpu::tests::options`
    - e2e `watch_json_and_invalid_arguments`: 2 NDJSON lines at 0.25 s;
      `--interval` without `--watch`, unknown flag and interval 0 each give
      exit 2
    - `integrated_gpu_is_never_shown_as_dedicated` (`--all` sections)
- [x] **Colour-coded temperatures and utilization in `cpu`**
  - Where: `terminal/report.rs` (temperatures by warning level; utilization
    green < 60%, yellow < 85%, red otherwise)
  - Every value is also printed in words, so colour is never the only signal
- [x] **Install via `cargo install --path .`; `cpu` / `gpu` on PATH in a new
  terminal**
  - Verified by (2026-11-13):
    - `cargo install --path . --root <scratch>` installed `cpu.exe`,
      `gpu.exe` and `rusttemp.exe` (27 s with a warm cache)
    - a fresh `powershell -NoProfile` with only that `bin` folder added to
      PATH, run from `%TEMP%`, resolved `cpu` and `gpu` there
    - in that shell, `cpu --version`, `gpu --version`, `gpu`, `cpu` and
      `rusttemp cpu --json` all worked
    - `cargo uninstall rusttemp --root <scratch>` removed all three
  - Later installed into the owner's `~/.cargo/bin` at their request.
    `cpu` and `gpu` resolved from a fresh shell started from the home
    folder.

## Phase 7 — Testing and validation

- [x] **Hardware-detection tests**: iGPU, dGPU, multi-GPU, none, unknown,
  software
  - `hardware::gpu::tests`, 10 tests:
    - Probe A devices
    - hybrid NVIDIA laptop
    - desktop dGPU without flags
    - eGPU
    - ambiguous APU
    - conflicting signals
    - NVML memory enrichment
  - `hardware::linux::tests` (3)
  - e2e: none, iGPU only, iGPU + unclassified card, NVIDIA without NVML
- [x] **Sensor tests**: temperature, power, frequency, utilization, memory,
  missing sensors
  - `sensors::gpu::*`, `report::gpu::*`, `report::cpu::*`,
    `measurement::tests` (zero ≠ unavailable)
  - `report::gpu::tests::json_has_no_duplicate_keys`: regression test for a
    bug found during the Phase 8 audit, where JSON repeated `architecture` /
    `driver_version`, so parsers saw the unmerged "Unavailable" copy
- [x] **CLI tests**: `cpu`, `gpu`, `--watch`, `--json`, invalid arguments,
  missing permissions, unsupported platform
  - `tests/cpu_cli.rs` (7), `tests/gpu_cli.rs` (5), `tests/cli.rs` (20 on
    Windows, 21 on Linux)
  - Missing permissions:
    - `report::cpu::tests::unelevated_report_explains_instead_of_inventing`
    - real unelevated runs
  - Unsupported platform: the existing `rusttemp` stub backend
- [x] **Windows suite + Linux suite (WSL) passing; clippy `-D warnings`;
  fmt** (2026-11-13, final run)
  - Windows (`cargo test --release`): **171 unit** (+2 ignored hardware
    diagnostics) + 20 + 7 + 5 e2e, all passing
  - Linux (musl, `--no-default-features`, run in WSL Ubuntu): **152 unit** +
    21 + 7 + 5 e2e, all passing
  - clippy `--all-targets -D warnings`: clean on Windows default features,
    Windows `--no-default-features` and Linux musl
  - `cargo fmt --check`: clean
- [x] **Elevated real-hardware validation run recorded**
  - Phase 3: every CPU field
  - Phase 4: iGPU power 0.05 → 10.8 W, clock 100 → 1400 MHz, PDH vs IGCL
    utilization
  - Logs in the development scratch area; results in
    [docs/REFERENCE.md](docs/REFERENCE.md#gpu-sensors-sources-and-validation)

## Phase 8 — Optimization and documentation

- [x] **Startup time and CPU cost of one-shot and watch modes measured**
  (release build, this machine)
  - `cpu` one-shot: 0.9–1.25 s wall, mostly the deliberate 0.5 s
    measurement window plus Intel driver (Level Zero) initialisation
  - `gpu` one-shot: about 1.1 s
  - `cpu --watch`: 0.39 s CPU time over about 10 s (about 4% of one core
    at 1 s refresh)
  - Peak working set: 74 MiB, dominated by Intel's Level Zero runtime that
    IGCL loads
- [x] **Dependency review, binary size**
  - 6 direct dependencies on Windows: clap, serde, serde_json, ctrlc,
    windows-sys, nvml-wrapper. Linux adds libc.
  - 44 crates in the normal dependency graph, including RustTemp itself
  - Release binaries (LTO, stripped): `cpu` 1.7 MB, `gpu` 1.4 MB,
    `rusttemp` 2.0 MB
- [x] **README**: install, PATH, `cpu` / `gpu` usage, supported hardware,
  limitations
  - Highlighted install box updated for all three commands
  - Command table, output guide, GPU support matrix, known limitations,
    troubleshooting (name clash → `rusttemp cpu`), musl build note
  - Verified by: a link and anchor check over all Markdown files (0
    problems)
- [x] **docs/REFERENCE.md**: GPU identification sources, classification
  rules, sensor sources and validation, `cpu` / `gpu` JSON schema, exit
  codes
  - Verified by:
    - every documented source and rule checked against the code; five
      mismatches found and corrected in the doc
    - JSON field lists generated from real `cpu --json` / `gpu --all --json`
      output
- [x] **Final quality audit**
  - Found and fixed: the duplicate JSON keys above
  - Added: a rendering test for multiple dedicated GPUs
  - Open items, all documented:
    - the [!] above (no dGPU hardware)
    - iGPU temperature and voltage not available on this platform
    - none for licensing: PolyForm Noncommercial 1.0.0 was chosen by the
      owner (see Phase 9)

## Phase 9 — Public release readiness

- [x] **Licence: PolyForm Noncommercial 1.0.0**, chosen by the owner
  (2026-11-13): no copyright line, no selling on third-party platforms
  - `LICENSE.md` is the official text, unmodified (SHA-256 `c0ea4a89…`)
  - Cargo `license = "PolyForm-Noncommercial-1.0.0"`, `publish = false`
  - The bundled PawnIO module keeps LGPL-2.1-or-later. Crate licences are
    collected into `THIRD_PARTY_LICENSES.md` for each release
    (`scripts/third_party_licenses.py`: 43 crates on Windows, 29 on
    Linux).
- [x] **Self-contained Windows programs**
  - Static C runtime (`.cargo/config.toml`)
  - Verified: the PE import tables of `cpu.exe`, `gpu.exe` and
    `rusttemp.exe` contain no `VCRUNTIME140.dll` / `api-ms-win-crt-*`
  - All tests pass on the static build (171 unit + 20 / 7 / 5 e2e)
- [x] **Windows one-line installer** (`install.ps1`) and uninstaller
  (`uninstall.ps1`)
  - Tested through `Get-Content -Raw | iex`, the same way `irm | iex` runs
    them, against a release zip packaged like the workflow does:
    - install, then the window stays open (no `exit`)
    - a tampered checksum is rejected and nothing is installed
    - reinstall over an existing copy works
  - PATH handling tested on a throwaway registry key:
    - the entry is added once
    - `%USERPROFILE%` entries and the REG_EXPAND_SZ type are preserved
    - uninstall removes the entry and the folder, leaving nothing behind
  - Not tested: the elevated path (Program Files, machine PATH, PawnIO
    install through winget). PawnIO is already installed here, and the
    "already installed" detection was verified.
- [x] **Linux installer** (`install.sh`)
  - Tested in WSL Ubuntu with a tarball packaged like the workflow does:
    - piped `sh` install works, and the installed programs run
    - a tampered checksum gives exit 1 and nothing is installed
    - `dash -n` passes; the file has LF line endings
- [x] **GitHub Actions**
  - `ci.yml`: fmt, clippy (both feature sets), tests on Windows and
    Linux, static musl build, MSRV 1.85 check
  - `release.yml`: on a `v*` tag, test, build, package with licences and
    SHA-256 files, publish the release
  - Checked locally: the workflow steps were reproduced (builds, packaging,
    licence script). The workflows themselves have not run on GitHub yet,
    because the repository is not pushed.
- [x] **Repository hygiene**
  - `.gitignore`, `.gitattributes` (LF; `*.sh` LF; `*.ps1` CRLF; `*.bin`
    binary)
  - `CHANGELOG.md`, `SECURITY.md`
  - No personal data or local paths in tracked files (checked by search)
- [x] **README for public users**
  - One-line install, three-step administrator quick start
    (`rusttemp cpu` / `rusttemp gpu`), 1000 ms `--watch` default stated
  - Manual download, update / uninstall, build from source
  - Licence summary
- [!] **First push and v0.1.0 release**: waiting for the owner's go-ahead.
  The install command can only work after the repository is public and the
  release workflow has published v0.1.0.
