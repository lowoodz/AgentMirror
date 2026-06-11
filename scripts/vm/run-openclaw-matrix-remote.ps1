param(
    [switch]$KeepMatrixConfig
)

$ErrorActionPreference = "Stop"
$GuestWork = $env:SMR_GUEST_WORK
if (-not $GuestWork) { $GuestWork = Join-Path $env:SMR_GUEST_STAGING "openclaw-matrix" }

$CfgDir = Join-Path $env:APPDATA "securemodelroute"
$Cfg = Join-Path $CfgDir "smr.yaml"
$Backup = Join-Path $CfgDir "smr.yaml.matrix-backup"
$MatrixCfg = Join-Path $GuestWork "smr.yaml"
$EnvFile = Join-Path $GuestWork "matrix.env"
$Log = Join-Path $GuestWork "matrix-test.log"
$Base = "http://127.0.0.1:8080"

function Restore-Config {
    if (Test-Path $Backup) {
        Copy-Item -Force $Backup $Cfg
        Remove-Item -Force $Backup -ErrorAction SilentlyContinue
        try { Invoke-WebRequest -Uri "$Base/api/reload" -Method PUT -TimeoutSec 120 | Out-Null } catch {}
        Write-Host "==> Restored smr.yaml from backup"
    }
}

function Wait-Health {
    param([int]$TimeoutSec = 120)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri "$Base/health" -TimeoutSec 4 -UseBasicParsing
            if ($r.StatusCode -eq 200) { return $true }
        } catch {}
        Start-Sleep -Seconds 2
    }
    return $false
}

function Stop-SafeRoute {
    foreach ($name in @("smr", "SafeRoute", "smr-gui")) {
        Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "Stopping $($_.Name) pid=$($_.Id)"
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Seconds 2
    $conn = Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue
    foreach ($c in $conn) {
        if ($c.OwningProcess) {
            Stop-Process -Id $c.OwningProcess -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Seconds 1
}

function Start-SafeRouteWithConfig {
    param([string]$ConfigPath)
    $candidates = @(
        (Join-Path $env:LOCALAPPDATA "SafeRoute\smr-gui.exe"),
        (Join-Path $env:LOCALAPPDATA "SafeRoute\SafeRoute.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\SafeRoute\SafeRoute.exe"),
        (Join-Path ${env:ProgramFiles} "SafeRoute\SafeRoute.exe"),
        (Join-Path $env:SMR_GUEST_STAGING "smr-desktop-out\SafeRoute.exe")
    )
    foreach ($exe in $candidates) {
        if (-not ($exe -and (Test-Path $exe))) { continue }
        Write-Host "==> Starting SafeRoute: $exe (SMR_CONFIG=$ConfigPath)"
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $exe
        $psi.UseShellExecute = $false
        $psi.Environment["SMR_CONFIG"] = $ConfigPath
        [void][System.Diagnostics.Process]::Start($psi)
        if (Wait-Health -TimeoutSec 90) { return $true }
    }
    return $false
}

New-Item -ItemType Directory -Force -Path $CfgDir, $GuestWork | Out-Null
$env:Path = "$env:Path;$env:APPDATA\npm"

$python = "$env:SMR_GUEST_STAGING/python312/python.exe"
if (-not (Test-Path $python)) { $python = "C:/Users/Public/python312/python.exe" }
if (-not (Test-Path $python)) { $python = "python" }

$keysFile = Join-Path $GuestWork "test_model_api_key.txt"
if (Test-Path $keysFile) { $env:SMR_KEYS_FILE = $keysFile }

if (-not (Test-Path $MatrixCfg)) { throw "Missing $MatrixCfg (host should upload smr.yaml)" }
if (-not (Test-Path $EnvFile)) { throw "Missing $EnvFile" }

$matrixRoot = ""
Get-Content $EnvFile | ForEach-Object {
    if ($_ -match '^SMR_MATRIX_ROOT=(.+)$') { $matrixRoot = $Matches[1].Trim() }
}
if (-not $matrixRoot) { throw "SMR_MATRIX_ROOT missing in $EnvFile" }

$fixturePy = @"
import os, sys
sys.path.insert(0, r'$GuestWork')
os.environ['SMR_MATRIX_ROOT'] = r'$matrixRoot'
from openclaw_matrix_common import ensure_fixtures
from pathlib import Path
paths = ensure_fixtures(Path(r'$matrixRoot'))
print('fixtures', paths['matrix_root'])
"@
& $python -c $fixturePy

if (Test-Path $Backup) { Remove-Item -Force $Backup -ErrorAction SilentlyContinue }
if (Test-Path $Cfg) { Copy-Item -Force $Cfg $Backup }
Copy-Item -Force $MatrixCfg $Cfg
Write-Host "==> Deployed matrix smr.yaml to $Cfg"

$extraCfg = Join-Path $env:SMR_GUEST_STAGING "smr-install-smoke-home/etc/securemodelroute/smr.yaml"
if (Test-Path (Split-Path $extraCfg -Parent)) {
    Copy-Item -Force $MatrixCfg $extraCfg
    Write-Host "==> Deployed matrix smr.yaml to $extraCfg"
}

Stop-SafeRoute
if (-not (Start-SafeRouteWithConfig -ConfigPath $Cfg)) { throw "SafeRoute not listening on $Base" }

$reloadOk = $false
for ($i = 0; $i -lt 5; $i++) {
    try {
        $resp = Invoke-WebRequest -Uri "$Base/api/reload" -Method PUT -TimeoutSec 180 -UseBasicParsing
        if ($resp.StatusCode -eq 200) { $reloadOk = $true; break }
    } catch {
        Write-Host "reload attempt $($i + 1) failed: $_"
        Start-Sleep -Seconds 3
    }
}
if (-not $reloadOk) { throw "api/reload failed after deploying matrix smr.yaml" }

$st = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 10
Write-Host "==> Active config: $($st.config_path)"
if ($st.config_path -notlike "*securemodelroute*") {
    throw "unexpected config_path: $($st.config_path)"
}

$deadline = (Get-Date).AddMinutes(3)
$fileIndexReady = $false
while ((Get-Date) -lt $deadline) {
    try {
        $st = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 5
        if ($st.file_index_ready) { $fileIndexReady = $true; break }
    } catch {}
    Start-Sleep -Seconds 2
}
if (-not $fileIndexReady) { throw "file_index_ready timeout" }

$bootstrap = Join-Path $env:USERPROFILE ".openclaw\workspace\BOOTSTRAP.md"
if (Test-Path $bootstrap) {
    Remove-Item -Force $bootstrap
    Write-Host "==> Removed BOOTSTRAP.md for matrix E2E"
}

$generatePy = Join-Path $GuestWork "generate_openclaw_saferoute_config.py"
if (Test-Path $generatePy) {
    & $python $generatePy --force | Write-Host
}

$patchPy = Join-Path $GuestWork "patch_openclaw_saferoute.py"
if (Test-Path $patchPy) {
    & $python $patchPy | Write-Host
    $env:Path = "$env:Path;$env:APPDATA\npm"
    $openclaw = Get-Command openclaw.cmd -ErrorAction SilentlyContinue
    if ($openclaw) {
        & $openclaw.Source gateway restart 2>$null
        Start-Sleep -Seconds 4
        Write-Host "==> openclaw gateway restarted"
    }
}

$testScript = Join-Path $GuestWork "openclaw_security_matrix_test.py"
Remove-Item -Force $Log -ErrorAction SilentlyContinue
$env:PYTHONUNBUFFERED = "1"
$env:PYTHONIOENCODING = "utf-8"
$env:PYTHONUTF8 = "1"
& $python $testScript --env-file $EnvFile *> $Log
Get-Content $Log
$rc = $LASTEXITCODE

if (-not $KeepMatrixConfig) {
    Restore-Config
    Stop-SafeRoute
    Start-SafeRouteWithConfig -ConfigPath $Cfg | Out-Null
} else {
    Write-Host "==> Keeping matrix smr.yaml"
}

exit $rc
