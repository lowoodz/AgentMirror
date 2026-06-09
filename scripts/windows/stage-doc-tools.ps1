# Stage poppler pdftotext (+ DLLs) for Windows SafeRoute bundles.
param(
    [Parameter(Mandatory = $false)]
    [string]$Root = "",
    [string]$OutDir = "",
    [string]$Arch = "x64"
)

$ErrorActionPreference = "Stop"

if (-not $Root) {
    $Root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
}
if (-not $OutDir) {
    $OutDir = Join-Path $Root "resources\doc-tools"
}

$PopplerVersion = "24.08.0-0"
$Stage = Join-Path $OutDir "windows-$Arch"
$Bin = Join-Path $Stage "bin"
$Lib = Join-Path $Stage "lib"
$Cache = Join-Path $Root "dist\vendor-cache"

New-Item -ItemType Directory -Force -Path $Bin, $Lib, $Cache | Out-Null
if (Test-Path $Stage) {
    Remove-Item $Stage -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $Bin, $Lib | Out-Null

$ZipName = "Release-$PopplerVersion.zip"
$ZipUrl = "https://github.com/oschwartz10612/poppler-windows/releases/download/v$PopplerVersion/$ZipName"
$ZipPath = Join-Path $Cache $ZipName

if (-not (Test-Path $ZipPath)) {
    Write-Host "==> Download poppler-windows $PopplerVersion"
    Invoke-WebRequest -Uri $ZipUrl -OutFile $ZipPath -UseBasicParsing
}

$Extract = Join-Path $Cache "poppler-$PopplerVersion"
if (-not (Test-Path $Extract)) {
    $tmp = Join-Path $Cache "extract-$PopplerVersion"
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    Expand-Archive -Path $ZipPath -DestinationPath $tmp -Force
    $top = Get-ChildItem $tmp -Directory | Select-Object -First 1
    if (-not $top) { throw "poppler zip extract failed" }
    Move-Item $top.FullName $Extract -Force
}

$PopplerBin = Join-Path $Extract "Library\bin"
if (-not (Test-Path (Join-Path $PopplerBin "pdftotext.exe"))) {
    throw "pdftotext.exe not found in $PopplerBin"
}

Copy-Item (Join-Path $PopplerBin "pdftotext.exe") (Join-Path $Bin "pdftotext.exe") -Force
Get-ChildItem $PopplerBin -Filter "*.dll" | ForEach-Object {
    Copy-Item $_.FullName (Join-Path $Lib $_.Name) -Force
    Copy-Item $_.FullName (Join-Path $Bin $_.Name) -Force
}

Write-Host "==> staged doc-tools at $Stage"
Get-ChildItem $Bin | Select-Object Name, Length
