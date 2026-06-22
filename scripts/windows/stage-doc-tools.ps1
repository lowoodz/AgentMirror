# Stage poppler pdftotext (+ DLLs) for Windows SafeRoute bundles.
param(
    [Parameter(Mandatory = $false)]
    [string]$Root = "",
    [string]$OutDir = "",
    [string]$Arch = "x64"
)

$ErrorActionPreference = "Stop"

function Get-PeMachine {
    param([string]$Path)
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { return "invalid" }
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    if ($pe + 6 -gt $bytes.Length) { return "invalid" }
    $machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
    switch ($machine) {
        0x8664 { return "x64" }
        0x014c { return "x86" }
        0xAA64 { return "arm64" }
        default { return ("0x{0:X4}" -f $machine) }
    }
}

function Test-PeX64 {
    param([string]$Path)
    (Get-PeMachine $Path) -eq "x64"
}

function Invoke-VcredistExtract {
    param([string]$BinDir, [string]$RepoRoot)
    $cache = Join-Path $RepoRoot "dist\vendor-cache"
    $exe = Join-Path $cache "vc_redist.x64.exe"
    if (-not (Test-Path $exe)) {
        New-Item -ItemType Directory -Force -Path $cache | Out-Null
        $url = "https://aka.ms/vs/17/release/vc_redist.x64.exe"
        Invoke-WebRequest -Uri $url -OutFile $exe -UseBasicParsing
    }
    $out = Join-Path $env:TEMP ("smr-vcrt-extract-" + [guid]::NewGuid().ToString("n"))
    Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    $proc = Start-Process -FilePath $exe -ArgumentList @("/extract:$out", "/quiet", "/norestart") -Wait -PassThru
    if ($proc.ExitCode -ne 0 -and $proc.ExitCode -ne 1638) {
        throw "vc_redist.x64.exe /extract failed with exit $($proc.ExitCode)"
    }
    foreach ($dll in @("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll")) {
        $found = Get-ChildItem -Path $out -Recurse -Filter $dll -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $found) { throw "no x64 ${dll} in vc_redist extract output" }
        if (-not (Test-PeX64 $found.FullName)) {
            throw "$($found.FullName) is $(Get-PeMachine $found.FullName), expected x64"
        }
        Copy-Item $found.FullName (Join-Path $BinDir $dll) -Force
    }
    Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
}

function Invoke-StageVcRuntime {
    param([string]$BinDir, [string]$RepoRoot)
    $VcrtScript = Join-Path $RepoRoot "scripts\vendor\stage-vcrt-dlls.sh"
    if (Test-Path $VcrtScript) {
        $bash = Get-Command bash -ErrorAction SilentlyContinue
        if ($bash) {
            & $bash.Source $VcrtScript $BinDir
            if ($LASTEXITCODE -ne 0) { throw "stage-vcrt-dlls.sh failed" }
            return
        }
    }
    $cacheDir = Join-Path $RepoRoot "dist\vendor-cache\vcrt-x64"
    $cached = @("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll") | Where-Object {
        Test-Path (Join-Path $cacheDir $_)
    }
    if ($cached.Count -eq 3) {
        foreach ($dll in $cached) {
            $src = Join-Path $cacheDir $dll
            if (-not (Test-PeX64 $src)) { throw "${src} is not x64" }
            Copy-Item $src (Join-Path $BinDir $dll) -Force
        }
        return
    }
    Invoke-VcredistExtract -BinDir $BinDir -RepoRoot $RepoRoot
}

function Test-WindowsDocToolsBin {
    param([string]$BinDir)
    $pdftotext = Join-Path $BinDir "pdftotext.exe"
    if (-not (Test-Path $pdftotext)) { throw "missing pdftotext.exe" }
    if (-not (Test-PeX64 $pdftotext)) {
        throw "pdftotext.exe is $(Get-PeMachine $pdftotext), expected x64"
    }
    $stray = Join-Path $BinDir "pdftotext"
    if ((Test-Path $stray) -and -not $stray.EndsWith(".exe")) {
        Remove-Item $stray -Force
    }
    foreach ($dll in @("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll")) {
        $path = Join-Path $BinDir $dll
        if (-not (Test-Path $path)) { throw "missing ${dll}" }
        if (-not (Test-PeX64 $path)) {
            throw "${dll} is $(Get-PeMachine $path), expected x64"
        }
    }
}

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

if (Test-Path (Join-Path $Bin "pdftotext.exe")) {
    foreach ($skip in @("poppler-glib.dll", "poppler-cpp.dll")) {
        Remove-Item (Join-Path $Bin $skip) -Force -ErrorAction SilentlyContinue
        Remove-Item (Join-Path $Lib $skip) -Force -ErrorAction SilentlyContinue
    }
    Invoke-StageVcRuntime -BinDir $Bin -RepoRoot $Root
    Test-WindowsDocToolsBin -BinDir $Bin
    Write-Host "==> doc-tools already staged at $Stage (skip poppler download)"
    Get-ChildItem $Bin | Select-Object Name, Length
    return
}

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
    $downloaded = $false
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Invoke-WebRequest -Uri $ZipUrl -OutFile $ZipPath -UseBasicParsing -TimeoutSec 600
            $downloaded = $true
            break
        } catch {
            Write-Warning "poppler download attempt $attempt failed: $($_.Exception.Message)"
            if ($attempt -lt 3) { Start-Sleep -Seconds 5 }
        }
    }
    if (-not $downloaded) {
        throw "Failed to download poppler-windows after 3 attempts: $ZipUrl"
    }
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
$SkipDlls = @("poppler-glib.dll", "poppler-cpp.dll")
Get-ChildItem $PopplerBin -Filter "*.dll" | ForEach-Object {
    if ($SkipDlls -contains $_.Name) { return }
    Copy-Item $_.FullName (Join-Path $Lib $_.Name) -Force
    Copy-Item $_.FullName (Join-Path $Bin $_.Name) -Force
}

Invoke-StageVcRuntime -BinDir $Bin -RepoRoot $Root
Test-WindowsDocToolsBin -BinDir $Bin

Write-Host "==> staged doc-tools at $Stage"
Get-ChildItem $Bin | Select-Object Name, Length
