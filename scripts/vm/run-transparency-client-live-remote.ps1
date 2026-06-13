param(
    [string]$GuestWork = "",
    [string]$CountDir = ""
)

$ErrorActionPreference = "Stop"
if (-not $env:SMR_GUEST_STAGING) {
    $env:SMR_GUEST_STAGING = Join-Path $env:USERPROFILE "smr-staging"
}
if (-not $GuestWork) { $GuestWork = Join-Path $env:SMR_GUEST_STAGING "transparency-client-live" }
if (-not $CountDir) { $CountDir = $env:USERPROFILE }
if ($env:SMR_TRANSPARENCY_COUNT_DIR) {
    $candidate = $env:SMR_TRANSPARENCY_COUNT_DIR
    if (Test-Path $candidate) { $CountDir = $candidate }
}

$CfgDir = Join-Path $env:APPDATA "securemodelroute"
$Cfg = Join-Path $CfgDir "smr.yaml"
$Backup = Join-Path $CfgDir "smr.yaml.transparency-backup"
$Base = "http://127.0.0.1:8080"
$Log = Join-Path $GuestWork "transparency-client-live.log"

function Restore-Config {
    if (Test-Path $Backup) {
        Copy-Item -Force $Backup $Cfg
        Remove-Item -Force $Backup -ErrorAction SilentlyContinue
        try { Invoke-WebRequest -Uri "$Base/api/reload" -Method PUT -TimeoutSec 120 | Out-Null } catch {}
        Write-Host "==> Restored smr.yaml from backup"
    }
}

function Wait-Health {
    param([int]$TimeoutSec = 90)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
            if ($curl) {
                $out = & $curl.Source -sf --max-time 4 "$Base/health" 2>$null
                if ($out -match "OK") { return $true }
            }
            $r = Invoke-WebRequest -Uri "$Base/health" -TimeoutSec 4 -UseBasicParsing
            if ($r.StatusCode -eq 200 -and $r.Content -match "OK") { return $true }
        } catch {}
        Start-Sleep -Seconds 2
    }
    return $false
}

function Stop-SafeRoute {
    foreach ($name in @("smr", "SafeRoute", "smr-gui")) {
        Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object {
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Seconds 2
}

function Start-SafeRouteWithConfig {
    param([string]$ConfigPath)
    $cli = Join-Path $env:SMR_GUEST_STAGING "smr-home\bin\smr.exe"
    if (Test-Path $cli) {
        Write-Host "==> Starting smr CLI: $cli --config $ConfigPath"
        Start-Process -FilePath $cli -ArgumentList @("--config", $ConfigPath) | Out-Null
        Start-Sleep -Seconds 4
        if (Wait-Health -TimeoutSec 60) { return $true }
        Write-Host "WARN: smr CLI started but health check failed"
    }
    $guiCandidates = @(
        (Join-Path $env:LOCALAPPDATA "SafeRoute\SafeRoute.exe"),
        (Join-Path $env:LOCALAPPDATA "SafeRoute\smr-gui.exe"),
        (Join-Path $env:LOCALAPPDATA "Programs\SafeRoute\SafeRoute.exe"),
        (Join-Path ${env:ProgramFiles} "SafeRoute\SafeRoute.exe"),
        (Join-Path $env:SMR_GUEST_STAGING "smr-desktop-out\SafeRoute.exe")
    )
    foreach ($exe in $guiCandidates) {
        if (-not ($exe -and (Test-Path $exe))) { continue }
        Write-Host "==> Starting SafeRoute GUI: $exe"
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $exe
        $psi.UseShellExecute = $false
        $psi.Environment["SMR_CONFIG"] = $ConfigPath
        [void][System.Diagnostics.Process]::Start($psi)
        Start-Sleep -Seconds 6
        if (Wait-Health -TimeoutSec 60) { return $true }
    }
    return $false
}

New-Item -ItemType Directory -Force -Path $CfgDir, $GuestWork | Out-Null
$env:Path = "$env:Path;$env:APPDATA\npm"

$python = Join-Path $env:SMR_GUEST_STAGING "python312\python.exe"
if (-not (Test-Path $python)) { $python = "python" }

$envFile = Join-Path $GuestWork "test.env"
if (Test-Path $envFile) {
    Get-Content $envFile | ForEach-Object {
        if ($_ -match '^\s*([A-Za-z_][A-Za-z0-9_]*)=(.*)$') {
            $k = $Matches[1]; $v = $Matches[2].Trim().Trim('"').Trim("'")
            if ($k -in @("SMR_GLM_API_KEY", "SMR_DEEPSEEK_API_KEY", "SMR_DEEPSEEK_BASE_URL", "SMR_DEEPSEEK_ANTHROPIC_BASE_URL", "SMR_TRANSPARENCY_COUNT_DIR", "SMR_TRANSPARENCY_OPENAI_MODEL", "SMR_TRANSPARENCY_ANTHROPIC_MODEL")) {
                Set-Item -Path "env:$k" -Value $v
            }
        }
    }
}
$guestSmr = Join-Path $env:SMR_GUEST_STAGING "smr-home\bin\smr.exe"
if (-not (Test-Path $guestSmr)) {
    $env:SMR_GUEST_STAGING = Join-Path $env:USERPROFILE "smr-staging"
    Write-Host "==> Using guest staging: $($env:SMR_GUEST_STAGING)"
}

$transparencyCfg = Join-Path $GuestWork "smr-transparency.yaml"
if (-not (Test-Path $transparencyCfg)) { throw "Missing $transparencyCfg" }

if (Test-Path $Cfg) { Copy-Item -Force $Cfg $Backup }
Copy-Item -Force $transparencyCfg $Cfg
Write-Host "==> Deployed transparency smr.yaml"

Stop-SafeRoute
if (-not (Start-SafeRouteWithConfig -ConfigPath $Cfg)) { throw "SafeRoute not listening on $Base" }
try { Invoke-WebRequest -Uri "$Base/api/reload" -Method PUT -TimeoutSec 120 | Out-Null } catch {}

Write-Host "==> HTTP wire transparency (mock upstream)"
& $python (Join-Path $GuestWork "transparency_pass_through_test.py") --release
if ($LASTEXITCODE -ne 0) { throw "transparency_pass_through_test.py failed" }

$env:SMR_TRANSPARENCY_COUNT_DIR = $CountDir
$env:SMR_TRANSPARENCY_WORKDIR = $GuestWork
Write-Host "==> Client live E2E count_dir=$CountDir workdir=$GuestWork"
Push-Location $GuestWork
try {
    & $python (Join-Path $GuestWork "transparency_client_live_test.py") --attach *> $Log
    Get-Content $Log
    $code = $LASTEXITCODE
} finally {
    Pop-Location
    Restore-Config
    Stop-SafeRoute
}
exit $code
