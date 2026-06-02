# Install SecureModelRoute on Windows x86_64.
param(
    [switch]$Service,
    [switch]$Gui,
    [switch]$All,
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"

function Write-InstallLog($msg) {
    if (-not $Quiet) { Write-Host $msg }
}

if ($All) {
    $Service = $true
    $Gui = $true
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = if (Test-Path (Join-Path $ScriptDir "smr.exe")) { $ScriptDir } else { Split-Path -Parent $ScriptDir }

$Prefix = if ($env:SMR_INSTALL_PREFIX) { $env:SMR_INSTALL_PREFIX } else { Join-Path $env:USERPROFILE ".local" }
$BinDir = Join-Path $Prefix "bin"
$ConfDir = Join-Path $Prefix "etc\securemodelroute"
$SmrExe = Join-Path $BinDir "smr.exe"
$Config = Join-Path $ConfDir "smr.yaml"
$Launcher = Join-Path $BinDir "securemodelroute.cmd"
$LogOut = Join-Path $ConfDir "smr.log"
$LogErr = Join-Path $ConfDir "smr.err.log"

$SourceExe = Join-Path $Root "smr.exe"
if (-not (Test-Path $SourceExe)) {
    $SourceExe = Join-Path $Root "target\release\smr.exe"
}
if (-not (Test-Path $SourceExe)) {
    Write-Error "smr.exe not found. Run package.ps1 first or extract the release zip."
}

Write-InstallLog "==> Installing to $Prefix"
New-Item -ItemType Directory -Force -Path $BinDir, $ConfDir | Out-Null
Copy-Item $SourceExe $SmrExe -Force

if (-not (Test-Path $Config)) {
    $Example = Join-Path $Root "smr.example.yaml"
    if (-not (Test-Path $Example)) {
        $Example = Join-Path $Root "config\smr.example.yaml"
    }
    Copy-Item $Example $Config -Force
    Write-InstallLog "    Created $Config"
}

@(
    "@echo off",
    "start `"`" `"$SmrExe`" --config `"$Config`" --open %*"
) | Set-Content -Path $Launcher -Encoding ASCII

function Install-SmrDesktop {
    param([string]$SearchRoot)

    $DestDir = Join-Path $env:LOCALAPPDATA "Programs\SecureModelRoute"
    $AppPath = Join-Path $DestDir "SecureModelRoute.exe"
    $Installed = $false

    # Prefer the portable GUI exe shipped beside install.ps1 (IExpress / zip layout).
    $Bundled = Get-ChildItem $SearchRoot -Filter "SecureModelRoute.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($Bundled) {
        New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
        Copy-Item $Bundled.FullName $AppPath -Force
        $Installed = $true
        Write-InstallLog "    Installed portable app: $AppPath"
    }

    # Tauri NSIS bundle only (not our IExpress SecureModelRoute-*-x64-Setup.exe).
    if (-not $Installed) {
        $Setup = Get-ChildItem $SearchRoot -Filter "*-setup.exe" -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -notmatch 'x64-Setup\.exe$' } |
            Select-Object -First 1
        if ($Setup) {
            Write-InstallLog "    Running NSIS installer: $($Setup.Name)"
            Start-Process -FilePath $Setup.FullName -ArgumentList "/S" -Wait
            foreach ($candidate in @(
                $AppPath,
                (Join-Path $env:LOCALAPPDATA "Programs\com.securemodelroute.desktop\SecureModelRoute.exe")
            )) {
                if (Test-Path $candidate) {
                    $AppPath = $candidate
                    $Installed = $true
                    break
                }
            }
        }
    }

    if (-not $Installed) {
        Write-InstallLog "    No bundled desktop app; building from source (requires npm + gui/)"
        $RepoRoot = if (Test-Path (Join-Path $SearchRoot "Cargo.toml")) { $SearchRoot } else { Split-Path -Parent $SearchRoot }
        $GuiDir = Join-Path $RepoRoot "gui"
        if ((Get-Command npm -ErrorAction SilentlyContinue) -and (Test-Path $GuiDir)) {
            Push-Location $GuiDir
            $env:CARGO_TARGET_DIR = Join-Path $RepoRoot "target"
            npm ci --silent 2>$null; if ($LASTEXITCODE -ne 0) { npm install --silent }
            npm run build --silent
            Pop-Location
            $Built = Get-ChildItem (Join-Path $RepoRoot "target\release") -Filter "SecureModelRoute.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($Built) {
                New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
                Copy-Item $Built.FullName $AppPath -Force
                $Installed = $true
            } else {
                Write-Warning "Tauri build finished but SecureModelRoute.exe not found under target\release"
            }
        } else {
            Write-Warning "npm or gui/ missing; extract smr-*-windows-x86_64-app.zip or run package.ps1 first"
        }
    }

    if ($Installed -and (Test-Path $AppPath)) {
        $GuiLauncher = Join-Path $BinDir "SecureModelRoute.cmd"
        @(
            "@echo off",
            "set SMR_CONFIG=$Config",
            "start `"`" `"$AppPath`" --background %*"
        ) | Set-Content -Path $GuiLauncher -Encoding ASCII

        $Wsh = New-Object -ComObject WScript.Shell
        foreach ($shortcut in @(
            @{ Dir = [Environment]::GetFolderPath("Programs"); Label = "Start menu" },
            @{ Dir = [Environment]::GetFolderPath("Desktop"); Label = "Desktop" }
        )) {
            $LinkPath = Join-Path $shortcut.Dir "SecureModelRoute.lnk"
            $Link = $Wsh.CreateShortcut($LinkPath)
            $Link.TargetPath = $GuiLauncher
            $Link.WorkingDirectory = $BinDir
            $Link.Description = "SecureModelRoute desktop"
            $Link.Save()
            Write-InstallLog "    $($shortcut.Label): $LinkPath"
        }
        Write-InstallLog "    App: $AppPath"
        Write-InstallLog "    Launcher: $GuiLauncher"
        return $AppPath
    }

    return $null
}

$DesktopAppPath = $null
if ($Gui -or $All) {
    Write-InstallLog "==> Installing desktop app (tray GUI, embeds server)"
    $DesktopAppPath = Install-SmrDesktop -SearchRoot $Root
    if ($DesktopAppPath) {
        [Environment]::SetEnvironmentVariable("SMR_CONFIG", $Config, "User")
        $env:SMR_CONFIG = $Config
    }
}

# Headless service only when GUI is not installed (GUI keeps running in the system tray).
if ($Service -and -not $Gui) {
    $TaskName = "SecureModelRoute"
    $ServiceCmd = Join-Path $BinDir "smr-service.cmd"
    @(
        "@echo off",
        "`"$SmrExe`" --config `"$Config`" 1>> `"$LogOut`" 2>> `"$LogErr`""
    ) | Set-Content -Path $ServiceCmd -Encoding ASCII
    $Action = New-ScheduledTaskAction -Execute $ServiceCmd -WorkingDirectory $ConfDir
    $Trigger = New-ScheduledTaskTrigger -AtLogOn
    $Settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -RestartCount 999 `
        -RestartInterval (New-TimeSpan -Minutes 1) `
        -ExecutionTimeLimit ([TimeSpan]::Zero)
    Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Force | Out-Null
    Write-InstallLog "    Scheduled task: $TaskName (logon, auto-restart)"
    Write-InstallLog "    Logs: $LogOut"
    try {
        Start-ScheduledTask -TaskName $TaskName -ErrorAction Stop
        Write-InstallLog "    Service started"
    } catch {
        Start-Process -FilePath $SmrExe -ArgumentList "--config", $Config -WindowStyle Hidden
        Write-InstallLog "    Started smr process (task start pending next logon)"
    }
}

if ($DesktopAppPath -and $All) {
    $StartupFolder = [Environment]::GetFolderPath("Startup")
    $StartupLink = Join-Path $StartupFolder "SecureModelRoute.lnk"
    $GuiLauncher = Join-Path $BinDir "SecureModelRoute.cmd"
    $Wsh = New-Object -ComObject WScript.Shell
    $Link = $Wsh.CreateShortcut($StartupLink)
    $Link.TargetPath = $GuiLauncher
    $Link.WorkingDirectory = $BinDir
    $Link.Description = "SecureModelRoute (background tray)"
    $Link.Save()
    Write-InstallLog "    Logon startup: $StartupLink (--background, tray only)"
    if (-not $Quiet) {
        Start-Process -FilePath $GuiLauncher
        Write-InstallLog "    Tray app started"
    }
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
    $env:Path = "$env:Path;$BinDir"
    Write-InstallLog "    Added $BinDir to user PATH (restart terminal to apply everywhere)"
}

Write-InstallLog ""
Write-InstallLog "Installed:"
Write-InstallLog "  binary:   $SmrExe"
Write-InstallLog "  launcher: $Launcher"
Write-InstallLog "  config:   $Config"
Write-InstallLog "  web UI:   http://127.0.0.1:8080/ui"
if ($All) {
    Write-InstallLog "  mode:     full (CLI + tray GUI; close window to hide in tray)"
} elseif ($Gui) {
    Write-InstallLog "  mode:     tray GUI (close window to hide in tray)"
} else {
    Write-InstallLog ""
    Write-InstallLog "Optional: .\install.ps1 -All   # CLI + tray GUI"
    Write-InstallLog "          .\install.ps1 -Service  # headless background only"
    Write-InstallLog "          .\install.ps1 -Gui"
}
Write-InstallLog ""
Write-InstallLog "Run:  securemodelroute"
