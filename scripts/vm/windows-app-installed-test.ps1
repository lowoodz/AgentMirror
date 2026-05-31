# Install from Setup payload, launch tray GUI, run attach-mode blackbox on Windows UTM guest.
param(
    [string]$StageDir = "C:\Users\Public\smr-app-test-stage",
    [string]$Prefix = "C:\Users\Public\smr-app-test-home",
    [string]$SecretsDir = "C:\Users\Public\smr-app-test-secrets",
    [string]$ConfigPath = "C:\Users\Public\smr-app-test-home\smr.yaml",
    [string]$LogPath = "C:\Users\Public\smr-app-installed-test.log",
    [string]$TestRoot = "C:\Users\Public\smr-test-suite",
    [string]$Base = "http://127.0.0.1:8080"
)

$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

Remove-Item $LogPath -Force -ErrorAction SilentlyContinue
Log "==> Windows installed-app black-box test"

Get-Process smr, smr-gui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Get-Process SecureModelRoute -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

New-Item -ItemType Directory -Force -Path $StageDir, $Prefix, $SecretsDir, (Split-Path $ConfigPath) | Out-Null
Set-Content -Path (Join-Path $SecretsDir "project.txt") -Value "probe-secret-data" -Encoding UTF8

foreach ($f in @("smr.exe", "SecureModelRoute.exe", "install.ps1", "smr.example.yaml")) {
    if (-not (Test-Path (Join-Path $StageDir $f))) {
        Log "ERROR: missing staged file $f under $StageDir"
        exit 1
    }
}

Log "==> install.ps1 -All -Quiet (tray GUI + CLI)"
$env:SMR_INSTALL_PREFIX = $Prefix
Push-Location $StageDir
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $StageDir "install.ps1") -All -Quiet
Pop-Location
if ($LASTEXITCODE -ne 0) {
    Log "ERROR: install.ps1 failed exit=$LASTEXITCODE"
    exit 1
}

$AppExe = Join-Path $env:LOCALAPPDATA "Programs\SecureModelRoute\SecureModelRoute.exe"
if (-not (Test-Path $AppExe)) {
    Log "ERROR: desktop app not installed at $AppExe"
    exit 1
}
Log "Desktop app: $AppExe"

$python = Get-Command python -ErrorAction SilentlyContinue
if (-not $python) { $python = Get-Command python3 -ErrorAction SilentlyContinue }
if (-not $python) {
    Log "ERROR: python not found on guest"
    exit 1
}

Log "==> Write test config -> $ConfigPath"
& $python.Source (Join-Path $TestRoot "scripts\generate_test_config.py") $ConfigPath $SecretsDir 2>&1 | ForEach-Object { Log $_ }
if (-not (Test-Path $ConfigPath)) {
    Log "ERROR: config not created"
    exit 1
}

Log "==> Launch tray GUI with SMR_CONFIG"
$env:SMR_CONFIG = $ConfigPath
$gui = Start-Process -FilePath $AppExe -ArgumentList @("--background") -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 8

function Wait-Ready {
    for ($i = 0; $i -lt 90; $i++) {
        try {
            $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 3
            if ("$h" -match "OK") {
                $st = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 3
                if ($st.file_index_ready) { return $true }
            }
        } catch {}
        Start-Sleep -Seconds 1
    }
    return $false
}

if (-not (Wait-Ready)) {
    Log "ERROR: installed GUI server not ready"
    if (-not $gui.HasExited) { Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue }
    exit 1
}
Log "Server ready pid=$($gui.Id)"

try {
    $ui = Invoke-WebRequest -Uri "$Base/ui" -TimeoutSec 15 -UseBasicParsing
    if ($ui.Content -notmatch "SecureModelRoute") {
        Log "ERROR: admin UI missing marker"
        exit 1
    }
    Log "Admin UI OK bytes=$($ui.Content.Length)"
} catch {
    Log "ERROR: admin UI: $($_.Exception.Message)"
    exit 1
}

Log "==> Tray smoke: GUI process alive while hitting API"
if ($gui.HasExited) {
    Log "ERROR: GUI exited early"
    exit 1
}
$health2 = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 5
Log "Health after background launch: $health2"

Log "==> blackbox_test.py (attach mode)"
$env:SMR_ATTACH = "1"
$env:SMR_BASE = $Base
& $python.Source (Join-Path $TestRoot "scripts\blackbox_test.py") 2>&1 | ForEach-Object { Log $_ }
$bb = $LASTEXITCODE

if (-not $gui.HasExited) {
    Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
}
Get-Process smr -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

if ($bb -ne 0) {
    Log "INSTALLED-APP TEST FAILED (blackbox exit=$bb)"
    exit 1
}

Log "INSTALLED-APP TEST PASSED"
exit 0
