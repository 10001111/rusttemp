# CPU + GPU hardware monitor: feasibility report and implementation plan

Status: **Approved 2026-11-13 and implemented (Phases 1–8).** Progress and verification evidence: [DEVELOPMENT_CHECKLIST.md](../DEVELOPMENT_CHECKLIST.md). This document records the original research and design.
Date: 2026-11-13.

This plan extends the existing RustTemp project. It does not replace it. The
current `rusttemp` command keeps working, and two new commands, `cpu` and
`gpu`, are added on top of the same engine.

---

## Part A — Phase 1: discovery and feasibility

### A1. Environment

| Item | Finding |
|---|---|
| OS | Windows 11 Home 10.0.26300, x64. Memory Integrity (VBS/HVCI) on, so Windows runs under the Hyper-V hypervisor. |
| CPU | Intel Core i7-13620H (Raptor Lake, family 6 / model 186): 6 P-cores + 4 E-cores, 16 threads |
| GPUs present | **Intel UHD Graphics only** (PCI `VEN_8086 DEV_A7A8`, bus 0 / device 2 / function 0, driver 32.0.101.7076). **No dedicated GPU**: no NVIDIA or AMD device in any PnP class, including hidden or disabled ones. |
| Software adapter | Microsoft Basic Render Driver (`VEN_1414 DEV_008C`), present on every Windows system |
| Rust | 1.99.1 stable (MSVC); project MSRV 1.85 verified |
| Existing project | RustTemp 0.1.0: CPU temperatures, health, speed/load/power, `--info`. 111 unit + 19 e2e tests passing. |
| Vendor libraries | `ControlLib.dll` (Intel Graphics Control Library, v1.2.253, signed) **present**. `nvml.dll`, `atiadlxx.dll`, `amdadlx64.dll` **absent**. |
| PawnIO driver | Installed and running (used for CPU registers) |
| Command names | `cpu` and `gpu` are **free**: no existing command, alias or function in PowerShell |

### A2. Probe results (real hardware)

Throwaway probes were written and run outside the project (scratchpad) to
check each interface before planning around it.

**Probe A: adapter enumeration via D3DKMT** (`gdi32`, no admin).

| Adapter | Flags reported by Windows | PCI | Memory segments |
|---|---|---|---|
| Intel UHD Graphics | `RenderSupported`, `DisplaySupported`, **`PostDevice`**, **`HybridIntegrated`**, `RuntimePowerManagement` | 0:2.0 | 128 MiB dedicated (carved from RAM) + 8004 MiB shared |
| Microsoft Basic Render | `RenderSupported`, **`SoftwareDevice`** | none (0xFFFFFFFF) | 0 dedicated |

Other facts from the probe:
- The adapter LUIDs match the instance names of the Windows GPU performance
  counters (`luid_0x00000000_0x0001087d`). Identification and utilization
  counters can therefore be joined reliably.

**Probe B: Intel IGCL telemetry** (`ctlPowerTelemetryGet`). Works with and
without admin.

| Item | Supported | Observation |
|---|---|---|
| Activity counters (global / 3D / media) | ✅ | Seconds busy. Utilization = Δbusy / Δtime. |
| GPU clock | ✅ | 100–350 MHz at light load |
| GPU voltage | ✅ (claimed) | **Fails a plausibility check**: 0.260 V at 100 MHz but 0.242 V at 350 MHz. Voltage falling as clock rises is not physical. **Will not be displayed.** |
| GPU energy counter | ✅ (claimed) | **Equals the CPU package energy**, not the GPU (see Probe C). **Must not be shown as GPU power.** |
| GPU temperature | ❌ | Not supported by the driver for this iGPU |
| VRAM energy / clock / temperature | ❌ | Expected, since an iGPU has no VRAM of its own |
| Throttle flags (power / temperature / current / voltage / utilization limited) | ✅ | All false at light load; still needs validation under load |

**Probe C: RAPL + IGCL side by side** (7 one-second windows, admin, last 4
windows under a CPU-only load).

| Window | Package (RAPL) | IGCL "GPU energy" | Cores (PP0) | **Graphics plane (PP1)** | PSYS | Vcore (`IA32_PERF_STATUS`) |
|---|---|---|---|---|---|---|
| idle | 11.02 W | 11.02 W | 5.97 W | 0.05 W | 4.43 W | 0.720 V |
| idle | 8.23 W | 8.24 W | 3.29 W | 0.08 W | 3.55 W | 0.722 V |
| CPU load | 24.66 W | 24.65 W | 19.10 W | 0.32 W | 8.68 W | 0.895 V |
| CPU load | 23.27 W | 23.27 W | 17.05 W | 0.83 W | 7.98 W | 0.872 V |

Conclusions:
- **iGPU power = RAPL PP1 (graphics plane).** It needs admin + PawnIO.
- IGCL's energy counter is the package counter on this iGPU, so it is
  excluded.
- PSYS reads lower than package power, so it is not trustworthy on this
  laptop and won't be used.
- **CPU voltage is available** from `IA32_PERF_STATUS[47:32] × 2⁻¹³ V` (admin).
  It tracked the clock ratio consistently: 0.71 V at 2.3 GHz, 1.18 V at
  4.6 GHz, 0.87–0.91 V at 3.1 GHz.

### A3. Capability matrix

Legend:
- ✅ verified on this machine
- 🧪 implementable but can't be verified on this machine (mock-tested only)
- ⛔ technically unavailable
- 🔒 needs admin + PawnIO

**CPU (`cpu` command)**

| Requirement | Source | Status |
|---|---|---|
| Manufacturer, model, physical/logical cores, base clock | CPUID, Windows topology, power information | ✅ (already in RustTemp) |
| Architecture | ISA `x86-64` (from target), plus a microarchitecture name from a family/model table (e.g. 6/186 = Raptor Lake). Shown only for models in the table. | ✅ / table-limited |
| Max advertised boost | `MSR_TURBO_RATIO_LIMIT` 🔒. CPUID 0x16 is hidden by the hypervisor here. | ✅ 🔒 |
| Current frequency, total + per-core utilization | APERF/MPERF 🔒 or PDH (no admin); PDH `% Processor Time` | ✅ |
| Package + per-core temperature, session max, sensor availability | Intel DTS via PawnIO 🔒; ACPI zone (no admin) | ✅ |
| Package power, power limits, session power history | RAPL 🔒 | ✅ (history currently summary only; sparkline planned) |
| **CPU voltage** | `IA32_PERF_STATUS` 🔒 | ✅ implemented and verified (0.73–0.94 V) |
| Thermal / power throttling | DTS status flags 🔒 | ✅ |
| Overall CPU temperature | Package sensor (shown as "overall") | ✅ |

**Integrated GPU (section of `cpu`)**

| Requirement | Source | Status |
|---|---|---|
| Detection + classification | D3DKMT flags + PCI location + memory segments (see §B3) | ✅ |
| Manufacturer, model, device IDs | D3DKMT + IGCL | ✅ |
| Architecture | PCI device-ID table (e.g. 0xA7A8 → Xe-LP). Shown only when in the table. | table-limited |
| Utilization | PDH `GPU Engine` (vendor-neutral) and IGCL activity counters (Intel); to be cross-checked in Phase 4 | ✅ |
| Core frequency | IGCL `gpuCurrentClockFrequency`; needs validation under 3D load in Phase 4 | ✅ (partially validated) |
| Memory frequency | **Not applicable**: uses system RAM | — |
| Power | RAPL PP1 🔒 | ✅ |
| Temperature | ⛔ No separate sensor exposed. The graphics share the CPU die, so the package temperature covers them. | ⛔ |
| Voltage | ⛔ IGCL value fails plausibility | ⛔ |
| Shared / dedicated memory + usage | D3DKMT segment sizes + PDH `GPU Adapter Memory` | ✅ |

**Dedicated GPU (`gpu` command)**

| Vendor | Identification | Sensors | Status |
|---|---|---|---|
| **NVIDIA** | D3DKMT / SetupAPI + NVML PCI info | NVML: utilization, graphics/memory clocks, temperature, power, power limit, fan %, (fan RPM where supported), VRAM used/free/total, clock-throttle reasons, driver version, architecture. **GPU voltage and memory temperature are not exposed by NVML on GeForce GPUs (⛔).** | 🧪: no NVIDIA GPU here |
| **AMD** | D3DKMT / SetupAPI | Vendor-neutral PDH utilization + memory only. ADLX vendor sensors are **not implemented in this plan** (complex SDK, and no AMD hardware to validate). | partially supported 🧪 |
| **Intel Arc (discrete)** | D3DKMT + IGCL | IGCL temperature / power / fan / VRAM, where the driver supports them. The energy semantics must be re-validated on Arc. | 🧪 |
| none (this machine) | — | Prints `Dedicated GPU: Not detected.` | ✅ |

**Linux** (no Linux hardware with GPUs or sensors available, so everything is 🧪):
- Identification: `/sys/class/drm/card*/device/{vendor,device,class,boot_vga}`
  and the PCI address.
- Sensors:
  - `amdgpu`: hwmon temperature, power, fan, `freq*_input`, `gpu_busy_percent`
    and `mem_info_vram_*`.
  - `i915` / `xe`: `gt_act_freq_mhz`.
  - NVIDIA: NVML.

**macOS:** not supported. Public APIs don't expose temperatures or power
without root (`powermetrics`), and the IOKit/SMC keys are private.

### A4. Key limitations, known up front

1. **No dedicated GPU on the development machine.** The NVIDIA, AMD and Intel
   Arc paths can only be tested with mock providers. They will be labelled
   "implemented, hardware-unverified" until someone runs them on real
   hardware.
2. **Admin-only measurements:**
   - CPU temperatures, voltage, power, limits, throttle flags and max turbo
   - iGPU power

   These come from processor registers (MSRs), which Windows only exposes to
   kernel drivers (PawnIO). Everything else works without admin, and a
   per-field note explains what admin would add.
3. **iGPU temperature and voltage** are technically unavailable on this
   platform.
4. **Laptop fans** are controlled by the embedded controller. Windows exposes
   no CPU fan RPM, so it is shown as unavailable.
5. **Optimus/hybrid laptops:** an idle dGPU may be powered off. Whether NVML
   queries wake it can't be tested here. This will be documented and the
   power state shown where Windows reports it.
6. **Classification edge case:** a desktop AMD APU without hybrid flags, with
   a large BIOS memory carve-out, is ambiguous. The rule set reports it as
   *Unknown* rather than guessing (§B3).

---

## Part B — Phase 2: architecture and development plan

### B1. Command interface (direct `cpu` / `gpu`)

**Decision: separate `cpu` and `gpu` executables in the same Cargo package,**
installed by the existing `cargo install --path .`.

```
src/bin/cpu.rs   →  fn main() { rusttemp::commands::cpu::main() }
src/bin/gpu.rs   →  fn main() { rusttemp::commands::gpu::main() }
src/main.rs      →  rusttemp (unchanged flags) + `rusttemp cpu` / `rusttemp gpu` subcommands
```

Why this approach:
- `cargo install --path .` places `cpu.exe`, `gpu.exe` and `rusttemp.exe` in
  `%USERPROFILE%\.cargo\bin`, which is already on the PATH. No wrapper
  scripts, PowerShell profile edits or aliases are needed.
- It works in cmd, PowerShell, Windows Terminal and Linux shells alike.
- The entry points are a few lines each. All logic lives in the library, so
  nothing is duplicated.
- `rusttemp cpu` / `rusttemp gpu` remain as a fallback if a user already has
  another `cpu` or `gpu` command.

Rejected alternatives:
- PowerShell aliases or functions: these work only in PowerShell and require
  profile edits.
- `.cmd` wrappers: an extra file to keep in sync, with slower startup.

CLI for the new commands (clap):

```
cpu [--watch] [--interval SECONDS] [--json] [--no-color] [--utc]
    [--elevated C --high C --critical C]
gpu [--watch] [--interval SECONDS] [--json] [--no-color] [--utc] [--all]
```

- `--interval` takes **seconds** (decimals allowed, 0.25–3600), as the brief
  specifies. `rusttemp --interval` keeps milliseconds for backward
  compatibility. Both are documented.
- `gpu --all` also lists unclassified and software adapters. By default they
  appear only as a one-line count, so integrated graphics can never be shown
  as dedicated.

### B2. Module structure (evolution of the current code)

```
src/
  main.rs, bin/cpu.rs, bin/gpu.rs     entry points (thin)
  commands/       NEW   cpu.rs, gpu.rs: argument parsing, one-shot vs watch, orchestration
  measurement.rs  NEW   Measurement<T>: Measured{value, unit, source} | Unavailable{reason} | NotApplicable{reason}
  hardware/       NEW   identification only: "what devices exist"
    gpu.rs              GpuDevice {vendor/device/subsys IDs, PCI address, LUID, name, driver, memory, kind, evidence}
    classify.rs         pure integrated/discrete/unknown/software classification with evidence list
    windows.rs          D3DKMT enumeration + SetupAPI (driver version, location)
    linux.rs            /sys/class/drm enumeration (root-relative, mock-testable)
    tables.rs           CPU microarchitecture + GPU device-ID → architecture tables (documented sources)
  cpuinfo/        KEEP  CPU identification (CPUID, Windows, Linux)
  sensors/        KEEP  CPU temperature backends (+ gpu/ below)
    gpu/          NEW   sensor acquisition: "what can be measured"
      mod.rs            GpuSensorProvider trait → GpuSample of Measurement<T> fields
      pdh.rs            Windows vendor-neutral: GPU Engine utilization, adapter memory usage
      igcl.rs           Intel (ControlLib.dll, runtime-loaded, struct sizes asserted)
      rapl.rs           iGPU power from PP1 via PawnIO
      nvml.rs           NVIDIA (nvml-wrapper, feature "nvidia")
      linux_drm.rs      amdgpu / i915 / xe sysfs + hwmon
  perf/           KEEP  CPU speed/load/power (+ CPU voltage from IA32_PERF_STATUS)
  health.rs, monitoring/  KEEP  (watch loop reused by cpu/gpu --watch)
  terminal/, output/      KEEP  + report renderers for the cpu/gpu layouts
```

The brief's proposed tree is used as a guide, adapted to keep working code in
place. Identification (`hardware/`) and acquisition (`sensors/gpu/`) are
separate, as required. Each sensor provider is keyed by the device's
LUID/PCI address and returns only what it actually measured.

**`Measurement<T>`** is the central new type. It makes "unavailable",
"genuinely zero" and "not applicable" different values at compile time, and
it carries the source of every number (e.g. "RAPL PP1 via PawnIO", "NVML",
"PDH GPU Engine"). The terminal shows the reason; JSON emits
`{value, unit, source}` or `{status, reason}`.

### B3. iGPU / dGPU classification rules

Evidence is collected per adapter, and each item is shown to the user
(`--json` / `--all`):

| Signal | Strength | Points to |
|---|---|---|
| `SoftwareDevice` flag or no PCI address | decisive | software renderer (excluded) |
| `HybridIntegrated` flag (set by the driver on hybrid-capable systems) | strong | integrated |
| `HybridDiscrete` flag | strong | discrete |
| `Detachable` flag (external GPU) | strong | discrete |
| Intel vendor **and** PCI 0:2.0 (Intel's fixed integrated-graphics location) | strong | integrated |
| Dedicated memory ≤ 512 MiB **and** shared memory ≥ dedicated | supporting | integrated |
| PCI bus ≠ 0 **and** dedicated memory ≥ 2 GiB | supporting | discrete |
| (Linux) Intel at `0000:00:02.0` | strong | integrated |

Decision:
1. Any decisive signal wins.
2. Otherwise, **integrated** or **discrete** when at least one strong signal
   agrees and none contradicts.
3. Without a strong signal, two agreeing supporting signals are enough.
4. Anything else is **Unknown**.

Manufacturer, model name or memory size are never enough on their own.
Applied to this machine: Intel UHD → integrated (two strong signals, one
supporting); Basic Render → software.

### B4. Dependency evaluation

| Dependency | Decision | Reason |
|---|---|---|
| `windows-sys` (existing) | add features `Wdk_Graphics_Direct3D` and `Win32_Devices_DeviceAndDriverInstallation` | D3DKMT and SetupAPI are plain C APIs, so the COM-heavy `windows` crate isn't needed |
| `nvml-wrapper` 0.13 | **add**, behind a Cargo feature `nvidia` (default on) | Maintained (updated 2026-08), ~8M downloads. Correct NVML struct layouts matter because NVIDIA can't be tested here. Loaded at runtime from an explicit path (`System32\nvml.dll` / `libnvidia-ml.so.1`). Missing NVML means "not available", never a crash. |
| Intel IGCL | **hand-written FFI** (no maintained Rust crate) | Small C API (init / enum / properties / telemetry). Struct sizes asserted against the C header (`ctl_power_telemetry_t` = 1024 bytes, measured with MSVC). Loaded from System32 only. |
| AMD ADLX | **not in this plan** | Large SDK and no AMD hardware for validation. Recorded as a follow-up. |
| `sysinfo`, `wmi` | rejected | They hide data sources (against the source-attribution rule), and WMI's COM adds startup cost |

### B5. Sensor acquisition strategy and resource use

- **One-shot `cpu` / `gpu`:** take a baseline, wait 0.5 s, then sample. Rates
  (utilization, power, frequency) need two readings. Nothing keeps running
  afterwards.
- **`--watch`:** one sample per interval on the main thread. There is no
  background polling; the only extra thread is the Ctrl+C handler. NVML /
  IGCL / PDH handles are opened once and reused.
- **Per-field failure isolation:** every provider call returns
  `Measurement`, so one failing sensor never hides the others or crashes the
  report.
- **Privileges:** the commands run unelevated. Admin-only fields show
  `Unavailable (needs administrator: read from processor registers via
  PawnIO)`.

### B6. Testing strategy

| Layer | How |
|---|---|
| Pure logic | Unit tests: classification (fixtures from Probe A, plus synthetic hybrid-NVIDIA laptop, AMD desktop dGPU, multi-GPU, eGPU, ambiguous APU, software adapter), PERF_STATUS voltage decode, PP1 power, IGCL plausibility checks, `Measurement` serialization, architecture tables |
| Providers | `GpuEnumerator` / `GpuSensorProvider` traits with mock implementations: no iGPU, no dGPU, multiple dGPUs, missing driver, permission denied, partial sensors |
| CLI end-to-end | Run the real `cpu` / `gpu` binaries: `--json` validity, `--watch --interval` with a count, invalid arguments, unsupported platform. A mock DRM tree via `--sysfs-root` gives cross-platform GPU e2e coverage. |
| Real hardware (Windows) | Opt-in diagnostic tests, run elevated (like `diag_reference_clocks`): IGCL vs PP1 vs package; PERF_STATUS voltage vs ratio; PDH vs IGCL utilization under a GPU load; iGPU clock under 3D load |
| Not testable here | NVIDIA / AMD / Arc dGPUs, Linux GPUs. Recorded as hardware-unverified, with a request for community test output. |

### B7. Phase plan

| Phase | Scope | Exit criteria |
|---|---|---|
| 3 CPU | `Measurement` type; CPU voltage; architecture table; power history (sparkline in watch); `cpu` report layout | All CPU fields shown with source or reason; validated elevated on this machine |
| 4 iGPU | D3DKMT enumeration + classification; PDH GPU utilization/memory; IGCL clock/activity/flags; PP1 power; iGPU section in `cpu` | Intel UHD classified with evidence; PDH vs IGCL utilization cross-checked; clock validated under 3D load |
| 5 dGPU | NVML provider; generic PDH provider for AMD/other; multi-GPU; `Dedicated GPU: Not detected.` | Mock-tested (multi-GPU, missing NVML); real "not detected" path verified |
| 6 CLI | `cpu` / `gpu` binaries, `rusttemp cpu|gpu`, `--watch/--interval/--json/--help`, colors | e2e tests; install + PATH verified |
| 7 Validation | Full suites (Windows + Linux in WSL), permission scenarios, unsupported platform | Checklist evidence complete |
| 8 Docs/optimization | README, REFERENCE, dependency review, startup-time check | Docs match behaviour; final audit |

Progress is tracked in [`DEVELOPMENT_CHECKLIST.md`](../DEVELOPMENT_CHECKLIST.md).
