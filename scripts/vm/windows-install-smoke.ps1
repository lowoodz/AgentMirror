# Windows dist install smoke (CLI zip + portable GUI, no live API tests).
param(
    [string]$ZipPath = "",
    [string]$GuiExe = "",
    [switch]$CliOnly,
    [string]$LogPath = "",
    [string]$Prefix = "",
    [string]$Base = "http://127.0.0.1:8080"
)

$GuestStaging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
if (-not $ZipPath) { $ZipPath = Join-Path $GuestStaging "smr.zip" }
if (-not $LogPath) { $LogPath = Join-Path $GuestStaging "smr-install-smoke.log" }
if (-not $Prefix) { $Prefix = Join-Path $GuestStaging "smr-install-smoke-home" }

$ErrorActionPreference = "Continue"
function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

Remove-Item $LogPath -Force -ErrorAction SilentlyContinue
Log "==> Windows install smoke test"

Get-Process smr, SafeRoute -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

if (-not (Test-Path $ZipPath)) {
    Log "ERROR: zip not found: $ZipPath"
    exit 1
}

$Work = Join-Path $GuestStaging "smr-install-smoke-work"
Remove-Item $Work -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive -Path $ZipPath -DestinationPath $Work -Force

$InstallPs1 = Join-Path $Work "install.ps1"
$SmrSrc = Join-Path $Work "smr.exe"
if (-not (Test-Path $InstallPs1) -or -not (Test-Path $SmrSrc)) {
    Log "ERROR: zip missing install.ps1 or smr.exe"
    exit 1
}

$env:SMR_INSTALL_PREFIX = $Prefix
Remove-Item $Prefix -Recurse -Force -ErrorAction SilentlyContinue
Push-Location $Work

if ($CliOnly) {
    Log "CLI-only install smoke"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $InstallPs1 -Quiet
} else {
    if (-not $GuiExe) { $GuiExe = Join-Path $GuestStaging "smr-app-test-stage\SafeRoute.exe" }
    if (-not (Test-Path $GuiExe)) {
        $alt = Join-Path $Work "SafeRoute.exe"
        if (Test-Path $alt) { $GuiExe = $alt }
    }
    if (-not (Test-Path $GuiExe)) {
        Log "ERROR: GUI exe not found (pass -CliOnly for CLI-only smoke)"
        exit 1
    }
    Copy-Item $GuiExe (Join-Path $Work "SafeRoute.exe") -Force
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $InstallPs1 -Gui -Quiet
}

$installRc = $LASTEXITCODE
Pop-Location
if ($installRc -ne 0) {
    Log "ERROR: install.ps1 exit $installRc"
    exit 1
}

$BinDir = Join-Path $Prefix "bin"
$SmrExe = Join-Path $BinDir "smr.exe"
$Config = Join-Path $Prefix "etc\securemodelroute\smr.yaml"
if (-not (Test-Path $SmrExe)) {
    Log "ERROR: smr.exe not installed to $SmrExe"
    exit 1
}
Log "Installed CLI: $SmrExe"

$proc = Start-Process -FilePath $SmrExe -ArgumentList @("--config", $Config) -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 6
try {
    $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 5
    if ("$h" -notmatch "OK") { throw "unexpected health: $h" }
    Log "Health OK"
} catch {
    Log "ERROR: health failed: $($_.Exception.Message)"
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    exit 1
}
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

$UninstallPs1 = Join-Path $Work "uninstall.ps1"
if (Test-Path $UninstallPs1) {
    Log "Running uninstall.ps1 -KeepConfig -Quiet"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $UninstallPs1 -KeepConfig -Quiet
    if ($LASTEXITCODE -ne 0) {
        Log "ERROR: uninstall.ps1 exit $LASTEXITCODE"
        exit 1
    }
}

Log "INSTALL SMOKE TEST PASSED"
exit 0
