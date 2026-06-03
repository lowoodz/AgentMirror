# Smoke verification on Windows (health / status / ui).
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not (Test-Path (Join-Path $Root "Cargo.toml"))) {
    $Root = Split-Path -Parent $Root
}
Set-Location $Root

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CARGO_TARGET_DIR = Join-Path $Root "target"

Write-Host "==> cargo test"
cargo test --quiet
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "==> cargo build --release"
cargo build --release --quiet -p smr-cli
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Bin = Join-Path $Root "target\release\smr.exe"
$Port = 18080
$Cfg = Join-Path $Root "config\smr.example.yaml"
$TmpCfg = [System.IO.Path]::GetTempFileName()
(Get-Content $Cfg -Raw) -replace "127.0.0.1:8080", "127.0.0.1:$Port" | Set-Content $TmpCfg -Encoding UTF8

$Proc = Start-Process -FilePath $Bin -ArgumentList @("--config", $TmpCfg) -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 2

try {
    Write-Host "==> health"
    $health = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/health" -TimeoutSec 5
    if ("$health" -notmatch "OK") { throw "health check failed: $health" }

    Write-Host "==> api status"
    $status = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/status" -TimeoutSec 5
    if (-not $status.proxy_url) { throw "status missing proxy_url" }

    Write-Host "==> ui"
    $ui = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/ui" -TimeoutSec 5 -UseBasicParsing
    if ($ui.Content -notmatch "SafeRoute|SecureModelRoute") { throw "ui page missing title" }

    Write-Host ""
    Write-Host "All verification checks passed."
}
finally {
    if ($Proc -and -not $Proc.HasExited) {
        Stop-Process -Id $Proc.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item $TmpCfg -Force -ErrorAction SilentlyContinue
}
