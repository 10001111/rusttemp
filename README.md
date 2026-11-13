# RustTemp

[![CI](https://github.com/10001111/rusttemp/actions/workflows/ci.yml/badge.svg)](https://github.com/10001111/rusttemp/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/10001111/rusttemp)](https://github.com/10001111/rusttemp/releases)
[![License: PolyForm Noncommercial](https://img.shields.io/badge/license-PolyForm%20Noncommercial-blue)](LICENSE.md)
![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange)
![Platforms: Windows | Linux](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux-blue)
![Interface: terminal](https://img.shields.io/badge/interface-terminal-lightgrey)

**A terminal hardware monitor for Windows and Linux, written in Rust.**
Type `cpu` or `gpu` to see what your hardware is and how it is doing:
temperatures, clock speeds, load, power, voltage, memory and throttling.

```
================================================================
                  RUST HARDWARE MONITOR — CPU
================================================================
CPU INFORMATION
----------------------------------------------------------------
Manufacturer:         Intel (GenuineIntel)  [CPUID]
Processor:            13th Gen Intel(R) Core(TM) i7-13620H  [CPUID]
Microarchitecture:    Raptor Lake  [CPUID family 6 / model 186 (0xBA) + model table]
Max Boost Frequency:  4900 MHz  [MSR_TURBO_RATIO_LIMIT via PawnIO]
TEMPERATURE
----------------------------------------------------------------
CPU Temperature:      63.0 °C  [intel-dts (PawnIO): Package id 0]
POWER & VOLTAGE
----------------------------------------------------------------
Package Power:        11.2 W  [Intel RAPL via PawnIO]
Core Voltage:         0.922 V  [IA32_PERF_STATUS via PawnIO]
INTEGRATED GPU
----------------------------------------------------------------
Model:                Intel(R) UHD Graphics  [Windows graphics kernel (D3DKMT)]
GPU Utilization:      5%  [Windows GPU engine counters (busiest engine)]
GPU Temperature:      Unavailable — no separate GPU sensor is exposed; ...
...
```

Every value shows where it came from. Anything that can't be measured says
**Unavailable** and why; it is never shown as 0.

> [!TIP]
> **Install with one command. No Rust, Git or download of this repository
> needed.** Open **PowerShell as Administrator** and paste:
>
> ```powershell
> irm https://raw.githubusercontent.com/10001111/rusttemp/main/install.ps1 | iex
> ```
>
> Then run `rusttemp cpu` or `rusttemp gpu`. See the
> [Quick start](#quick-start-windows) for the three steps.

## Table of contents

- [Features](#features)
- [Quick start (Windows)](#quick-start-windows)
- [Installation](#installation)
  - [Updating and uninstalling](#updating-and-uninstalling)
  - [Build from source](#build-from-source)
- [Usage](#usage)
- [Understanding the output](#understanding-the-output)
- [Configuration](#configuration)
- [Platform and hardware support](#platform-and-hardware-support)
- [Security and privacy](#security-and-privacy)
- [Known limitations](#known-limitations)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

## Features

- **`cpu` command.** One report with:
  - CPU identification: manufacturer, model, architecture, cores, base and
    max boost clock
  - current frequency and utilization, with a per-core table
  - temperatures: package, per core, session maximum
  - package power, core voltage, power limits and power history
  - thermal and power throttling
  - the **integrated GPU**
- **`gpu` command.** Every dedicated GPU, each shown separately:
  - utilization and clocks
  - temperature and its limit
  - power and its limit
  - VRAM used and free
  - fan speed and throttling

  Supported through NVIDIA NVML, Linux `amdgpu` sysfs, and Windows' generic
  GPU counters.
- **Live monitoring every second.** `rusttemp cpu --watch` and
  `rusttemp gpu --watch` refresh every 1000 ms by default; choose another
  rate with `--interval` (0.25–3600 seconds).
- **Careful GPU classification.** Integrated vs dedicated is decided from
  several signals Windows/Linux report, never from the brand name alone. A GPU
  that can't be confirmed either way is shown as *unclassified*, never as
  dedicated.
- **Live temperature monitor (`rusttemp`).** Package and per-core temperatures, with
  current, min, average and max values and the time of each sensor's last
  update.
- **CPU identification** (`--info`): manufacturer, model, family/model/stepping,
  microcode, P-core/E-core split, caches, base and max clock.
- **CPU health.** Shows whether the CPU is throttling now or has throttled
  since boot (thermal, PROCHOT, power limit), how close it is to its own
  temperature limit, and plain-language explanations of each.
- **Speed, load and power.** Per-core clock speed and load, package power in
  watts, and the power limits programmed in the CPU.
- **Configurable warnings.** NORMAL / ELEVATED / HIGH / CRITICAL levels, always
  spelled out in words, not shown only by color.
- **JSON output** for scripts and dashboards. Use `--json` for a stream, or
  `--once --json` for a single reading.
- **Honest data.** RustTemp never invents a reading. Anything it can't read is
  shown as unavailable, with the reason.

## Quick start (Windows)

1. **Open PowerShell as Administrator.** Click Start, type `PowerShell`,
   right-click **Windows PowerShell**, choose **Run as administrator**, and
   click **Yes**.
2. **Install RustTemp.** Paste this line and press Enter:

   ```powershell
   irm https://raw.githubusercontent.com/10001111/rusttemp/main/install.ps1 | iex
   ```

   It downloads the latest release from GitHub and checks its SHA-256
   checksum. It installs the `rusttemp`, `cpu` and `gpu` commands, and the
   PawnIO driver that CPU temperatures need.

3. **Run it**, in the same window:

   ```powershell
   rusttemp cpu            # CPU report: model, temperatures, clocks, power, integrated GPU
   rusttemp gpu            # dedicated GPU report
   rusttemp gpu --watch    # live view, refreshed every second (1000 ms); Ctrl+C to stop
   ```

   `cpu` and `gpu` on their own are shortcuts for the same commands.

**Every time after that:** open PowerShell as Administrator and run
`rusttemp cpu` or `rusttemp gpu`.

> [!NOTE]
> Without administrator rights RustTemp still works: it shows the CPU model,
> load, clocks and GPU usage. It marks CPU temperatures, power and voltage as
> "needs administrator rights" instead of guessing them.

### Quick start (Linux)

```sh
curl -fsSL https://raw.githubusercontent.com/10001111/rusttemp/main/install.sh | sh
rusttemp cpu
```

No driver is needed on Linux. CPU power readings need root:
`sudo ~/.local/bin/rusttemp cpu`.

## Installation

### Windows: one-line installer (recommended)

```powershell
irm https://raw.githubusercontent.com/10001111/rusttemp/main/install.ps1 | iex
```

| Step | What happens |
|---|---|
| Download | The latest `rusttemp-x86_64-pc-windows-msvc.zip` is downloaded from [GitHub Releases](https://github.com/10001111/rusttemp/releases). |
| Verify | Its SHA-256 checksum is checked against the published `.sha256` file. A mismatch stops the installer before anything is installed. |
| Install | `rusttemp.exe`, `cpu.exe` and `gpu.exe` are copied to `C:\Program Files\RustTemp` (as Administrator, for all users) or to `%LOCALAPPDATA%\Programs\RustTemp` (otherwise, for you only). |
| PATH | That folder is added to PATH, so the commands work in any new terminal and in the current window. |
| Driver | As Administrator, the [PawnIO](https://pawnio.eu) driver is installed with `winget` if it is missing. It is signed by Microsoft and is needed for CPU temperatures and power. |

The programs are self-contained: no Visual C++ Redistributable or other
runtime is needed. Requirements: 64-bit Windows 10 or 11.

**Options.** Set these before running the install command:

| Setting | Effect |
|---|---|
| `$env:RUSTTEMP_VERSION = 'v0.1.0'` | Install a specific release instead of the latest |
| `$env:RUSTTEMP_INSTALL_DIR = 'D:\Tools\RustTemp'` | Install to another folder |
| `$env:RUSTTEMP_NO_PAWNIO = '1'` | Don't install the PawnIO driver |
| `$env:RUSTTEMP_NO_MODIFY_PATH = '1'` | Don't change PATH |
| `$env:RUSTTEMP_ZIP = 'C:\Downloads\rusttemp-x86_64-pc-windows-msvc.zip'` | Install from a zip you already downloaded |

> [!TIP]
> **Prefer to read a script before running it?** Download it, open it, then
> run it:
>
> ```powershell
> irm https://raw.githubusercontent.com/10001111/rusttemp/main/install.ps1 -OutFile install.ps1
> notepad install.ps1
> Get-Content .\install.ps1 -Raw | iex
> ```

### Windows: manual download

1. Download `rusttemp-x86_64-pc-windows-msvc.zip` and its `.sha256` file from
   [Releases](https://github.com/10001111/rusttemp/releases).
2. Check the zip: `Get-FileHash .\rusttemp-x86_64-pc-windows-msvc.zip`
   must print the same hash as the `.sha256` file.
3. Extract it anywhere and run `.\rusttemp.exe cpu` from that folder.
4. For CPU temperatures: `winget install namazso.PawnIO`, then use an
   administrator PowerShell.

### Linux

```sh
curl -fsSL https://raw.githubusercontent.com/10001111/rusttemp/main/install.sh | sh
```

- **Install location:** `~/.local/bin`, or `/usr/local/bin` when run as
  root (`curl -fsSL … | sudo sh`).
- **What you get:** static x86_64 programs that run on any distribution.
  The download is checksum-verified like on Windows.
- **Settings:** `RUSTTEMP_VERSION`, `RUSTTEMP_INSTALL_DIR` and
  `RUSTTEMP_TARBALL` work like the Windows settings.
- **NVIDIA:** the static Linux build has no NVIDIA (NVML) support. For NVIDIA
  GPUs on Linux, [build from source](#build-from-source).
- **Manual download:** `rusttemp-x86_64-unknown-linux-musl.tar.gz` from
  [Releases](https://github.com/10001111/rusttemp/releases).

### Updating and uninstalling

**Update:** run the install command again. It replaces the installed
programs with the latest release.

**Uninstall on Windows.** Use an administrator PowerShell if you installed
as Administrator:

```powershell
irm https://raw.githubusercontent.com/10001111/rusttemp/main/uninstall.ps1 | iex
```

- This removes the programs and their PATH entry.
- The PawnIO driver stays installed, because other tools may use it.
  Remove it with `winget uninstall namazso.PawnIO`.

**Uninstall on Linux:**

```sh
rm ~/.local/bin/cpu ~/.local/bin/gpu ~/.local/bin/rusttemp
```

### Build from source

For developers, or for platforms without a ready-made download.

| Requirement | Notes |
|---|---|
| **Rust 1.85 or newer** | Install from <https://rustup.rs>. Verified with 1.85.1 and 1.99.1. |
| **Windows: MSVC build tools** | Visual Studio Build Tools with the *Desktop development with C++* workload, which provides the linker Rust uses. |

Install straight from GitHub (no clone needed). This puts `cpu`, `gpu` and
`rusttemp` in `~/.cargo/bin`:

```sh
cargo install --git https://github.com/10001111/rusttemp
```

Or clone and build:

```sh
git clone https://github.com/10001111/rusttemp
cd rusttemp
cargo build --release            # → target/release/{cpu,gpu,rusttemp}(.exe)
cargo run --release --bin cpu    # run without installing
```

`cargo uninstall rusttemp` removes a Cargo-installed copy.

## Usage

### Run as administrator on Windows

Open Start, type **Terminal** (or **PowerShell** / **cmd**), right-click it,
choose **Run as administrator**, and click **Yes**. Then run any of the
commands below.

Without administrator rights RustTemp still runs. It shows CPU info, load,
approximate clocks and the ACPI temperature zone, plus a note explaining what
needs admin.

On **Linux**, run `rusttemp` as a normal user. Power readings need root. Use
the full path, because `sudo` doesn't search your personal folders:

```sh
sudo ~/.local/bin/rusttemp cpu
```

### The `cpu` and `gpu` commands

| Command | What it does |
|---|---|
| `cpu` | CPU report, including the integrated GPU, then exit |
| `gpu` | Dedicated GPU report (each GPU separately), then exit |
| `gpu --all` | Also show integrated GPUs and software renderers, under their own headings |
| `cpu --watch` / `gpu --watch` | Keep refreshing every **1 second (1000 ms)** until **Ctrl+C** |
| `cpu --watch --interval 2` | Refresh every 2 **seconds** (0.25–3600, decimals allowed) |
| `cpu --watch --count 10` | Stop after 10 refreshes |
| `cpu --json` / `gpu --json` | JSON: one document, or one compact document per line with `--watch` |
| `cpu --elevated C --high C --critical C` | Temperature colour thresholds in °C |
| `--no-color`, `--utc` | Plain text; UTC times |
| `cpu --help` / `gpu --help` | Command-specific help |
| `rusttemp cpu …` / `rusttemp gpu …` | The same commands, if `cpu`/`gpu` clash with another program |

`cpu --interval` and `gpu --interval` are in seconds. The older
`rusttemp --interval` keeps milliseconds, so existing scripts don't break.

### The `rusttemp` command

| Command | What it does |
|---|---|
| `rusttemp` | Live monitor. Refreshes every second; press **Ctrl+C** to stop. Prints a summary when it exits. |
| `rusttemp --info` | CPU information, speed and power, and health, with explanations |
| `rusttemp --once` | Take one reading, then exit |
| `rusttemp --sensors` | List every sensor found and how it was identified |
| `rusttemp --interval 2000` | Refresh interval in milliseconds (100–3 600 000; default 1000) |
| `rusttemp --count N` | Stop the monitor after N readings |
| `rusttemp --json` | Machine-readable output (see [docs/REFERENCE.md](docs/REFERENCE.md#json-output)) |
| `rusttemp --elevated C --high C --critical C` | Set the warning thresholds in °C (see [Configuration](#configuration)) |
| `rusttemp --utc` | Show times in UTC instead of local time |
| `rusttemp --no-color` | Plain text without colors (the `NO_COLOR` environment variable also works) |
| `rusttemp --sysfs-root PATH` | Read a copied Linux sensor tree, for diagnostics and testing |
| `rusttemp --help` / `--version` | Show help / version |

### Examples

```sh
cpu                                     # full CPU + integrated GPU report
gpu --watch --interval 2                # dedicated GPUs, refreshing every 2 s
cpu --json > cpu.json                   # one report as JSON
cpu --watch --count 60 --json > log.ndjson   # record 60 readings
rusttemp --info                         # identify the CPU and check its health
rusttemp --interval 2000                # monitor, refreshing every 2 seconds
rusttemp --count 60 --json > log.ndjson # record 60 readings to a file
rusttemp --once --json                  # one reading as JSON, for scripts
rusttemp --elevated 80 --high 90 --critical 98
```

## Understanding the output

### `cpu`

Sections, in order:
1. **CPU INFORMATION**: manufacturer, model, architecture, cores, base / max
   boost clock, current frequency, utilization.
2. **TEMPERATURE**: overall (package), session maximum, the CPU's own limit
   (TjMax).
3. **PER-CORE**: frequency, load and temperature for every core. A core that
   was asleep shows `idle`.
4. **POWER & VOLTAGE**: package and cores power, core voltage, power limits,
   power history (a sparkline in `--watch`).
5. **THROTTLING**: thermal, power and PROCHOT throttling, now and since boot.
6. **INTEGRATED GPU**: model, architecture, utilization, clock, power, shared
   and dedicated memory. If there is none, it prints exactly
   `Integrated GPU: Not detected.`

Each line ends with its source in brackets, for example
`[Intel RAPL via PawnIO]`.

- **`Not applicable`** means the value doesn't exist for that device. For
  example, an integrated GPU has no memory clock of its own.
- **`Unavailable`** means it exists but couldn't be read here, and the
  reason follows.

### `gpu`

- Each dedicated GPU gets its own **DEDICATED GPU** section.
- With several GPUs, the sections are titled "DEDICATED GPU 1 OF 2" and so
  on. Values are never merged.
- With no dedicated GPU, it prints exactly `Dedicated GPU: Not detected.`
- A GPU that RustTemp can't confirm as integrated or dedicated appears under
  **UNCLASSIFIED GPU**, with the evidence it found.

What each GPU value means:

| Line | Meaning |
|---|---|
| GPU Utilization | Busiest engine (3D, video, copy…), as Task Manager shows it |
| Core / Memory Frequency | Current GPU and VRAM clock |
| GPU Temperature / Temperature Limit | Current temperature and the slowdown threshold reported by the driver |
| GPU Power / Power Limit | Current board power and the enforced limit |
| VRAM | Dedicated memory used, total and free |
| Fan Speed | RPM and percent. Laptop GPUs often have no fan of their own; that is shown as *not applicable*. |
| Thermal / Power Throttling | The GPU is currently slowing down because of heat or its power limit |
| Classified As | Why RustTemp decided the GPU is integrated or dedicated |

### `rusttemp` live monitor

### Live monitor

- **CPU SENSORS** lists package and core temperatures. **OTHER SENSORS**
  lists everything not proven to be on the CPU, such as the laptop's ACPI
  thermal zone or NVMe drives.
- **UPDATED** is the time of each sensor's last valid reading. Times are
  local, with the UTC offset shown in the header.
- **STATUS** shows the warning level. `UNAVAILABLE: <reason>` means the value
  couldn't be read; RustTemp never guesses one.

### CPU information and health

```
CPU HEALTH
  Thermal throttling      now: no · logged: YES (all CPU sensors)
  PROCHOT throttling      now: no · logged: YES (all CPU sensors)
  Critical temperature    now: no · logged: no
  Power-limit throttling  now: no · logged: YES (all CPU sensors)
  Hottest CPU sensor      Package id 0 at 63.0 °C · 37.0 °C below its 100.0 °C limit
```

| Line | What it means |
|---|---|
| Thermal throttling | The CPU slowed itself down because it got too hot. Occasional events under heavy load are common on laptops. Throttling during light use suggests a cooling problem (dust, fan, thermal paste). |
| PROCHOT throttling | Another part of the system (e.g. the laptop's embedded controller or power circuitry) told the CPU to slow down. |
| Critical temperature | The CPU reached shutdown-level temperature. This should never happen; if it's logged, check the cooling. |
| Power-limit throttling | Speed is held back by a power limit. This is expected on laptops, especially on battery or in quiet modes, and is not a fault. |
| Hottest CPU sensor | Degrees left before the CPU's own temperature limit (TjMax). |

- **now** means the condition is happening right now.
- **logged** means the CPU has recorded it at least once since power-on (or
  since some software cleared the flag).

CPUs don't report wear or remaining lifespan, so RustTemp shows these facts
rather than an invented "health score".

### CPU speed and power

```
CPU SPEED & POWER
  Package power           18.1 W (cores 11.7 W)
  Power limits            PL1 82.0 W · PL2 82.0 W · TDP 45.0 W
  Clock                   avg 1450 MHz · base 2400 MHz · max turbo 4900 MHz
  CPU load                100%
```

- **Package power** is what the whole CPU is drawing right now.
- **Power limits** are the values programmed in the CPU. Firmware, the
  Windows power mode or running on battery can enforce **lower** limits.
- **Clock** shows the average speed of the cores. A core that was asleep for
  the whole measurement shows `idle`.

How every value is measured, and how the measurements were validated, is
documented in [docs/REFERENCE.md](docs/REFERENCE.md).

## Configuration

### Warning thresholds

| Level | Default | Option |
|---|---|---|
| NORMAL | below 70 °C | — |
| ELEVATED | 70 °C or more | `--elevated` |
| HIGH | 85 °C or more | `--high` |
| CRITICAL | 95 °C or more | `--critical` |

> [!WARNING]
> These are **generic** thresholds, **not** safety limits for your processor.
> The safe maximum differs between CPU models. Check your CPU's own limit with
> `rusttemp --info` ("Thermal limit") and set the thresholds to suit it. For
> example:
>
> ```sh
> rusttemp --elevated 80 --high 90 --critical 98
> ```

Values must be strictly increasing (elevated < high < critical).

### Time zone and colors

- Times are shown in local time by default. Add `--utc` for UTC. JSON always
  includes both.
- Colors are used only in an interactive terminal. Turn them off with
  `--no-color` or the `NO_COLOR` environment variable.

## Platform and hardware support

| Platform | Temperatures | Health flags | Speed / load / power | Status |
|---|---|---|---|---|
| **Windows 10/11, Intel** | Package + every core (PawnIO, admin) and ACPI zone | Yes (PawnIO, admin) | Yes (load and approximate speed without admin) | **Verified** on a Core i7-13620H |
| Windows, AMD | ACPI zone only | No | Load and approximate speed only | CPU sensors not implemented yet |
| **Linux** | `coretemp`, `k10temp`, `zenpower`, `x86_pkg_temp`, `cpu*thermal`, plus all other hwmon sensors | Kernel throttle counters | Yes (power needs root) | Tested with simulated sensor data. **Not yet verified on real hardware.** |
| macOS | — | — | — | Not supported |

**GPUs**

| GPU | Identification | Sensors | Status |
|---|---|---|---|
| **Intel integrated** (Windows) | Windows graphics kernel + PCI | Utilization, memory (no admin). Clock, throttle flags, execution units (Intel IGCL, no admin). Power from the CPU's graphics plane (admin + PawnIO). | **Verified** on Intel UHD Graphics (Raptor Lake-P) |
| **NVIDIA** (Windows, Linux) | Windows graphics kernel / DRM + NVML | NVML: utilization, clocks, temperature, power and limit, VRAM, fan, throttling, driver version. NVML exposes no GPU voltage. | Implemented; **hardware-unverified** (no NVIDIA GPU was available) |
| **AMD** (Windows) | Windows graphics kernel + PCI | Utilization and memory only; AMD vendor sensors are not implemented yet | Partial; hardware-unverified |
| **AMD** (Linux, `amdgpu`) | DRM sysfs | Utilization, temperatures, power and cap, fan, clocks, voltage, VRAM | Implemented; tested with simulated data only |
| **Intel** (Linux, `i915` / `xe`) | DRM sysfs | Clock only (utilization needs the privileged perf interface) | Implemented; tested with simulated data only |

Inside virtual machines (including WSL), CPU sensors are usually not
available, and `--info` describes the virtual CPU. RustTemp says so when it
detects a VM.

## Security and privacy

- **Read-only.** RustTemp reads sensors and CPU registers. It never writes to
  any hardware register or changes any system setting.
- **No network access and no telemetry.** The programs never connect to
  the internet. Only the installer does, to download the release from
  github.com and check its checksum.
- **Driver.** On Windows, privileged reads go through the PawnIO driver
  (signed by Microsoft WHCP). RustTemp loads PawnIO's library only from its
  installation folder. The bundled PawnIO module only allows reading a fixed
  list of registers. Its source and checksums are in
  [assets/pawnio/README.md](assets/pawnio/README.md).
- Administrator rights are only needed for those register reads.

## Known limitations

- **Windows CPU temperatures, throttle flags, power and exact clocks** need
  PawnIO and an administrator terminal. They are Intel-only for now.
- **Power limits** shown are the CPU's register values. On the test laptop
  the registers said 82 W while the laptop actually held the CPU to about
  20 W.
- **Linux CPU sensors haven't been verified on real hardware yet.** WSL2
  exposes no sensors.
- **Max clock without admin** may show as unavailable on Windows when Memory
  Integrity (VBS) is on. With admin + PawnIO it is read from the CPU.
- **"Logged" throttle flags** can be cleared by other monitoring software.
- **Readings were not cross-checked against another monitoring tool** during
  development. Comparing with HWiNFO or LibreHardwareMonitor is recommended.
- **GPUs:**
  - Only Intel integrated graphics has been verified on real hardware. The
    NVIDIA (NVML), AMD and Linux GPU paths are tested with simulated data
    only.
  - Integrated GPUs have no separate temperature sensor; the CPU package
    temperature covers them.
  - Intel's driver-reported iGPU voltage failed a plausibility check, so it
    is not shown.
  - Desktop AMD GPUs without hybrid-graphics flags are shown as
    *unclassified*, because AMD also makes integrated GPUs.
- **Static Linux builds** (musl) can't load NVIDIA's NVML library; build them
  with `--no-default-features` (see [Development](#development)).
- Sensors and GPUs are discovered once, at startup.
- No macOS support.

## Troubleshooting

| Problem | Solution |
|---|---|
| `'cpu'` / `'gpu'` / `'rusttemp'` *is not recognized* / `command not found` | Open a **new** terminal after installing. If it still fails, run the install command again and read its messages. The install folder must be on your `PATH`: `C:\Program Files\RustTemp`, `%LOCALAPPDATA%\Programs\RustTemp` or `~/.local/bin`. |
| The installer says *"Download failed"* | Check your internet connection. Corporate proxies can block GitHub downloads; use the [manual download](#windows-manual-download) instead. |
| The installer says *"Checksum mismatch"* | The download was corrupted or altered, so nothing was installed. Run it again; if it keeps failing, please open an issue. |
| *"running scripts is disabled on this system"* | That only affects `.\install.ps1` run as a file. Use the `irm … \| iex` command, which works with any execution policy. |
| `cpu` or `gpu` runs a different program | Another tool uses the same name earlier on your `PATH`. Use `rusttemp cpu` / `rusttemp gpu` instead. |
| `gpu` says *"NVIDIA NVML is not available"* | Install or update the NVIDIA driver; NVML ships with it. |
| Building from source fails with `link.exe not found` | Install Visual Studio Build Tools with *Desktop development with C++*. |
| Note: *"PawnIO access denied: run RustTemp from an administrator terminal"* | Open the terminal with *Run as administrator*. |
| Note: *"PawnIO is not installed"* | `winget install namazso.PawnIO`, then open a new administrator terminal. |
| Note: *"PawnIO driver is not running"* | Reinstall PawnIO or reboot. Check with `sc query PawnIO`. |
| Only the ACPI zone is shown on an AMD PC | Expected: AMD CPU sensors on Windows are not implemented yet. |
| *"No temperature sensors were found"* (Linux) | Load the driver: `sudo modprobe coretemp` (Intel) or `sudo modprobe k10temp` (AMD). VMs and WSL have no sensors. |
| `°C` shows as `Â°C` when piping in Windows PowerShell 5.1 | Run `[Console]::OutputEncoding = [Text.Encoding]::UTF8` first, or use Windows Terminal / PowerShell 7. |
| Strange characters or escape codes | Use `--no-color`. |
| *"thermal zone performance counters are unavailable"* | Rebuild Windows' counters with `lodctr /R` from an administrator prompt. |

## Development

### Build

```sh
cargo build --release        # → target/release/{cpu,gpu,rusttemp}(.exe)
cargo build --release --no-default-features   # without NVIDIA (NVML) support
```

### Test and lint

```sh
cargo test                                  # unit + end-to-end tests (no real sensors needed)
cargo clippy --all-targets -- -D warnings   # lints, warnings treated as errors
cargo fmt --check                           # formatting
```

The tests run the real program against simulated Linux sensor folders, so
they pass on any machine. On Windows a few tests also use the real OS. Run
`cargo test` from an administrator terminal to exercise the PawnIO code
paths.

### Cross-compile for Linux (from Windows or macOS)

```sh
rustup target add x86_64-unknown-linux-musl
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
  cargo build --release --target x86_64-unknown-linux-musl --no-default-features
```

The result is a static binary with no C toolchain needed.
`--no-default-features` leaves out NVIDIA support, because a static binary
can't load NVIDIA's NVML library. Native Linux builds (`cargo build` on
Linux) include it. Running the test
binaries from `target/x86_64-unknown-linux-musl/debug/deps/` inside WSL runs
the Linux test suite.

### Releasing

1. Update the version in `Cargo.toml` and add an entry to
   [CHANGELOG.md](CHANGELOG.md).
2. Commit, then tag and push:
   `git tag v0.1.0 && git push origin main v0.1.0`.
3. The [release workflow](.github/workflows/release.yml) then:
   - runs the tests
   - builds the Windows and static Linux programs
   - packages them with the licence files and checksums
   - publishes a GitHub release

   The installers always download the latest release.

### Project layout

```
src/
  bin/cpu.rs, bin/gpu.rs      the `cpu` and `gpu` executables (thin entry points)
  commands/                   `cpu` / `gpu` argument parsing and orchestration
  report/                     cpu / gpu report data (Measurement values with sources)
  measurement.rs              measured / unavailable / not-applicable values
  hardware/                   identification: GPU enumeration + classification, model tables
  sensors/gpu/                GPU sensors: Windows counters, Intel IGCL, NVIDIA NVML, Linux DRM
  main.rs, app.rs, cli.rs     `rusttemp` entry point and options
  sensors/                    temperature sensors (Linux sysfs, Windows PawnIO + ACPI)
  cpuinfo/                    CPU identification (CPUID, Windows APIs, /proc + sysfs)
  perf/                       speed, load and power
  health.rs                   throttling and headroom summary
  monitoring/                 sampling loop, statistics, thresholds
  terminal/, output/          text and JSON output
tests/                        end-to-end tests (rusttemp, cpu, gpu)
install.ps1, uninstall.ps1    Windows installer / uninstaller (downloads a GitHub release)
install.sh                    Linux installer
scripts/                      release helpers (third-party licence notices)
.github/workflows/            CI (tests, lint) and release builds
assets/pawnio/                bundled PawnIO module (LGPL-2.1) and its provenance
docs/                         technical reference, design notes, status, reports
```

### Further documentation

| Document | Contents |
|---|---|
| [docs/REFERENCE.md](docs/REFERENCE.md) | How sensors are detected, where every value comes from, measurement validation, JSON format, exit codes |
| [docs/DISCOVERY.md](docs/DISCOVERY.md) | Original research, architecture and dependency decisions |
| [docs/HARDWARE_MONITOR_PLAN.md](docs/HARDWARE_MONITOR_PLAN.md) | `cpu` / `gpu` feasibility report, probe results and architecture |
| [DEVELOPMENT_CHECKLIST.md](DEVELOPMENT_CHECKLIST.md) | `cpu` / `gpu` implementation checklist with verification evidence |
| [docs/PROGRESS.md](docs/PROGRESS.md) | Requirement status with evidence |
| [docs/REPORT.md](docs/REPORT.md) | Implementation and validation reports per revision |

## Contributing

Contributions are welcome. Before submitting a change:

1. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
   `cargo test`. All three should pass.
2. Add tests for new behaviour. Sensor logic can be tested with simulated
   folders (see the existing tests in `src/sensors/platform/sysfs.rs` and
   `tests/cli.rs`).
3. Keep RustTemp's main rule: **never show a value that wasn't actually
   measured.** Report missing data as unavailable, with a reason.
4. Don't claim hardware support you haven't verified. If you test RustTemp
   on new hardware, especially Linux machines, AMD CPUs or NVIDIA/AMD GPUs,
   please share the output of `cpu --json`, `gpu --all --json` and
   `rusttemp --sensors --json`.
5. Update the README and [docs/PROGRESS.md](docs/PROGRESS.md) when behaviour
   changes.

By contributing, you agree that your contribution is licensed under the same
licence as the project (PolyForm Noncommercial 1.0.0).

Security problems: please follow [SECURITY.md](SECURITY.md) instead of
opening a public issue.

## License

RustTemp is licensed under the
[PolyForm Noncommercial License 1.0.0](LICENSE.md).

- **You may:** use, copy, modify and share RustTemp for free, for any
  noncommercial purpose. This covers personal use, study, hobby projects,
  and use by charities, schools, public research and government
  institutions.
- **You may not:** sell RustTemp, or use it for commercial purposes. That
  includes selling copies or modified versions on app stores, marketplaces or
  any other third-party platform, bundling it into a paid product, or offering
  it as a paid service.

The licence requires no copyright notice to be displayed. Anyone who passes
RustTemp on must include the licence terms (or their URL). This summary is
only a convenience; the [licence text](LICENSE.md) is what applies.

**Third-party components** keep their own licences:
- **PawnIO module:** the Windows programs embed the PawnIO `IntelMSR` module
  (`assets/pawnio/IntelMSR.bin`), licensed under LGPL-2.1-or-later. See
  [assets/pawnio/COPYING](assets/pawnio/COPYING) and
  [assets/pawnio/README.md](assets/pawnio/README.md).
- **Rust crates:** the crates RustTemp is built from are listed, with their
  licence texts, in `THIRD_PARTY_LICENSES.md` inside every release download.
