# Verify dist/ artifacts after scripts/package.ps1.
param(
    [switch]$RequireSetup,
    [switch]$RequireAppZip,
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$Root = Get-SmrRoot -StartDir $PSScriptRoot
$art = Get-SmrDistArtifacts -Root $Root
$failures = @()

function CheckFile($path, $label) {
    if (-not (Test-Path $path)) {
        $failures += "Missing $label`: $path"
        return
    }
    $item = Get-Item $path
    if ($item.Length -lt 1024) {
        $failures += "Suspiciously small $label ($($item.Length) bytes): $path"
        return
    }
    if (-not $Quiet) {
        Write-Host "[OK] $label ($([math]::Round($item.Length / 1MB, 2)) MB): $(Split-Path $path -Leaf)"
    }
}

CheckFile $art.CliZip "CLI zip"
if ($RequireSetup) { CheckFile $art.SetupExe "NSIS setup" }
if ($RequireAppZip) { CheckFile $art.AppZip "App zip" }

$smrInDist = Join-Path $art.DistDir "smr.exe"
if (Test-Path $smrInDist) {
    $ver = & $smrInDist --version 2>&1
    if ($LASTEXITCODE -ne 0) {
        $failures += "smr.exe --version failed: $ver"
    } elseif (-not $Quiet) {
        Write-Host "[OK] smr.exe --version: $($ver -join ' ')"
    }
}

if ($failures.Count -gt 0) {
    Write-Error ("Package verification failed:`n - " + ($failures -join "`n - "))
}

if (-not $Quiet) {
    Write-Host ""
    Write-Host "Package verification passed (v$($art.Version))."
}
exit 0
