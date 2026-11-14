# RustTemp installer for Windows (x64).
#
# Open PowerShell as Administrator, then run:
#
#     irm https://raw.githubusercontent.com/10001111/rusttemp/main/install.ps1 | iex
#
# What it does:
#   1. Downloads the latest RustTemp release from GitHub and verifies its
#      SHA-256 checksum.
#   2. Installs cpu.exe, gpu.exe and rusttemp.exe and adds them to PATH:
#        as Administrator:   C:\Program Files\RustTemp        (all users)
#        otherwise:          %LOCALAPPDATA%\Programs\RustTemp (current user)
#   3. As Administrator: installs the PawnIO driver with winget if it is
#      missing. CPU temperatures and power need it.
#
# Optional settings (set them before running the command above):
#   $env:RUSTTEMP_VERSION = 'v0.1.0'     install a specific release (default: latest)
#   $env:RUSTTEMP_INSTALL_DIR = 'D:\Tools\RustTemp'   install somewhere else
#   $env:RUSTTEMP_NO_PAWNIO = '1'        do not install the PawnIO driver
#   $env:RUSTTEMP_NO_MODIFY_PATH = '1'   do not change PATH
#   $env:RUSTTEMP_ZIP = 'C:\path\rusttemp-x86_64-pc-windows-msvc.zip'
#                                        install from a downloaded zip; a
#                                        matching .sha256 file next to it is
#                                        verified when present
#
# Uninstall:
#     irm https://raw.githubusercontent.com/10001111/rusttemp/main/uninstall.ps1 | iex
#
# Works with Windows PowerShell 5.1 and PowerShell 7. It never calls `exit`,
# so it is safe to run with `iex` in an open window.

& {
    Set-StrictMode -Version 2
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # PowerShell 5.1 downloads are very slow with the progress bar

    $Repo = '10001111/rusttemp'
    $Asset = 'rusttemp-x86_64-pc-windows-msvc.zip'
    $Programs = @('cpu.exe', 'gpu.exe', 'rusttemp.exe')

    function Write-Step([string]$Text) { Write-Host "==> $Text" -ForegroundColor Cyan }
    function Write-Note([string]$Text) { Write-Host "    $Text" }
    function Write-Warn([string]$Text) { Write-Host "    WARNING: $Text" -ForegroundColor Yellow }

    function Test-Flag([string]$Name) {
        $v = [Environment]::GetEnvironmentVariable($Name)
        return ($null -ne $v) -and ($v -ne '') -and ($v -ne '0')
    }

    # PATH is read and written as raw registry values so that entries such as
    # %SystemRoot%\system32 keep their variables (REG_EXPAND_SZ).
    function Get-EnvKey([string]$Scope) {
        if ($Scope -eq 'Machine') {
            return [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\Session Manager\Environment', $true)
        }
        return [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    }

    function Add-ToPath([string]$Dir, [string]$Scope) {
        $key = Get-EnvKey $Scope
        try {
            $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $entries = @($raw -split ';' | Where-Object { $_ -ne '' })
            $present = @($entries | Where-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $Dir.TrimEnd('\') })
            if ($present.Count -gt 0) { return $false }
            $key.SetValue('Path', (($entries + $Dir) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
        } finally {
            $key.Close()
        }
        # Setting and clearing a variable through .NET broadcasts the
        # "environment changed" message, so newly opened terminals see the
        # new PATH without signing out.
        [Environment]::SetEnvironmentVariable('RUSTTEMP_PATH_REFRESH', '1', 'User')
        [Environment]::SetEnvironmentVariable('RUSTTEMP_PATH_REFRESH', $null, 'User')
        return $true
    }

    function Get-ExpectedHash([string]$ShaFile) {
        $text = (Get-Content -LiteralPath $ShaFile -Raw).Trim()
        $hash = ($text -split '\s+')[0].ToLowerInvariant()
        if ($hash -notmatch '^[0-9a-f]{64}$') { throw "The checksum file $ShaFile is not a SHA-256 checksum." }
        return $hash
    }

    try {
        Write-Host ''
        Write-Host 'RustTemp installer' -ForegroundColor Green
        Write-Host ''

        if (-not [Environment]::Is64BitOperatingSystem) {
            throw 'RustTemp needs 64-bit Windows 10 or 11.'
        }
        $arch = $env:PROCESSOR_ARCHITEW6432
        if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
        if ($arch -eq 'ARM64') {
            Write-Warn 'This is an ARM64 PC. RustTemp is an x64 program and runs under emulation; CPU temperature sensors are not supported on ARM.'
        }

        $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
        $isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

        $programFiles = $env:ProgramW6432
        if (-not $programFiles) { $programFiles = $env:ProgramFiles }

        $installDir = $env:RUSTTEMP_INSTALL_DIR
        if (-not $installDir) {
            if ($isAdmin) {
                $installDir = Join-Path $programFiles 'RustTemp'
            } else {
                $installDir = Join-Path $env:LOCALAPPDATA 'Programs\RustTemp'
            }
        }
        $installDir = [IO.Path]::GetFullPath($installDir)
        $pathScope = 'User'
        if ($isAdmin) { $pathScope = 'Machine' }

        if ($isAdmin) {
            Write-Note 'Running as Administrator: installing for all users.'
        } else {
            Write-Warn 'Not running as Administrator. RustTemp will be installed for your user only, and the PawnIO driver (needed for CPU temperatures) will not be installed. For the full setup, close this window, open PowerShell with "Run as administrator" and run the command again.'
        }

        $work = Join-Path ([IO.Path]::GetTempPath()) ('rusttemp-install-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $work | Out-Null
        try {
            # ---- 1. Get the release zip and verify it.
            $zip = Join-Path $work $Asset
            if ($env:RUSTTEMP_ZIP) {
                Write-Step "Using local file $($env:RUSTTEMP_ZIP)"
                Copy-Item -LiteralPath $env:RUSTTEMP_ZIP -Destination $zip
                $localSha = $env:RUSTTEMP_ZIP + '.sha256'
                if (Test-Path -LiteralPath $localSha) {
                    $expected = Get-ExpectedHash $localSha
                } else {
                    $expected = $null
                    Write-Warn 'No .sha256 file next to the zip; the download cannot be verified.'
                }
            } else {
                [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
                if ($env:RUSTTEMP_VERSION) {
                    $base = "https://github.com/$Repo/releases/download/$($env:RUSTTEMP_VERSION)"
                } else {
                    $base = "https://github.com/$Repo/releases/latest/download"
                }
                Write-Step "Downloading $Asset"
                Write-Note "from $base"
                try {
                    Invoke-WebRequest -Uri "$base/$Asset" -OutFile $zip -UseBasicParsing
                    Invoke-WebRequest -Uri "$base/$Asset.sha256" -OutFile "$zip.sha256" -UseBasicParsing
                } catch {
                    throw "Download failed: $($_.Exception.Message). Check your internet connection, or see https://github.com/$Repo/releases"
                }
                $expected = Get-ExpectedHash "$zip.sha256"
            }

            if ($expected) {
                Write-Step 'Verifying SHA-256 checksum'
                $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($actual -ne $expected) {
                    throw "Checksum mismatch: expected $expected, got $actual. The file was not installed."
                }
                Write-Note 'OK'
            }

            $extract = Join-Path $work 'files'
            Expand-Archive -LiteralPath $zip -DestinationPath $extract -Force
            $main = Get-ChildItem -LiteralPath $extract -Recurse -Filter 'rusttemp.exe' | Select-Object -First 1
            if (-not $main) { throw 'The release zip does not contain rusttemp.exe.' }
            $payload = $main.DirectoryName
            foreach ($p in $Programs) {
                if (-not (Test-Path -LiteralPath (Join-Path $payload $p))) { throw "The release zip does not contain $p." }
            }

            # ---- 2. Install the files.
            Write-Step "Installing to $installDir"
            New-Item -ItemType Directory -Path $installDir -Force | Out-Null
            try {
                Get-ChildItem -LiteralPath $payload | Copy-Item -Destination $installDir -Recurse -Force
            } catch [System.UnauthorizedAccessException] {
                throw "No permission to write to $installDir. Run PowerShell as Administrator, or set `$env:RUSTTEMP_INSTALL_DIR to a folder you own."
            } catch [System.IO.IOException] {
                throw "Could not replace the files in $installDir. Close any window that is running cpu, gpu or rusttemp, then run the installer again."
            }
            $version = (& (Join-Path $installDir 'rusttemp.exe') --version) -join ' '
            Write-Note "Installed $version"

            if (Test-Flag 'RUSTTEMP_NO_MODIFY_PATH') {
                Write-Note 'PATH not changed (RUSTTEMP_NO_MODIFY_PATH is set).'
            } else {
                if (Add-ToPath $installDir $pathScope) {
                    Write-Note "Added $installDir to the $($pathScope.ToLower()) PATH."
                } else {
                    Write-Note "$installDir is already on the $($pathScope.ToLower()) PATH."
                }
                # Make the commands work in this window straight away.
                $sessionEntries = @($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $installDir.TrimEnd('\') })
                if ($sessionEntries.Count -eq 0) { $env:Path = $env:Path.TrimEnd(';') + ';' + $installDir }

                foreach ($name in @('cpu', 'gpu', 'rusttemp')) {
                    $found = Get-Command $name -ErrorAction SilentlyContinue | Select-Object -First 1
                    if ($found -and $found.CommandType -eq 'Application') {
                        $dir = Split-Path -Parent $found.Source
                        if ($dir.TrimEnd('\') -ine $installDir.TrimEnd('\')) {
                            Write-Warn "'$name' currently runs $($found.Source), which comes earlier on PATH. Use 'rusttemp $name' instead, or remove the other copy (for an old Cargo install: cargo uninstall rusttemp)."
                        }
                    } elseif ($found) {
                        Write-Warn "'$name' is also a PowerShell $($found.CommandType.ToString().ToLower()). Use '$name.exe' or 'rusttemp $name' instead."
                    }
                }
            }
        } finally {
            Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
        }

        # ---- 3. PawnIO driver (CPU temperatures and power on Windows).
        Write-Step 'PawnIO driver (needed for CPU temperatures and power)'
        $pawnioDll = Join-Path $programFiles 'PawnIO\PawnIOLib.dll'
        if (Test-Path -LiteralPath $pawnioDll) {
            Write-Note 'Already installed.'
        } elseif (Test-Flag 'RUSTTEMP_NO_PAWNIO') {
            Write-Note 'Skipped (RUSTTEMP_NO_PAWNIO is set). Install it later with: winget install namazso.PawnIO'
        } elseif (-not $isAdmin) {
            Write-Warn 'Not installed: this needs Administrator rights. Re-run the installer from PowerShell opened with "Run as administrator", or run: winget install namazso.PawnIO'
        } else {
            $winget = Get-Command winget -ErrorAction SilentlyContinue
            if (-not $winget) {
                Write-Warn 'winget was not found. Install PawnIO from https://pawnio.eu (or install "App Installer" from the Microsoft Store and run the installer again).'
            } else {
                Write-Note 'Installing with winget (signed driver from https://pawnio.eu) ...'
                & winget install --id namazso.PawnIO --exact --source winget --silent --accept-package-agreements --accept-source-agreements
                if (Test-Path -LiteralPath $pawnioDll) {
                    Write-Note 'PawnIO installed.'
                } else {
                    Write-Warn "winget did not install PawnIO (exit code $LASTEXITCODE). Install it from https://pawnio.eu, then run cpu again."
                }
            }
        }

        Write-Host ''
        Write-Host 'Done! Try it now:' -ForegroundColor Green
        Write-Host ''
        Write-Host '    rusttemp          live temperature monitor (refreshes every second; Ctrl+C to stop)'
        Write-Host '    rusttemp cpu      CPU report: model, temperatures, clocks, power, integrated GPU'
        Write-Host '    rusttemp gpu      dedicated GPU report'
        Write-Host '    (shortcuts: cpu, gpu)   add --watch to keep refreshing, --help for options'
        Write-Host ''
        if (-not $isAdmin) {
            Write-Host '    CPU temperatures and power need PowerShell opened as Administrator.' -ForegroundColor Yellow
            Write-Host ''
        }
    } catch {
        Write-Host ''
        Write-Host "Installation failed: $($_.Exception.Message)" -ForegroundColor Red
        Write-Host "Help: https://github.com/10001111/rusttemp#troubleshooting"
        Write-Host ''
    }
}
