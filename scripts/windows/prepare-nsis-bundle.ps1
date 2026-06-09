# Stage smr.exe + example config into gui/bundle-extra for Tauri NSIS resources.
param(
    [Parameter(Mandatory = $true)]
    [string]$Root,
    [string]$TargetTriple = ""
)

$ErrorActionPreference = "Stop"

$BundleExtra = Join-Path $Root "gui\bundle-extra"
New-Item -ItemType Directory -Force -Path $BundleExtra | Out-Null

$candidates = @()
if ($TargetTriple) {
    $candidates += Join-Path $Root "target\$TargetTriple\release\smr.exe"
}
$candidates += Join-Path $Root "target\release\smr.exe"

$SmrExe = $null
foreach ($path in $candidates) {
    if (Test-Path $path) {
        $SmrExe = $path
        break
    }
}
if (-not $SmrExe) {
    throw "smr.exe not found. Build CLI first: cargo build --release -p smr-cli"
}

$Example = Join-Path $Root "config\smr.example.yaml"
if (-not (Test-Path $Example)) {
    throw "Missing config\smr.example.yaml"
}

Copy-Item $SmrExe (Join-Path $BundleExtra "smr.exe") -Force
Copy-Item $Example (Join-Path $BundleExtra "smr.example.yaml") -Force

$StageDocTools = Join-Path $Root "scripts\windows\stage-doc-tools.ps1"
if (Test-Path $StageDocTools) {
    & $StageDocTools -Root $Root
}

Write-Host "==> NSIS bundle extras: $BundleExtra"
