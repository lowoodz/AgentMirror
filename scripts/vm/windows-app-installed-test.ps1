# Install tray GUI from staged payload (UTM guest runs as SYSTEM — avoid install.ps1 -All shortcuts).
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

function Find-Python {
    foreach ($cmd in @("python", "py", "python3")) {
        $c = Get-Command $cmd -ErrorAction SilentlyContinue
        if ($c) { return $c.Source }
    }
    $roots = @(
        "C:\Users\Public\python312",
        "$env:LOCALAPPDATA\Programs\Python",
        "C:\Program Files\Python312",
        "C:\Program Files\Python311"
    )
    foreach ($root in $roots) {
        $exe = Join-Path $root "python.exe"
        if (Test-Path $exe) { return $exe }
        if (-not (Test-Path $root)) { continue }
        $found = Get-ChildItem -Path $root -Recurse -Filter python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) { return $found.FullName }
    }
    return $null
}

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

foreach ($f in @("smr.exe", "SecureModelRoute.exe")) {
    if (-not (Test-Path (Join-Path $StageDir $f))) {
        Log "ERROR: missing staged file $f under $StageDir"
        exit 1
    }
}

$BinDir = Join-Path $Prefix "bin"
$GuiDir = Join-Path $Prefix "Programs\SecureModelRoute"
New-Item -ItemType Directory -Force -Path $BinDir, $GuiDir | Out-Null
Copy-Item (Join-Path $StageDir "smr.exe") (Join-Path $BinDir "smr.exe") -Force
Copy-Item (Join-Path $StageDir "SecureModelRoute.exe") (Join-Path $GuiDir "SecureModelRoute.exe") -Force
Log "Installed CLI -> $BinDir"
Log "Installed GUI  -> $GuiDir"

$AppExe = Join-Path $GuiDir "SecureModelRoute.exe"
if (-not (Test-Path $AppExe)) {
    Log "ERROR: desktop app missing at $AppExe"
    exit 1
}

$python = Find-Python
if (-not $python) {
    Log "ERROR: python not found on guest (run utm-run-python-tests once to bootstrap)"
    exit 1
}
Log "Using Python: $python"

Log "==> Write test config -> $ConfigPath"
& $python (Join-Path $TestRoot "scripts\generate_test_config.py") $ConfigPath $SecretsDir 2>&1 | ForEach-Object { Log $_ }
if (-not (Test-Path $ConfigPath)) {
    Log "ERROR: config not created"
    exit 1
}

Log "==> Launch tray GUI (SMR_CONFIG, --background)"
$env:SMR_CONFIG = $ConfigPath
$gui = Start-Process -FilePath $AppExe -ArgumentList @("--background") -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 10

function Wait-Ready {
    for ($i = 0; $i -lt 90; $i++) {
        try {
            $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 3
            if ("$h" -match "OK") {
                $st = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 3
                if ($st.file_index_ready) { return $true }
            }
        } catch {}
        if ($gui.HasExited) {
            Log "ERROR: GUI exited during startup code=$($gui.ExitCode)"
            return $false
        }
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
    if ($ui.Content -notmatch "SafeRoute|SecureModelRoute") {
        Log "ERROR: admin UI missing marker"
        exit 1
    }
    Log "Admin UI OK bytes=$($ui.Content.Length)"
} catch {
    Log "ERROR: admin UI: $($_.Exception.Message)"
    exit 1
}

Log "==> Tray smoke: GUI process alive while API responds"
if ($gui.HasExited) {
    Log "ERROR: GUI exited early"
    exit 1
}
$health2 = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 5
Log "Health in background mode: $health2"

Log "==> blackbox_test.py (attach mode, 27 scenarios)"
$env:SMR_ATTACH = "1"
$env:SMR_BASE = $Base
$env:SMR_KEYS_FILE = Join-Path $TestRoot "test_model_api_key.txt"
$env:PYTHONUTF8 = "1"
Set-Location $TestRoot
& $python (Join-Path $TestRoot "scripts\blackbox_test.py") 2>&1 | ForEach-Object { Log $_ }
$bb = $LASTEXITCODE

if (-not $gui.HasExited) {
    Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
}
Get-Process smr -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

if ($bb -ne 0) {
    Log "INSTALLED-APP TEST FAILED (blackbox exit=$bb)"
    exit 1
}

Log "INSTALLED-APP TEST PASSED"
exit 0
