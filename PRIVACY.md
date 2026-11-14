# Privacy policy

RustTemp collects no personal data.

## The programs (`rusttemp`, `cpu`, `gpu`)

- **They read hardware information on your computer only.** That includes the
  CPU and GPU model, temperatures, clocks, load, power and memory use.
- **They make no network connections.** They contain no telemetry, analytics,
  crash reporting or update checks.
- **They store nothing.** No files, settings or logs are written, unless you
  redirect their output yourself (for example `cpu --json > file.json`).

## The installers (`install.ps1`, `install.sh`)

- **The installers connect only to GitHub.** They use `github.com` and
  `raw.githubusercontent.com` to download the release and its checksum.
- **GitHub's own privacy statement applies to those downloads.** See
  <https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement>.
- **On Windows, when run as Administrator, the installer may run `winget`**
  to install the PawnIO driver. The Microsoft and PawnIO terms apply to that
  step.

## Contact

Questions about this policy: open an issue at
<https://github.com/10001111/rusttemp/issues>.
