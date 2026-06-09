# NSIS install + smoke + uninstall test (Windows UTM guest).
# NSIS install/uninstall smoke — runs as windows-user (SSH interactive session).
param(
    [string]$StageDir = "",
    [string]$ConfigPath = "",
    [string]$LogPath = "",
    [string]$Base = "http://127.0.0.1:8080",
    [string]$InteractiveUser = $env:SMR_NSIS_TEST_USER
)

$GuestStaging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
if (-not $StageDir) { $StageDir = Join-Path $GuestStaging "smr-nsis-test-stage" }
if (-not $ConfigPath) { $ConfigPath = Join-Path $GuestStaging "smr-nsis-test\smr.yaml" }
if (-not $LogPath) { $LogPath = Join-Path $GuestStaging "smr-nsis-install-test.log" }

$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"
$WorkDir = Join-Path $GuestStaging "smr-nsis-test-work"

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

function Get-InteractiveUserInfo {
    param([string]$PreferredUser)
    if (-not [string]::IsNullOrWhiteSpace($PreferredUser)) {
        $home = Join-Path "C:\Users" $PreferredUser
        if (Test-Path $home) {
            return @{
                Name = $PreferredUser
                Home = $home
                LocalAppData = Join-Path $home "AppData\Local"
            }
        }
    }
    $UserDir = Get-ChildItem "C:\Users" -Directory |
        Where-Object { $_.Name -notin @("Public", "Default", "Default User", "All Users") } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $UserDir) {
        return $null
    }
    return @{
        Name = $UserDir.Name
        Home = $UserDir.FullName
        LocalAppData = Join-Path $UserDir.FullName "AppData\Local"
    }
}

function Invoke-InteractiveTask {
    param(
        [hashtable]$UserInfo,
        [string]$TaskName,
        [string]$ScriptPath,
        [string]$DoneMarker,
        [int]$TimeoutSec = 180
    )
    if (Test-Path $DoneMarker) { Remove-Item $DoneMarker -Force -ErrorAction SilentlyContinue }
    schtasks /Delete /TN $TaskName /F 2>$null | Out-Null
    $tr = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$ScriptPath`""
    $createOut = schtasks /Create /TN $TaskName /TR $tr /SC ONCE /ST 00:00 /RU $UserInfo.Name /IT /F 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "schtasks /Create failed for ${TaskName}: $createOut"
    }
    schtasks /Run /TN $TaskName | Out-Null
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $DoneMarker) {
            $codeText = (Get-Content $DoneMarker -Raw).Trim()
            schtasks /Delete /TN $TaskName /F 2>$null | Out-Null
            return [int]$codeText
        }
        Start-Sleep -Seconds 2
    }
    schtasks /Delete /TN $TaskName /F 2>$null | Out-Null
    throw "Interactive task timeout: $TaskName"
}

function Resolve-AppExe {
    param(
        [string]$LocalAppData,
        [string]$UserHome
    )
    @(
        (Join-Path $LocalAppData "Programs\com.securemodelroute.desktop\SafeRoute.exe"),
        (Join-Path $LocalAppData "Programs\com.securemodelroute.desktop\smr-gui.exe"),
        (Join-Path $LocalAppData "Programs\SafeRoute\SafeRoute.exe"),
        (Join-Path $LocalAppData "SafeRoute\SafeRoute.exe"),
        (Join-Path $LocalAppData "SafeRoute\smr-gui.exe")
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}

function Resolve-SmrBin {
    param(
        [string]$LocalAppData,
        [string]$UserHome
    )
    @(
        (Join-Path $UserHome ".local\bin\smr.exe"),
        (Join-Path $LocalAppData "SafeRoute\cli\smr.exe"),
        (Join-Path $LocalAppData "Programs\com.securemodelroute.desktop\resources\cli\smr.exe")
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}

function Test-UserUninstallEntry {
    param([string]$UserName)
    try {
        $sid = ([System.Security.Principal.NTAccount]::new($UserName)).Translate(
            [System.Security.Principal.SecurityIdentifier]
        ).Value
    } catch {
        try {
            $sid = ([System.Security.Principal.NTAccount]::new(".", $UserName)).Translate(
                [System.Security.Principal.SecurityIdentifier]
            ).Value
        } catch {
            return $false
        }
    }
    $root = "Registry::HKEY_USERS\$sid\Software\Microsoft\Windows\CurrentVersion\Uninstall\*"
    foreach ($item in Get-ItemProperty $root -ErrorAction SilentlyContinue) {
        if ([string]$item.DisplayName -match 'SafeRoute') {
            return $true
        }
    }
    return $false
}

Remove-Item $LogPath -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
Log "==> SafeRoute NSIS install test"
Log "Runner: $env:USERNAME"

$userInfo = Get-InteractiveUserInfo -PreferredUser $InteractiveUser
if (-not $userInfo) {
    Log "ERROR: no interactive user profile under C:\Users"
    exit 1
}
Log "Interactive user: $($userInfo.Name) ($($userInfo.Home))"

Get-Process smr, SafeRoute, smr-gui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

$Setup = Get-ChildItem $StageDir -Filter "*-setup.exe" -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending |
    Select-Object -First 1
if (-not $Setup) {
    Log "ERROR: no *-setup.exe in $StageDir"
    exit 1
}

$Example = Join-Path $StageDir "smr.example.yaml"
if (-not (Test-Path $Example)) {
    Log "ERROR: missing smr.example.yaml"
    exit 1
}

New-Item -ItemType Directory -Force -Path (Split-Path $ConfigPath) | Out-Null
Copy-Item $Example $ConfigPath -Force
Log "Config -> $ConfigPath"

$UninstallPs1 = Join-Path $StageDir "uninstall.ps1"
if (Test-Path $UninstallPs1) {
    $preUninstallScript = Join-Path $WorkDir "pre-uninstall.ps1"
    $preUninstallDone = Join-Path $WorkDir "pre-uninstall.done"
    @(
        "`$ErrorActionPreference = 'Continue'",
        "& powershell.exe -NoProfile -ExecutionPolicy Bypass -File '$UninstallPs1' -KeepConfig -Quiet",
        "Set-Content -Path '$preUninstallDone' -Value `$LASTEXITCODE -Encoding ascii"
    ) | Set-Content -Path $preUninstallScript -Encoding UTF8
    Log "Pre-clean via interactive uninstall.ps1"
    try {
        $preRc = Invoke-InteractiveTask -UserInfo $userInfo -TaskName "SmrNsisPreUninstall" `
            -ScriptPath $preUninstallScript -DoneMarker $preUninstallDone -TimeoutSec 240
        Log "Pre-uninstall exit: $preRc"
    } catch {
        Log "WARNING: pre-uninstall task failed: $($_.Exception.Message)"
    }
}

$setupScript = Join-Path $WorkDir "run-setup.ps1"
$setupDone = Join-Path $WorkDir "setup.done"
@(
    "`$ErrorActionPreference = 'Stop'",
    "`$p = Start-Process -FilePath '$($Setup.FullName)' -ArgumentList '/S' -Wait -PassThru",
    "Set-Content -Path '$setupDone' -Value `$p.ExitCode -Encoding ascii"
) | Set-Content -Path $setupScript -Encoding UTF8

Log "Running NSIS installer (interactive): $($Setup.Name)"
try {
    $setupRc = Invoke-InteractiveTask -UserInfo $userInfo -TaskName "SmrNsisSetup" `
        -ScriptPath $setupScript -DoneMarker $setupDone -TimeoutSec 300
} catch {
    Log "ERROR: NSIS setup task failed: $($_.Exception.Message)"
    exit 1
}
if ($setupRc -ne 0) {
    Log "ERROR: NSIS setup exit code $setupRc"
    exit 1
}
Log "NSIS setup exit: $setupRc"
Start-Sleep -Seconds 5

$AppExe = Resolve-AppExe -LocalAppData $userInfo.LocalAppData -UserHome $userInfo.Home
if (-not $AppExe) {
    Log "ERROR: installed app exe not found under $($userInfo.LocalAppData)\Programs"
    exit 1
}
Log "Installed app: $AppExe"

if (-not (Test-UserUninstallEntry -UserName $userInfo.Name)) {
    Log "ERROR: no Add/Remove Programs entry for $($userInfo.Name)"
    exit 1
}
Log "Add/Remove Programs entry OK"

$SmrBin = Resolve-SmrBin -LocalAppData $userInfo.LocalAppData -UserHome $userInfo.Home
if (-not $SmrBin) {
    Log "ERROR: NSIS did not install CLI companion under $($userInfo.Home)\.local\bin or $($userInfo.LocalAppData)\SafeRoute\cli"
    exit 1
}
Log "CLI companion: $SmrBin"

$launchScript = Join-Path $WorkDir "launch-gui.ps1"
$launchDone = Join-Path $WorkDir "launch.done"
@(
    "`$ErrorActionPreference = 'Stop'",
    "`$env:SMR_CONFIG = '$ConfigPath'",
    "Start-Process -FilePath '$AppExe' -ArgumentList @('--background') -WindowStyle Hidden | Out-Null",
    "Set-Content -Path '$launchDone' -Value 0 -Encoding ascii"
) | Set-Content -Path $launchScript -Encoding UTF8

Log "Launch tray GUI (interactive, SMR_CONFIG=$ConfigPath)"
try {
    Invoke-InteractiveTask -UserInfo $userInfo -TaskName "SmrNsisLaunch" `
        -ScriptPath $launchScript -DoneMarker $launchDone -TimeoutSec 60 | Out-Null
} catch {
    Log "ERROR: GUI launch task failed: $($_.Exception.Message)"
    exit 1
}
Start-Sleep -Seconds 12

$healthOk = $false
for ($i = 0; $i -lt 90; $i++) {
    try {
        $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 3
        if ("$h" -match "OK") {
            $healthOk = $true
            break
        }
    } catch {}
    Start-Sleep -Seconds 1
}
if (-not $healthOk) {
    Log "ERROR: health check failed on $Base"
    exit 1
}
Log "Health OK"

Get-Process smr, SafeRoute, smr-gui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

if (Test-Path $UninstallPs1) {
    $postUninstallScript = Join-Path $WorkDir "post-uninstall.ps1"
    $postUninstallDone = Join-Path $WorkDir "post-uninstall.done"
    @(
        "`$ErrorActionPreference = 'Continue'",
        "& powershell.exe -NoProfile -ExecutionPolicy Bypass -File '$UninstallPs1' -KeepConfig -Quiet",
        "Set-Content -Path '$postUninstallDone' -Value `$LASTEXITCODE -Encoding ascii"
    ) | Set-Content -Path $postUninstallScript -Encoding UTF8
    Log "Running uninstall.ps1 -KeepConfig -Quiet (interactive)"
    try {
        $uninstallRc = Invoke-InteractiveTask -UserInfo $userInfo -TaskName "SmrNsisPostUninstall" `
            -ScriptPath $postUninstallScript -DoneMarker $postUninstallDone -TimeoutSec 240
        if ($uninstallRc -ne 0) {
            Log "ERROR: uninstall.ps1 exit $uninstallRc"
            exit 1
        }
    } catch {
        Log "ERROR: uninstall task failed: $($_.Exception.Message)"
        exit 1
    }
} else {
    Log "WARNING: uninstall.ps1 missing; skipping uninstall step"
}

Log "NSIS INSTALL TEST PASSED"
exit 0
