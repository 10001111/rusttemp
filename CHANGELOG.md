# Changelog

All notable changes to RustTemp are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-13

First public release.

### Added

- **`cpu` command** (`rusttemp cpu`). It shows:
  - CPU identification and microarchitecture
  - frequency and utilization, total and per core
  - package and per-core temperatures
  - package power, core voltage, power limits and power history
  - thermal, power and PROCHOT throttling
  - an integrated-GPU section
- **`gpu` command** (`rusttemp gpu`). Each dedicated GPU gets its own
  section, with:
  - utilization and clocks
  - temperature and limit
  - power and limit
  - VRAM and fan
  - throttling

  `--all` also shows integrated GPUs and software renderers.
- **Evidence-based integrated / dedicated GPU classification.** Devices that
  can't be confirmed are shown as unclassified, never as dedicated.
- **GPU sensor sources:**
  - Windows performance counters
  - Intel IGCL
  - NVIDIA NVML (Windows and Linux)
  - Linux DRM sysfs (amdgpu, i915, xe)
- **`--watch`** live mode, refreshing every second by default
  (`--interval` 0.25–3600 s, `--count`). Also `--json`, `--no-color` and
  `--utc`.
- **`rusttemp` live temperature monitor**, with `--info` (CPU identification
  and health) and `--sensors`.
- **Every value states its source.** A value that can't be measured is shown
  as unavailable, with the reason, and never as 0.
- **One-line installers:**
  - Windows (`install.ps1`): checksum-verified download; installs the
    PawnIO driver when run as Administrator
  - Linux (`install.sh`)
  - Windows uninstaller (`uninstall.ps1`)
- **Release downloads:**
  - self-contained Windows x64 programs (static C runtime)
  - static Linux x86_64 programs
- **GitHub Actions** for CI (format, clippy, tests on Windows and Linux,
  MSRV) and for release builds.

[Unreleased]: https://github.com/10001111/rusttemp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/10001111/rusttemp/releases/tag/v0.1.0
