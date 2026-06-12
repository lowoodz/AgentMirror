# Run unit tests, smoke verify, transparency pass-through, black-box, and stress tests (Windows).
# Same host matrix as scripts/run_all_tests.sh (verify → transparency → install functional → blackbox → stress).
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not (Test-Path (Join-Path $Root "Cargo.toml"))) {
    $Root = Split-Path -Parent $Root
}
Set-Location $Root

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CARGO_TARGET_DIR = Join-Path $Root "target"
$env:PYTHONUTF8 = "1"

Write-Host "========== 1/6 Unit + smoke (verify.ps1) =========="
& (Join-Path $Root "scripts\verify.ps1")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$python = $null
foreach ($cmd in @("python", "py", "python3")) {
    if (Get-Command $cmd -ErrorAction SilentlyContinue) { $python = $cmd; break }
}
if (-not $python) {
    Write-Error "Python not found (required for transparency/blackbox/stress tests)"
}

Write-Host ""
Write-Host "========== 2/6 Transparency pass-through (mock) =========="
& $python (Join-Path $Root "scripts\transparency_pass_through_test.py") --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

. (Join-Path $Root "scripts\load_test_env.ps1")
if (-not (Test-SmrKeys)) {
    Write-Host "Skip live API tests: copy config/test.env.example to config/test.env and set SMR_GLM_API_KEY / SMR_DEEPSEEK_API_KEY"
    exit 0
}

Write-Host ""
Write-Host "========== 3/6 Install functional smoke =========="
& $python (Join-Path $Root "scripts\install_functional_test.py")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "========== 4/6 Black-box scenarios =========="
& $python (Join-Path $Root "scripts\blackbox_test.py")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "========== 5/6 Stress tests =========="
& $python (Join-Path $Root "scripts\live_test.py")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host ""
Write-Host "========== All test suites passed =========="
