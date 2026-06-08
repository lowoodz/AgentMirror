# Uninstall SafeRoute on Windows (NSIS GUI + optional CLI copy install).
param(
    [switch]$KeepConfig,
    [switch]$Quiet
)

$ErrorActionPreference = "Continue"

function Write-UninstallLog($msg) {
    if (-not $Quiet) { Write-Host $msg }
}

$Prefix = if ($env:SMR_INSTALL_PREFIX) { $env:SMR_INSTALL_PREFIX } else { Join-Path $env:USERPROFILE ".local" }
$BinDir = Join-Path $Prefix "bin"
$ConfDir = Join-Path $Prefix "etc\securemodelroute"

function Invoke-NsisUninstall {
    $patterns = @("SafeRoute", "SecureModelRoute")
    $roots = @(
        "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
        "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*"
    )
    foreach ($root in $roots) {
        foreach ($item in Get-ItemProperty $root -ErrorAction SilentlyContinue) {
            $name = [string]$item.DisplayName
            if (-not $patterns.Any({ $name -match $_ })) { continue }
            $cmd = [string]$item.UninstallString
            if ([string]::IsNullOrWhiteSpace($cmd)) { continue }
            Write-UninstallLog "==> Running NSIS uninstaller: $name"
            if ($cmd -match '^"(.+?)"(.*)$') {
                $exe = $matches[1]
                $args = $matches[2].Trim()
            } else {
                $parts = $cmd -split '\s+', 2
                $exe = $parts[0]
                $args = if ($parts.Count -gt 1) { $parts[1] } else { "" }
            }
            if ($args -notmatch '/S') { $args = "$args /S".Trim() }
            $argList = @()
            if ($args) { $argList = $args -split '\s+' | Where-Object { $_ } }
            if ($argList -notcontains '/S') { $argList += '/S' }
            try {
                $proc = Start-Process -FilePath $exe -ArgumentList $argList -Wait -PassThru
                if ($proc.ExitCode -ne 0) {
                    Write-UninstallLog "    NSIS uninstaller exit code: $($proc.ExitCode)"
                }
            } catch {
                Write-UninstallLog "    NSIS uninstaller failed: $($_.Exception.Message)"
                return $false
            }
            return $true
        }
    }

    foreach ($candidate in @(
        (Join-Path $env:LOCALAPPDATA "Programs\com.securemodelroute.desktop\uninstall.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\SafeRoute\uninstall.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\SecureModelRoute\uninstall.exe"),
        (Join-Path $env:LOCALAPPDATA "SafeRoute\uninstall.exe")
    )) {
        if (Test-Path $candidate) {
            Write-UninstallLog "==> Running uninstaller: $candidate"
            try {
                $proc = Start-Process -FilePath $candidate -ArgumentList @('/S') -Wait -PassThru
                if ($proc.ExitCode -ne 0) {
                    Write-UninstallLog "    uninstaller exit code: $($proc.ExitCode)"
                }
            } catch {
                Write-UninstallLog "    uninstaller failed: $($_.Exception.Message)"
                return $false
            }
            return $true
        }
    }
    return $false
}

function Remove-CopyInstall {
    Write-UninstallLog "==> Removing CLI / portable install artifacts"
    foreach ($file in @(
        (Join-Path $BinDir "smr.exe"),
        (Join-Path $BinDir "securemodelroute.cmd"),
        (Join-Path $BinDir "SecureModelRoute.cmd"),
        (Join-Path $BinDir "smr-service.cmd")
    )) {
        if (Test-Path $file) {
            Remove-Item $file -Force
            Write-UninstallLog "    removed $file"
        }
    }

    foreach ($dir in @(
        (Join-Path $env:LOCALAPPDATA "Programs\SecureModelRoute"),
        (Join-Path $env:LOCALAPPDATA "Programs\SafeRoute")
    )) {
        if (Test-Path $dir) {
            Remove-Item $dir -Recurse -Force
            Write-UninstallLog "    removed $dir"
        }
    }

    foreach ($link in @(
        (Join-Path ([Environment]::GetFolderPath("Programs")) "SecureModelRoute.lnk"),
        (Join-Path ([Environment]::GetFolderPath("Programs")) "SafeRoute.lnk"),
        (Join-Path ([Environment]::GetFolderPath("Desktop")) "SecureModelRoute.lnk"),
        (Join-Path ([Environment]::GetFolderPath("Desktop")) "SafeRoute.lnk"),
        (Join-Path ([Environment]::GetFolderPath("Startup")) "SecureModelRoute.lnk"),
        (Join-Path ([Environment]::GetFolderPath("Startup")) "SafeRoute.lnk")
    )) {
        if (Test-Path $link) {
            Remove-Item $link -Force
            Write-UninstallLog "    removed $link"
        }
    }

    $taskName = "SecureModelRoute"
    if (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
        Write-UninstallLog "    removed scheduled task $taskName"
    }

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -and $userPath -like "*$BinDir*") {
        $parts = $userPath -split ';' | Where-Object { $_ -and ($_ -ne $BinDir) }
        [Environment]::SetEnvironmentVariable("Path", ($parts -join ';'), "User")
        Write-UninstallLog "    removed $BinDir from user PATH"
    }

    if (-not $KeepConfig) {
        if (Test-Path $ConfDir) {
            Remove-Item $ConfDir -Recurse -Force
            Write-UninstallLog "    removed config $ConfDir"
        }
        $appDataCfg = Join-Path $env:APPDATA "securemodelroute"
        if (Test-Path $appDataCfg) {
            Remove-Item $appDataCfg -Recurse -Force
            Write-UninstallLog "    removed config $appDataCfg"
        }
    } else {
        Write-UninstallLog "    kept config ( -KeepConfig )"
    }
}

$ranNsis = Invoke-NsisUninstall
Remove-CopyInstall

if ($ranNsis) {
    Write-UninstallLog ""
    Write-UninstallLog "SafeRoute NSIS uninstall completed."
} else {
    Write-UninstallLog ""
    Write-UninstallLog "No NSIS entry found; removed portable/CLI install only."
}
Write-UninstallLog "Done."
