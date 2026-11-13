# RustTemp uninstaller for Windows.
#
#     irm https://raw.githubusercontent.com/10001111/rusttemp/main/uninstall.ps1 | iex
#
# Removes cpu.exe, gpu.exe and rusttemp.exe installed by install.ps1, and
# their PATH entries. Run it as Administrator if RustTemp was installed as
# Administrator (in C:\Program Files\RustTemp).
#
# The PawnIO driver is NOT removed, because other programs (for example fan
# control or hardware monitoring tools) may use it. To remove it as well:
#     winget uninstall namazso.PawnIO
#
# Set $env:RUSTTEMP_INSTALL_DIR first if you installed to a custom folder.

& {
    Set-StrictMode -Version 2
    $ErrorActionPreference = 'Stop'

    function Get-EnvKey([string]$Scope) {
        if ($Scope -eq 'Machine') {
            return [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\Session Manager\Environment', $true)
        }
        return [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    }

    function Remove-FromPath([string]$Dir, [string]$Scope) {
        $key = Get-EnvKey $Scope
        try {
            $raw = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $entries = @($raw -split ';' | Where-Object { $_ -ne '' })
            $kept = @($entries | Where-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ine $Dir.TrimEnd('\') })
            if ($kept.Count -eq $entries.Count) { return $false }
            $key.SetValue('Path', ($kept -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
        } finally {
            $key.Close()
        }
        [Environment]::SetEnvironmentVariable('RUSTTEMP_PATH_REFRESH', '1', 'User')
        [Environment]::SetEnvironmentVariable('RUSTTEMP_PATH_REFRESH', $null, 'User')
        return $true
    }

    try {
        $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
        $isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        $programFiles = $env:ProgramW6432
        if (-not $programFiles) { $programFiles = $env:ProgramFiles }

        $candidates = @()
        if ($env:RUSTTEMP_INSTALL_DIR) { $candidates += [IO.Path]::GetFullPath($env:RUSTTEMP_INSTALL_DIR) }
        $candidates += (Join-Path $programFiles 'RustTemp')
        $candidates += (Join-Path $env:LOCALAPPDATA 'Programs\RustTemp')

        # Leave the folders before deleting them.
        Set-Location ([IO.Path]::GetTempPath())

        $removed = 0
        foreach ($dir in ($candidates | Select-Object -Unique)) {
            # Only delete folders that really are RustTemp installations.
            if (-not (Test-Path -LiteralPath (Join-Path $dir 'rusttemp.exe'))) { continue }
            $machine = $dir.StartsWith($programFiles, [StringComparison]::OrdinalIgnoreCase)
            if ($machine -and -not $isAdmin) {
                Write-Host "RustTemp is installed in $dir, which needs Administrator rights to remove." -ForegroundColor Yellow
                Write-Host 'Open PowerShell with "Run as administrator" and run the uninstall command again.' -ForegroundColor Yellow
                continue
            }
            try {
                Remove-Item -LiteralPath $dir -Recurse -Force
            } catch [System.IO.IOException] {
                throw "Could not delete $dir. Close any window that is running cpu, gpu or rusttemp, then try again."
            }
            Write-Host "Removed $dir"
            foreach ($scope in @('User', 'Machine')) {
                if ($scope -eq 'Machine' -and -not $isAdmin) { continue }
                if (Remove-FromPath $dir $scope) { Write-Host "Removed $dir from the $($scope.ToLower()) PATH" }
            }
            $env:Path = (@($env:Path -split ';' | Where-Object { $_ -ne '' -and $_.TrimEnd('\') -ine $dir.TrimEnd('\') })) -join ';'
            $removed++
        }

        if ($removed -eq 0) {
            Write-Host 'No RustTemp installation was removed.'
        } else {
            Write-Host ''
            Write-Host 'RustTemp was uninstalled.' -ForegroundColor Green
        }
        Write-Host 'The PawnIO driver was left installed. To remove it too: winget uninstall namazso.PawnIO'
        Write-Host 'If you also installed RustTemp with Cargo: cargo uninstall rusttemp'
    } catch {
        Write-Host "Uninstall failed: $($_.Exception.Message)" -ForegroundColor Red
    }
}
