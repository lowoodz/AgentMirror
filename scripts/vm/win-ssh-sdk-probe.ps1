# Quick SDK probe on Windows VM (run via SSH after syncing dist binaries).
param(
    [string]$Base = "http://127.0.0.1:8080",
    [string]$StageDir = "",
    [string]$Prefix = "",
    [string]$SecretsDir = "",
    [string]$ConfigPath = "",
    [string]$TestRoot = "",
    [string]$LogPath = ""
)

$GuestStaging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
if (-not $StageDir) { $StageDir = Join-Path $GuestStaging "smr-app-test-stage" }
if (-not $Prefix) { $Prefix = Join-Path $GuestStaging "smr-app-test-home" }
if (-not $SecretsDir) { $SecretsDir = Join-Path $GuestStaging "smr-app-test-secrets" }
if (-not $ConfigPath) { $ConfigPath = Join-Path $GuestStaging "smr-app-test-home\smr.yaml" }
if (-not $TestRoot) { $TestRoot = Join-Path $GuestStaging "smr-test-suite" }
if (-not $LogPath) { $LogPath = Join-Path $GuestStaging "smr-ssh-sdk-probe.log" }

$ErrorActionPreference = "Stop"
$python = Join-Path $GuestStaging "python312\python.exe"
Remove-Item $LogPath -Force -ErrorAction SilentlyContinue

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content $LogPath $line -Encoding UTF8
    Write-Host $line
}

function Stop-ListeningServer {
    for ($attempt = 0; $attempt -lt 12; $attempt++) {
        foreach ($name in @("smr", "smr-gui", "SafeRoute")) {
            Get-Process $name -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        }
        cmd.exe /c "taskkill /F /IM smr.exe /T 2>nul & taskkill /F /IM SafeRoute.exe /T 2>nul" | Out-Null
        Start-Sleep -Seconds 2
        $alive = $false
        try {
            $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 2
            if ("$h" -match "OK") { $alive = $true }
        } catch {}
        if (-not $alive) { return }
    }
    throw "could not free $Base"
}

Log "==> SSH SDK probe"
Stop-ListeningServer

$GuiDir = Join-Path $Prefix "Programs\SafeRoute"
$BinDir = Join-Path $Prefix "bin"
New-Item -ItemType Directory -Force -Path $BinDir, $GuiDir, $SecretsDir | Out-Null
Set-Content -Path (Join-Path $SecretsDir "project.txt") -Value "probe-secret-data" -Encoding UTF8

foreach ($pair in @(
        @((Join-Path $StageDir "smr.exe"), (Join-Path $BinDir "smr.exe")),
        @((Join-Path $StageDir "SafeRoute.exe"), (Join-Path $GuiDir "SafeRoute.exe"))
    )) {
    $src, $dst = $pair
    if (-not (Test-Path $src)) { throw "missing $src" }
    if (Test-Path $dst) { Remove-Item $dst -Force }
    Copy-Item $src $dst -Force
    if ((Get-Item $src).Length -ne (Get-Item $dst).Length) {
        throw "copy failed: $src -> $dst"
    }
    Log "verified copy $(Split-Path $dst -Leaf) $((Get-Item $dst).Length) bytes"
}

& $python (Join-Path $TestRoot "scripts\generate_test_config.py") $ConfigPath $SecretsDir | ForEach-Object { Log $_ }
$env:SMR_FORCE_SERVER = "1"
$env:SMR_CONFIG = $ConfigPath
$gui = Start-Process -FilePath (Join-Path $GuiDir "SafeRoute.exe") -ArgumentList @("--background") -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 12
for ($i = 0; $i -lt 90; $i++) {
    try {
        $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 3
        $st = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 3
        if ("$h" -match "OK" -and $st.file_index_ready) { break }
    } catch {}
    if ($gui.HasExited) { throw "GUI exited" }
    Start-Sleep -Seconds 1
}
$ui = Invoke-WebRequest -Uri "$Base/ui" -TimeoutSec 15 -UseBasicParsing
Log "Admin UI bytes=$($ui.Content.Length)"

Set-Location $TestRoot
& $python -c @"
import sys, tempfile
from pathlib import Path
sys.path.insert(0, r'$TestRoot\scripts')
import blackbox_test as bb
from blackbox_test import Report, apply_test_config, scenario_openai_sdk_client, scenario_openai_python_sdk
from blackbox_test import start_mock, MockEmptySseHandler, MockDangerousJsonHandler, MockDangerousSseHandler, MockAnthropicJsonHandler
from test_common import parse_keys

bb.BASE = '$Base'
glm, ds = parse_keys()
secrets = Path(tempfile.mkdtemp(prefix='smr-blackbox-secrets-'))
(secrets / 'project.txt').write_text(bb.FILE_SECRET, encoding='utf-8')
(secrets / 'other.txt').write_text(bb.OTHER_FILE_SECRET, encoding='utf-8')
parent = secrets / 'parent'
child = parent / 'child'
parent.mkdir()
child.mkdir()
(parent / 'top.txt').write_text(bb.PARENT_ONLY_SECRET, encoding='utf-8')
(child / 'report.txt').write_text(bb.CHILD_ONLY_SECRET, encoding='utf-8')
ports = {'ops_json': 18191, 'ops_sse': 18192, 'empty_sse': 18193, 'anthropic_json': 18194}
for p,h in [(18191, MockDangerousJsonHandler),(18192, MockDangerousSseHandler),(18193, MockEmptySseHandler),(18194, MockAnthropicJsonHandler)]:
    start_mock(p, h)
assert apply_test_config(glm, ds, secrets, ports)
r = Report()
scenario_openai_sdk_client(r)
scenario_openai_python_sdk(r)
for s in r.scenarios:
    print(f'{s.name}: {\"PASS\" if s.ok else \"FAIL\"} {s.detail}', flush=True)
raise SystemExit(0 if r.failed == 0 else 1)
"@ 2>&1 | ForEach-Object { Log $_ }

Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
Log "DONE rc=$LASTEXITCODE"
