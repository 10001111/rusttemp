# RustTemp technical reference

This file covers the details behind the [README](../README.md): how sensors
are found, where every number comes from, how the measurements were
validated, and the JSON output format.

## Contents

- [Sensor detection](#sensor-detection)
- [Temperature interpretation](#temperature-interpretation)
- [CPU health details](#cpu-health-details)
- [Speed, load and power: sources and validation](#speed-load-and-power-sources-and-validation)
- [GPU identification and classification](#gpu-identification-and-classification)
- [GPU sensors: sources and validation](#gpu-sensors-sources-and-validation)
- [JSON output](#json-output)
- [Exit codes](#exit-codes)

---

## Sensor detection

### Windows: CPU temperatures (PawnIO)

1. CPUID confirms the CPU is a GenuineIntel processor with a digital thermal
   sensor (leaf 6, bit 0) and a package sensor (bit 6).
2. RustTemp loads `PawnIOLib.dll` by absolute path from the PawnIO install
   folder, never from the search path, so a planted DLL can't be picked up.
3. It loads the embedded, signed `IntelMSR` module. Provenance and hashes are
   in [assets/pawnio/README.md](../assets/pawnio/README.md).
4. Physical cores and packages come from `GetLogicalProcessorInformationEx`.
5. For each core and package, RustTemp pins its thread to one logical CPU,
   reads the thermal status register (`IA32_THERM_STATUS` 0x19C or
   `IA32_PACKAGE_THERM_STATUS` 0x1B1), then restores the thread's affinity.
6. TjMax is read once per core from `MSR_TEMPERATURE_TARGET` (0x1A2).
   Temperature = TjMax − digital readout.

The IntelMSR module only allows reads from a whitelist of registers.
RustTemp never writes to any hardware register.

### Windows: ACPI thermal zones

RustTemp reads the counter
`\Thermal Zone Information(*)\High Precision Temperature`, which is in tenths
of a kelvin. If that counter is missing, it falls back to `\Temperature`
(kelvin). No admin rights are needed. The data is the same as WMI's
`Win32_PerfFormattedData_Counters_ThermalZoneInformation`.

### Linux (sysfs)

- For every `/sys/class/hwmon/hwmonN`, RustTemp reads:
  - `name`
  - each `tempK_input` (millidegrees Celsius)
  - the optional `tempK_label`, `tempK_max` and `tempK_crit`
- Old drivers keep these files in `hwmonN/device/`, which is also checked.
- Each `/sys/class/thermal/thermal_zoneN` with a `temp` file is added. Its
  `critical` trip point is used as the critical limit.
- Zones whose `type` matches an hwmon chip name already found (usually
  `acpitz`) are skipped, because the kernel registers them under both classes.

### CPU identification rules

| Driver (hwmon `name` or zone `type`) | Labels → RustTemp kind |
|---|---|
| `coretemp` (Intel) | `Package id N` / `Physical id N` → CPU package; `Core N` → CPU core |
| `k10temp`, `zenpower` (AMD) | `Tctl` (or unlabelled) → CPU control; `Tdie`, `TccdN` → CPU die |
| thermal zone `x86_pkg_temp` | CPU package |
| `cpu*thermal` (ARM SoCs) | CPU |
| `acpitz` | ACPI thermal zone (never CPU) |
| anything else | other |

A label alone never makes a sensor a CPU sensor. For example, `CPUTIN` on a
motherboard Super-I/O chip stays "other".

Sensors are discovered once at startup. If a sensor stops responding, its
readings become `UNAVAILABLE` and the others keep updating.

## Temperature interpretation

- **Intel DTS readings (Windows)** have 1 °C resolution.
- **TjMax** is shown as the hardware-reported `crit` limit. It is the
  temperature at which the CPU throttles itself.
- **Package vs cores.** Intel's package value tracks the hottest point on the
  die. Expect it to equal, or come within a degree of, the hottest core.
- **Hybrid CPUs.** On the i7-13620H the four E-cores reported identical values
  in every sample, which fits one sensor shared by the E-core cluster.
- **Short spikes** of 10–20 °C when a program starts are normal for Intel CPUs.
- **Linux sysfs** values are in millidegrees, so readings have 0.001 °C
  resolution.
- **AMD Tctl** may carry a model-specific offset above the real die
  temperature. Prefer `Tdie`/`Tccd`.
- **ACPI thermal zones** are defined by the firmware. They often update in
  coarse steps and are not necessarily on the CPU.
- **Invalid values.** Readings outside −55 °C to 150 °C are treated as sentinel
  values and shown as `UNAVAILABLE`.
- **No assumed TjMax.** If TjMax cannot be read, the core is shown as
  `UNAVAILABLE`.
- `CURRENT` shows `--` whenever the latest read failed. Min/avg/max use valid
  readings only.

## CPU health details

| Line | Source | Meaning |
|---|---|---|
| Thermal throttling | status register bit 0 (now) / bit 1 (logged) | The CPU slowed itself down because it reached its throttle temperature. |
| PROCHOT throttling | bits 2 / 3 | Another part of the system, such as the embedded controller or the voltage regulators, asked the CPU to slow down. |
| Critical temperature | bits 4 / 5 | The CPU reached shutdown-level temperature. |
| Power-limit throttling | bits 10 / 11 (only if CPUID leaf 6 EAX bit 4 is set) | Speed is held back by a power limit. |
| Throttle events (Linux) | `/sys/devices/system/cpu/cpu*/thermal_throttle/*_throttle_count` | Number of throttle events since boot. |
| Hottest CPU sensor | current readings + TjMax | Headroom to the CPU's own limit. |
| This session | RustTemp's own sample counts | Samples with active throttling, and the share of time spent at or above each warning level (worst CPU sensor). |

- **"Logged"** flags are sticky. The CPU sets them when the condition occurs
  and they stay set until power-on reset, or until some software clears them.
  RustTemp only reads them.
- **No health score.** CPUs don't report wear or remaining lifespan, so
  RustTemp doesn't show a "health score".

## Speed, load and power: sources and validation

| Value | Windows | Linux |
|---|---|---|
| Load | PDH `\Processor Information(*)\% Processor Time` per logical CPU. Total = mean of the CPUs. | `/proc/stat` |
| Speed (admin) | measured TSC rate × ΔAPERF/ΔMPERF per core | `cpufreq/scaling_cur_freq` |
| Speed (no admin) | PDH `% Processor Performance` × the highest rated MHz (P-core base) | same as above |
| Package / cores power | `MSR_PKG_ENERGY_STATUS` / `MSR_PP0_ENERGY_STATUS` with `MSR_RAPL_POWER_UNIT` | `/sys/class/powercap/intel-rapl:*/energy_uj` (usually root only) |
| PL1 / PL2 | `MSR_PKG_POWER_LIMIT` | powercap `constraint_*_power_limit_uw` (`long_term` / `short_term`) |
| TDP | `MSR_PKG_POWER_INFO` | — |
| Base clock | `CallNtPowerInformation` rated MHz | cpufreq `base_frequency` |
| Max turbo | `MSR_TURBO_RATIO_LIMIT` (read on a P-core) | cpufreq `cpuinfo_max_freq` |

Rates need two readings, so `--once` and `--info` measure over a 0.5 s
window. A core that stayed asleep for the whole window has no measurable
clock and is shown as `idle`.

### Validation on an Intel Core i7-13620H (Windows 11, Memory Integrity on)

The opt-in test `diag_reference_clocks` compares four measurements over
identical 1-second windows:
- the APERF/MPERF ratio
- the Windows `% Processor Performance` value
- the MPERF tick rate
- the TSC rate

Run it from an administrator terminal:

```sh
cargo test --release diag_reference_clocks -- --ignored --nocapture
```

Results, over 12 windows at idle and under full load:

| Measurement | Result | Consequence |
|---|---|---|
| TSC rate | 2918.4 MHz, constant | — |
| MPERF rate on fully busy cores | ≈ 2890–2960 MHz, i.e. the TSC rate | APERF/MPERF is scaled by the measured TSC rate. The 2900 MHz `MSR_PLATFORM_INFO` ratio would be 0.6% low, and the 2400 MHz base far too low. |
| PDH `% Processor Performance` ÷ (100 × APERF/MPERF) | 1.200 (P-cores), 1.208 (E-cores) | Windows uses a ≈2400 MHz reference for **all** cores. Using the E-cores' 1800 MHz rating would be ≈25% wrong. The two speed methods agree within ≈1.5%. |

**Power limits.** On the same machine the CPU registers report
PL1 = PL2 = 82 W, yet under full load the CPU sustained only 18–20 W at about
1.4 GHz and 63 °C. A lower limit was being enforced elsewhere: firmware, the
Windows power mode, or battery operation. RustTemp therefore labels register
limits as "values programmed in the CPU".

**Windows `_Total` load.** Windows' own `_Total` "% Processor Time" instance
sometimes reported 0% over 0.5 s windows while individual CPUs were 20–38%
busy, because it is derived from idle-time accounting. RustTemp computes the
total as the mean of the per-CPU values instead.

**Core voltage** (`cpu`, admin) is `IA32_PERF_STATUS` (0x198) bits 47:32 ×
2⁻¹³ V, read on a P-core. Values outside 0.3–2.0 V are rejected as
implausible. On the i7-13620H it read 0.73–0.94 V across idle and full load.

**Integrated-GPU power** (`cpu`, admin) is the RAPL graphics plane
`MSR_PP1_ENERGY_STATUS` (0x641), converted with the same energy unit as
package power.

## GPU identification and classification

Identification (what each device *is*) is kept separate from sensor reading
(how it is *doing*). It runs once at startup.

| | Windows | Linux |
|---|---|---|
| Enumeration | `D3DKMTEnumAdapters2`. Software adapters are included and classified, never hidden. | `/sys/class/drm/cardN` (connector entries such as `card0-eDP-1` are skipped) |
| Name | `D3DKMTQueryAdapterInfo` (adapter registry info) | Vendor + PCI device ID, e.g. "AMD device 0x73DF". The kernel exposes no marketing name, and no PCI-ID database is bundled. |
| Vendor / device / subsystem / revision IDs | `KMTQAITYPE_PHYSICALADAPTERDEVICEIDS` | `device/vendor`, `device`, `subsystem_vendor`, `subsystem_device`, `revision` |
| PCI location | `KMTQAITYPE_ADAPTERADDRESS` | `PCI_SLOT_NAME` in `device/uevent` |
| Adapter type flags | `KMTQAITYPE_ADAPTERTYPE` (software device, HybridIntegrated, HybridDiscrete, Detachable…) | — |
| Dedicated / shared memory | `KMTQAITYPE_GETSEGMENTSIZE` | `mem_info_vram_total` (amdgpu). NVIDIA: NVML total memory. |
| Driver | SetupAPI display-class enumeration: `DriverVersion`, matched to the adapter by PCI bus/device/function | `DRIVER=` in `device/uevent` (kernel driver name) |
| Architecture | Built-in PCI device-ID table (Intel Raptor Lake-P / -S device IDs, from the Linux kernel's `i915_pciids.h`). For NVIDIA, NVML's architecture is used when the table has no entry. | same |

### Classification rules

Each observation is recorded as evidence with a strength. The decision
order is:

1. A **decisive** signal wins.
2. Otherwise, **strong** signals decide if they all point the same way. Strong
   signals that disagree give *unknown*.
3. Without strong signals, **two supporting** signals that agree, with none
   pointing the other way, decide.
4. Anything else is **unknown**.

| Signal | Strength | Indicates |
|---|---|---|
| Windows reports a software device | decisive | software |
| Microsoft adapter with no PCI location (Basic Render Driver) | decisive | software |
| `HybridIntegrated` adapter flag | strong | integrated |
| `HybridDiscrete` adapter flag | strong | dedicated |
| `Detachable` adapter flag (external GPU) | strong | dedicated |
| Intel device at PCI `00:02.0`, Intel's fixed integrated-graphics slot | strong | integrated |
| ≤ 512 MiB dedicated memory and shared memory ≥ dedicated | supporting | integrated |
| NVIDIA device on an x86 PC | supporting | dedicated |
| PCI bus ≠ 0 with ≥ 2 GiB dedicated memory | supporting | dedicated |

- **Never decided by vendor, name or memory size alone.** A vendor or a
  memory size is at most one supporting signal, and one supporting signal
  never decides.
- **The output lists every signal**, on the "Classified As" line and in
  `classification` in JSON.
- **Unknown devices are never reported as dedicated.** `gpu` shows them
  under "UNCLASSIFIED GPU", and `cpu` counts them in a note.
- **Known consequence:** a desktop AMD card without hybrid flags has only
  supporting signals on bus ≠ 0, and none from its vendor (AMD also makes
  integrated GPUs). With ≥ 2 GiB of VRAM it has exactly one supporting
  signal, so it is shown as unclassified, with its sensors.

On the development machine (Windows 11, i7-13620H) the results were:
- **Intel UHD Graphics:** three signals, all pointing to integrated
  (HybridIntegrated, PCI 0:2.0, 128 MiB dedicated with 8004 MiB shared).
- **Microsoft Basic Render Driver:** software, from the decisive
  software-device flag.

## GPU sensors: sources and validation

Every value is a measurement with its source, or *unavailable* with a
reason, or *not applicable*. When a source can't be read, nothing falls back
to a guessed value.

| Value | Windows | Linux |
|---|---|---|
| Utilization | PDH `\GPU Engine(*)\Utilization Percentage`, summed over processes per engine. Shown: the busiest engine per adapter (the Task Manager method). NVIDIA with NVML: NVML utilization. | amdgpu `gpu_busy_percent`; NVML for NVIDIA. Intel needs the privileged perf interface, so it is unavailable. |
| Dedicated / shared memory used | PDH `\GPU Adapter Memory(*)\Dedicated Usage` / `Shared Usage`. NVIDIA: NVML. | `mem_info_vram_used`, `mem_info_gtt_used` / `_total` |
| Core clock | Intel: IGCL `ctlPowerTelemetryGet` (current GPU frequency). NVIDIA: NVML graphics clock. | i915 `gt_act_freq_mhz` (else `gt_cur_freq_mhz`), xe `tile0/gt0/freq0/act_freq`, amdgpu hwmon `freq1_input`; NVML |
| Memory clock | NVML memory clock | amdgpu hwmon `freq2_input`; NVML |
| Temperature / limit | Intel discrete: IGCL. NVIDIA: NVML GPU temperature and slowdown threshold. | amdgpu hwmon: `junction`, else `edge`, else `temp1`; memory temperature from `mem`. No limit is read (unavailable). NVML for NVIDIA. |
| Power / limit | Intel integrated: RAPL PP1 (admin, see above). NVIDIA: NVML power usage and enforced limit. | amdgpu hwmon `power1_average` (else `power1_input`) and `power1_cap`; NVML |
| Voltage | Intel: IGCL, accepted only within 0.5–1.5 V. NVML exposes no GPU voltage. | amdgpu hwmon `in0_input` |
| Fan | NVML, first fan: RPM (`nvmlDeviceGetFanSpeedRPM`) and percent. 0 fans → not applicable. | amdgpu hwmon `fan1_input`, `pwm1` (÷ 255); NVML |
| Thermal / power throttling | Intel: IGCL `gpuPowerLimited` / temperature-limited flags. NVIDIA: NVML throttle reasons (see below). | NVML |
| Execution units | Intel: IGCL device properties | — |
| Driver version | NVML driver version when present, else SetupAPI | NVML, else kernel driver name |

**Integrated GPUs**
- **Memory clock, memory temperature and fan are not applicable.** An
  integrated GPU uses system memory and the CPU's cooling.
- **GPU temperature is unavailable.** No separate sensor is exposed; the
  graphics share the CPU die, so the CPU package temperature includes them.

**NVML throttle-reason mapping**
- **Power throttling:** `SW_POWER_CAP` (0x4) or `HW_POWER_BRAKE_SLOWDOWN`
  (0x80).
- **Thermal throttling:** `SW_THERMAL_SLOWDOWN` (0x20) or
  `HW_THERMAL_SLOWDOWN` (0x40).
- **Not mapped:** `GPU_IDLE`, application clocks and sync boost. They are
  not throttling.

**Loading NVML:**
- NVML is loaded at run time by explicit path: `%SystemRoot%\System32\nvml.dll`
  on Windows, `libnvidia-ml.so.1` on Linux.
- If it is missing, NVIDIA values are unavailable, with "NVIDIA NVML is not
  available".
- Builds made with `--no-default-features` leave NVML out. They say so in
  the same place.

### Validation (Intel UHD Graphics, i7-13620H, Windows 11)

A D3D11 compute-shader load generator was used to load the iGPU. The tool is
in the development scratch area and not shipped.

| Check | Idle | Under GPU load | Conclusion |
|---|---|---|---|
| PDH busiest-engine vs IGCL activity counter (Δ global activity ÷ Δ timestamp) | 7.6% vs 6.5% | 98.5% vs 95.7% | The two independent methods agree; PDH is used (no driver-specific library needed) |
| IGCL core clock | 100–350 MHz | 1250–1400 MHz | Responds to load as expected (rated max 1500 MHz) |
| RAPL PP1 graphics power | 0.05 W | 10.3–10.8 W | Tracks GPU load; CPU cores power stayed low |
| IGCL power-limited flag | false | true | Reported as power throttling |
| IGCL "GPU energy" counter | = RAPL package energy, to the joule, in every window | | **Not used.** On this iGPU it reports the whole package, not the GPU. |
| IGCL voltage | 0.24–0.42 V | | **Rejected** by the 0.5–1.5 V plausibility check |

Without administrator rights, PP1 power is unavailable and the reason says
so. All other iGPU values above need no admin.

**Not validated on hardware:** no dedicated GPU was available.
- The NVIDIA (NVML), AMD (Windows and Linux) and Intel Arc paths are covered
  by unit tests with synthetic data and mock sysfs trees only.
- The NVML mapping is written against NVML's documented semantics.

## JSON output

| Command | Output |
|---|---|
| `rusttemp --sensors --json` | One document: `{rusttemp_version, backend, sensor_count, cpu_sensor_count, notes, sensors}` |
| `rusttemp --once --json` | One snapshot document |
| `rusttemp --json` | NDJSON: one compact snapshot per line, per sample |
| `rusttemp --info --json` | `{rusttemp_version, cpu, thermal_limit_c, health, perf, notes}` |
| `cpu --json` | One `cpu` report document (below) |
| `gpu --json` | One `gpu` report document (below) |
| `cpu --watch --json` / `gpu --watch --json` | NDJSON: one compact report per line, per refresh |

### Measurement object (`cpu` and `gpu`)

Every value in the `cpu` and `gpu` reports is one of these:

```json
{ "status": "measured", "value": 63.0, "unit": "°C", "source": "intel-dts (PawnIO): Package id 0" }
{ "status": "unavailable", "reason": "needs administrator rights (processor registers via PawnIO)" }
{ "status": "not_applicable", "reason": "uses system memory (no VRAM of its own)" }
```

- Unavailable is never encoded as `0` or `null`.
- `unit` is an empty string for text values.
- Byte counts are integers in bytes.

### `cpu --json`

| Field | Content |
|---|---|
| `rusttemp_version`, `command` (`"cpu"`), `timestamp`, `timestamp_local`, `sample` | Header |
| `thresholds` | `{elevated_c, high_c, critical_c, note}` |
| `identification` | `manufacturer`, `model`, `architecture`, `microarchitecture`, `physical_cores`, `performance_cores`, `efficiency_cores`, `logical_processors`, `base_frequency_mhz`, `max_boost_frequency_mhz` |
| `activity` | `current_frequency_mhz`, `utilization_pct`, `cores: [{label, frequency_mhz, utilization_pct, temperature_c}]` |
| `temperature` | `overall_c` (hottest CPU sensor), `package_c`, `session_max_c`, `limit_c` (TjMax), `sensors` (which sensors were used) |
| `power` | `package_w`, `cores_w`, `voltage_v`, `pl1_w`, `pl2_w`, `tdp_w`, `session: {min, max, avg, samples}`, `history_w` (last 60 package-power samples, oldest first) |
| `throttling` | `thermal`, `power`, `prochot`. Each is a measurement whose value is `{now, since_boot}`; either may be `null` where the platform reports only one. |
| `integrated_gpu` | `{detected, devices: [GPU object], unclassified (count), detection_error}` |
| `notes` | Diagnostics, e.g. why administrator rights are needed |

### `gpu --json`

| Field | Content |
|---|---|
| `rusttemp_version`, `command` (`"gpu"`), `timestamp`, `timestamp_local`, `sample` | Header |
| `dedicated` | Array of GPU objects, one per dedicated GPU (empty: none detected) |
| `unclassified` | GPU objects that could not be confirmed integrated or dedicated |
| `integrated`, `software` | GPU objects; filled only with `--all` |
| `integrated_count`, `software_count` | Counts, always present |
| `detection_error` | Why GPU enumeration failed, or `null` |
| `notes` | Diagnostics |

### GPU object

| Field | Content |
|---|---|
| `kind` | `integrated`, `discrete`, `unknown` or `software` |
| `manufacturer`, `model`, `architecture`, `pci_vendor_id`, `pci_device_id`, `pci_address`, `driver_version` | Identification (measurements) |
| `classification` | `[{signal, strength, indicates}]`: the evidence behind `kind` |
| `utilization_pct`, `core_clock_mhz`, `memory_clock_mhz` | Activity |
| `temperature_c`, `memory_temperature_c`, `temperature_limit_c` | Temperatures |
| `power_w`, `power_limit_w`, `voltage_v` | Power |
| `dedicated_memory_used_bytes`, `dedicated_memory_total_bytes`, `shared_memory_used_bytes`, `shared_memory_total_bytes`, `memory_utilization_pct` | Memory. `memory_utilization_pct` is derived: used ÷ total of shared memory for integrated GPUs, of dedicated memory (VRAM) otherwise. |
| `fan_speed_rpm`, `fan_speed_pct` | Fan |
| `thermal_throttling`, `power_throttling`, `execution_units` | Throttling (booleans) and Intel EU count |

`cpu --json` and `gpu --json` emit each key once per object. A unit test
checks this, after an earlier build repeated `architecture` and
`driver_version`.

### Snapshot fields

| Field | Content |
|---|---|
| `timestamp` / `timestamp_local` / `timestamp_unix_ms` | UTC RFC 3339 / local RFC 3339 with offset (or `null`) / Unix milliseconds |
| `sample`, `monitoring_duration_s` | Sample number and elapsed time |
| `thresholds` | `{elevated_c, high_c, critical_c, note}` |
| `cpu` | `{vendor, brand}` |
| `health` | The CPU HEALTH section as data: `{flags_available, thermal_throttling, prochot, critical_temperature, power_limit_throttling, throttle_counters, hottest_cpu_sensor, session}`. Each condition is `{now: [...], logged: [...]}`, listing sensor names. |
| `perf` | `{cores: [{label, mhz, load_pct}], avg_mhz, total_load_pct, package_w, cores_w, limits: {pl1_w, pl1_enabled, pl2_w, pl2_enabled, tdp_w}, base_mhz, max_turbo_mhz, reference_mhz, speed_source, power_source, speed_unavailable, power_unavailable}` |
| `perf_session` | `{package_w: {min, max, avg, samples}, total_load_pct: {...}, peak_mhz, peak_core}` |
| `notes` | Diagnostics, for example why CPU sensors are unavailable |
| `sensors` | Array of sensor objects (below) |

### Sensor object

```json
{
  "id": "intel-dts/core2", "label": "Core 2 (P-core)", "chip": "intel-dts (PawnIO)",
  "kind": "cpu_core", "kind_description": "CPU core", "is_cpu": true,
  "source": "MSR 0x19C on CPU 0:4 via PawnIO",
  "hardware_limits": { "max_c": null, "crit_c": 100.0 },
  "unit": "celsius", "status": "ok", "temperature_c": 78.0, "error": null,
  "level": "elevated", "headroom_c": 22.0,
  "throttle": { "thermal_now": false, "thermal_logged": true, "prochot_now": false,
                "prochot_logged": true, "critical_now": false, "critical_logged": false,
                "power_limit_now": false, "power_limit_logged": true },
  "stats": { "min_c": 67.0, "max_c": 78.0, "avg_c": 71.667,
             "valid_samples": 3, "unavailable_samples": 0,
             "last_valid_update": "2026-11-12T16:09:10.120Z",
             "level_samples": { "normal": 2, "elevated": 1, "high": 0, "critical": 0 },
             "thermal_throttle_samples": 0 }
}
```

- `kind` is one of `cpu_package`, `cpu_core`, `cpu_die`, `cpu_control`,
  `cpu`, `acpi_thermal_zone` or `other`.
- An unavailable sensor has `status: "unavailable"`, `temperature_c: null`,
  `level: null` and an `error` string.
- In `--info --json`, caches appear as
  `{level, kind, instances: [{size_kib, count}]}`.

## Exit codes

`rusttemp`, `cpu` and `gpu` use the same codes. Missing sensors or GPUs
are not errors: they are reported as unavailable or "Not detected", and the
exit code is 0.

| Code | Meaning |
|---|---|
| 0 | Success. Also used when stdout is closed by the reader, e.g. `rusttemp --json \| head -1`. |
| 1 | Runtime error (e.g. unsupported platform, unreadable `--sysfs-root`) |
| 2 | Invalid command-line input: bad or out-of-range numbers, unknown or conflicting flags, non-increasing thresholds |
