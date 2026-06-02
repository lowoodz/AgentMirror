# Run unit tests, smoke verify, black-box scenarios, and stress tests (Windows).
# Same host matrix as scripts/run_all_tests.sh on macOS (verify → install functional → blackbox → stress).
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not (Test-Path (Join-Path $Root "Cargo.toml"))) {
    $Root = Split-Path -Parent $Root
}
Set-Location $Root

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CARGO_TARGET_DIR = Join-Path $Root "target"
$env:PYTHONUTF8 = "1"

Write-Host "========== 1/5 Unit + smoke (verify.ps1) =========="
& (Join-Path $Root "scripts\verify.ps1")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Keys = Join-Path $Root "test_model_api_key.txt"
if (-not (Test-Path $Keys)) {
    Write-Host "Skip live tests: copy test_model_api_key.example.txt to test_model_api_key.txt (gitignored) and add your keys"
    exit 0
}

$python = $null
foreach ($cmd in @("python", "py", "python3")) {
    if (Get-Command $cmd -ErrorAction SilentlyContinue) { $python = $cmd; break }
}
if (-not $python) {
    Write-Error "Python not found (required for blackbox/stress tests)"
}

Write-Host "========== 2/5 Install functional smoke =========="
& $python (Join-Path $Root "scripts\install_functional_test.py")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "========== 3/5 Black-box scenarios =========="
& $python (Join-Path $Root "scripts\blackbox_test.py")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "========== 4/5 Stress tests =========="
& $python (Join-Path $Root "scripts\live_test.py")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "========== All test suites passed =========="
