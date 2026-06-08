# Install staged dist payload for the interactive Windows user (UTM guest).
# Stage dir must contain: smr.exe, SafeRoute.exe, smr.example.yaml
param(
    [string]$StageDir = "",
    [string]$LogPath = ""
)

$GuestStaging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
if (-not $StageDir) { $StageDir = Join-Path $GuestStaging "smr-install-stage" }
if (-not $LogPath) { $LogPath = Join-Path $GuestStaging "smr-install-dist.log" }

$ErrorActionPreference = "Stop"
Remove-Item $LogPath -Force -ErrorAction SilentlyContinue

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
}

try {
    Log "==> SafeRoute dist install"
    Log "Stage: $StageDir"

    if (-not (Test-Path (Join-Path $StageDir "smr.exe"))) {
        throw "Missing staged file: smr.exe"
    }
    if (-not (Test-Path (Join-Path $StageDir "smr.example.yaml"))) {
        throw "Missing staged file: smr.example.yaml"
    }
    $StagedGui = Join-Path $StageDir "SafeRoute.exe"
    if (-not (Test-Path $StagedGui)) {
        throw "Missing staged GUI: SafeRoute.exe"
    }

    $UserDir = Get-ChildItem "C:\Users" -Directory |
        Where-Object { $_.Name -notin @("Public", "Default", "Default User", "All Users") } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $UserDir) { throw "No user profile under C:\Users" }
    $User = $UserDir.Name
    $UserHome = $UserDir.FullName
    if ([string]::IsNullOrWhiteSpace($UserHome)) {
        $UserHome = "C:\Users\$User"
    }
    Log "Target user: $User ($UserHome)"

    $BinDir = Join-Path $UserHome ".local\bin"
    $ConfDir = Join-Path $UserHome ".local\etc\securemodelroute"
    $GuiDir = Join-Path $UserHome "AppData\Local\Programs\SafeRoute"
    $Config = Join-Path $ConfDir "smr.yaml"
    $GuiExe = Join-Path $GuiDir "SafeRoute.exe"

    New-Item -ItemType Directory -Force -Path $BinDir, $ConfDir, $GuiDir | Out-Null
    Copy-Item (Join-Path $StageDir "smr.exe") (Join-Path $BinDir "smr.exe") -Force
    Copy-Item $StagedGui $GuiExe -Force
    if (-not (Test-Path $Config)) {
        Copy-Item (Join-Path $StageDir "smr.example.yaml") $Config -Force
        Log "Created config $Config"
    }

    $Launcher = Join-Path $BinDir "securemodelroute.cmd"
    @(
        "@echo off",
        "start `"`" `"$(Join-Path $BinDir 'smr.exe')`" --config `"$Config`" --open %*"
    ) | Set-Content -Path $Launcher -Encoding ASCII

    $Wsh = New-Object -ComObject WScript.Shell
    $Programs = Join-Path $UserHome "AppData\Roaming\Microsoft\Windows\Start Menu\Programs"
    $Desktop = Join-Path $UserHome "Desktop"
    foreach ($pair in @(
        @{ Dir = $Programs; Label = "Start menu" },
        @{ Dir = $Desktop; Label = "Desktop" }
    )) {
        if ([string]::IsNullOrWhiteSpace($pair.Dir) -or -not (Test-Path $pair.Dir)) { continue }
        $LinkPath = Join-Path $pair.Dir "SafeRoute.lnk"
        $Link = $Wsh.CreateShortcut($LinkPath)
        $Link.TargetPath = $GuiExe
        $Link.Arguments = "--background"
        $Link.WorkingDirectory = $GuiDir
        $Link.Description = "SafeRoute"
        $Link.Save()
        Log "$($pair.Label): $LinkPath"
    }

    $Startup = Join-Path $UserHome "AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"
    if (Test-Path $Startup) {
        $StartupLink = Join-Path $Startup "SafeRoute.lnk"
        $Link = $Wsh.CreateShortcut($StartupLink)
        $Link.TargetPath = $GuiExe
        $Link.Arguments = "--background"
        $Link.WorkingDirectory = $GuiDir
        $Link.Save()
        Log "Startup: $StartupLink"
    }

    Get-Process smr, smr-gui, SafeRoute -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2

    $TaskName = "SafeRouteLaunch"
    schtasks /Delete /TN $TaskName /F 2>$null | Out-Null
    schtasks /Delete /TN "SecureModelRouteLaunch" /F 2>$null | Out-Null
    $tr = "powershell.exe -NoProfile -WindowStyle Hidden -Command `"Start-Process -FilePath '$GuiExe' -ArgumentList '--background' -WindowStyle Hidden`""
    schtasks /Create /TN $TaskName /TR $tr /SC ONCE /ST 00:00 /RU $User /IT /F | Out-Null
    schtasks /Run /TN $TaskName | Out-Null
    Log "Launched GUI via scheduled task ($TaskName)"

    Log "CLI: $(Join-Path $BinDir 'smr.exe')"
    Log "GUI: $GuiExe"
    Log "INSTALL_OK"
}
catch {
    Log "ERROR: $($_.Exception.Message)"
    exit 1
}
