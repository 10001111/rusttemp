# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report them privately through GitHub instead: open the repository's
**Security** tab and choose **Report a vulnerability**.

Please include:
- what you found
- how to reproduce it
- the RustTemp version (`rusttemp --version`)

You should get a reply within a few days.

## Supported versions

Only the latest release receives fixes.

## How RustTemp handles privileges

RustTemp reads hardware sensors; on Windows some of them need administrator
rights, so it is worth knowing exactly what it does:

- **Read-only.** RustTemp never writes to hardware registers and never changes
  system settings.
- **Kernel access goes through PawnIO**, a driver signed by Microsoft's
  Windows Hardware Compatibility Publisher. RustTemp loads
  `PawnIOLib.dll` only by absolute path from `C:\Program Files\PawnIO`, never
  from the search path.
- **The bundled PawnIO module (`IntelMSR.bin`) only reads a fixed list of
  processor registers.** It is the unmodified official module. Its SHA-256
  and provenance are recorded in
  [assets/pawnio/README.md](assets/pawnio/README.md), so anyone can compare
  it with the official PawnIO.Modules release.
- **The programs make no network connections** and collect no telemetry.
- **The installers** download releases only from this repository's GitHub
  Releases over HTTPS. They verify each download's SHA-256 checksum before
  installing anything.

If you find a way to make RustTemp write to hardware, run code with elevated
rights, or load a library from an unexpected location, please report it as
described above.
