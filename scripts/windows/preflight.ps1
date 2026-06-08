# Preflight checks before Windows package / install / test.
param(
    [switch]$RequireNsis,
    [switch]$RequireNode,
    [switch]$RequirePython,
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$Root = Get-SmrRoot -StartDir $PSScriptRoot
Set-SmrBuildEnv -Root $Root

function Report($ok, $msg) {
    if (-not $Quiet) {
        $tag = if ($ok) { "OK" } else { "FAIL" }
        Write-Host "[$tag] $msg"
    }
    return $ok
}

$failures = @()

function Require($cond, $msg) {
    Report $cond $msg | Out-Null
    if (-not $cond) { $script:failures += $msg }
}

Require (Test-Path (Join-Path $Root "Cargo.toml")) "repo root: $Root"
Require (Get-Command cargo -ErrorAction SilentlyContinue) "cargo in PATH ($env:USERPROFILE\.cargo\bin)"

$rustc = & cargo --version 2>$null
Require ($LASTEXITCODE -eq 0) "cargo works: $rustc"

if ($RequireNode -or (Test-Path (Join-Path $Root "gui\package.json"))) {
    Require (Get-Command npm -ErrorAction SilentlyContinue) "npm in PATH (Node.js required for GUI/NSIS)"
    if (Get-Command npm -ErrorAction SilentlyContinue) {
        $npmv = & npm --version 2>$null
        Require ($LASTEXITCODE -eq 0) "npm works: v$npmv"
    }
}

if ($RequireNsis) {
    . "$PSScriptRoot\ensure-nsis-tools.ps1" -BuildRoot $Root | Out-Null
    $makensis = @(
        "${env:ProgramFiles(x86)}\NSIS\Bin\makensis.exe",
        "$env:ProgramFiles\NSIS\Bin\makensis.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
    Require ($makensis) "makensis.exe (run: winget install -e --id NSIS.NSIS)"
}

if ($RequirePython) {
    $py = $null
    foreach ($cmd in @("python", "py", "python3")) {
        if (Get-Command $cmd -ErrorAction SilentlyContinue) { $py = $cmd; break }
    }
    Require ($py) "Python for blackbox/functional tests"
}

$targetDir = Join-Path $Root "target"
if ($env:CARGO_TARGET_DIR -and $env:CARGO_TARGET_DIR -ne $targetDir) {
    Require $false "CARGO_TARGET_DIR=$($env:CARGO_TARGET_DIR) (expected $targetDir — stale binary risk)"
} else {
    Report $true "CARGO_TARGET_DIR=$targetDir" | Out-Null
}

$policy = Get-ExecutionPolicy -Scope Process
if ($policy -eq "Restricted") {
    Require $false "ExecutionPolicy is Restricted (use Bypass -Scope Process for scripts)"
} else {
    Report $true "ExecutionPolicy (Process): $policy" | Out-Null
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Error ("Preflight failed:`n - " + ($failures -join "`n - "))
}

if (-not $Quiet) {
    Write-Host ""
    Write-Host "Preflight passed."
}
exit 0
