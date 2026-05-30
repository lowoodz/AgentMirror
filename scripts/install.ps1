# Install SecureModelRoute on Windows x86_64.
param(
    [switch]$Service,
    [switch]$Gui
)

$ErrorActionPreference = "Stop"

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

Write-Host "==> Installing to $Prefix"
New-Item -ItemType Directory -Force -Path $BinDir, $ConfDir | Out-Null
Copy-Item $SourceExe $SmrExe -Force

if (-not (Test-Path $Config)) {
    $Example = Join-Path $Root "smr.example.yaml"
    if (-not (Test-Path $Example)) {
        $Example = Join-Path $Root "config\smr.example.yaml"
    }
    Copy-Item $Example $Config -Force
    Write-Host "    Created $Config"
}

@(
    "@echo off",
    "start `"`" `"$SmrExe`" --config `"$Config`" --open %*"
) | Set-Content -Path $Launcher -Encoding ASCII

if ($Gui) {
    Write-Host "==> Building desktop app (Tauri, requires npm)"
    $RepoRoot = if (Test-Path (Join-Path $Root "Cargo.toml")) { $Root } else { Split-Path -Parent $Root }
    $GuiDir = Join-Path $RepoRoot "gui"
    if ((Get-Command npm -ErrorAction SilentlyContinue) -and (Test-Path $GuiDir)) {
        Push-Location $GuiDir
        $env:CARGO_TARGET_DIR = Join-Path $RepoRoot "target"
        npm ci --silent 2>$null; if ($LASTEXITCODE -ne 0) { npm install --silent }
        npm run build --silent
        Pop-Location
        $AppExe = Get-ChildItem (Join-Path $RepoRoot "target\release") -Filter "SecureModelRoute.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $AppExe) {
            $AppExe = Get-ChildItem (Join-Path $RepoRoot "target\release") -Filter "securemodelroute.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
        }
        if ($AppExe) {
            $DestDir = Join-Path $env:LOCALAPPDATA "Programs\SecureModelRoute"
            New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
            Copy-Item $AppExe.FullName (Join-Path $DestDir "SecureModelRoute.exe") -Force
            $StartMenu = [Environment]::GetFolderPath("Programs")
            $Shortcut = Join-Path $StartMenu "SecureModelRoute.lnk"
            $Wsh = New-Object -ComObject WScript.Shell
            $Link = $Wsh.CreateShortcut($Shortcut)
            $Link.TargetPath = Join-Path $DestDir "SecureModelRoute.exe"
            $Link.WorkingDirectory = $DestDir
            $Link.Description = "SecureModelRoute desktop"
            $Link.Save()
            Write-Host "    Desktop app: $DestDir\SecureModelRoute.exe"
            Write-Host "    Start menu:  $Shortcut"
        } else {
            Write-Warning "Tauri build finished but SecureModelRoute.exe not found under target\release"
        }
    } else {
        Write-Warning "npm or gui/ missing; skipped desktop app build"
    }
}

if ($Service) {
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
    Write-Host "    Scheduled task: $TaskName (logon, auto-restart)"
    Write-Host "    Logs: $LogOut"
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
    $env:Path = "$env:Path;$BinDir"
    Write-Host "    Added $BinDir to user PATH (restart terminal to apply everywhere)"
}

Write-Host ""
Write-Host "Installed:"
Write-Host "  binary:   $SmrExe"
Write-Host "  launcher: $Launcher"
Write-Host "  config:   $Config"
Write-Host "  GUI:      http://127.0.0.1:8080/ui"
Write-Host ""
Write-Host "Run:  securemodelroute"
Write-Host "Or:   smr.exe --config `"$Config`" --open"
Write-Host ""
Write-Host "Background service: .\install.ps1 -Service"
Write-Host "Desktop app:        .\install.ps1 -Gui"
