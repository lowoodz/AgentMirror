# Build SafeRoute Tauri desktop on Windows as windows-user (MSVC). Staging: ~\smr-staging
$ErrorActionPreference = "Continue"
$Staging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
New-Item -ItemType Directory -Force -Path $Staging | Out-Null
$LogPath = Join-Path $Staging "smr-desktop-build.log"
$TargetFile = Join-Path $Staging "smr-gui-target.txt"
if (-not $env:SMR_WINDOWS_GUI_TARGET -and (Test-Path $TargetFile)) {
    $env:SMR_WINDOWS_GUI_TARGET = (Get-Content $TargetFile -Raw).Trim()
}
$TargetTriple = if ($env:SMR_WINDOWS_GUI_TARGET) { $env:SMR_WINDOWS_GUI_TARGET } else { "x86_64-pc-windows-msvc" }

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

function Import-VsDevEnv {
    param([string]$Platform = "x64")

    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $false }

    $installPath = & $vswhere -latest -products * -property installationPath 2>$null
    if (-not $installPath) { return $false }

    $candidates = @()
    if ($Platform -eq "x64") {
        $candidates += @(
            (Join-Path $installPath "VC\Auxiliary\Build\vcvarsamd64_arm64.bat"),
            (Join-Path $installPath "VC\Auxiliary\Build\vcvars64.bat"),
            (Join-Path $installPath "VC\Auxiliary\Build\vcvarsamd64.bat")
        )
    } else {
        $candidates += (Join-Path $installPath "VC\Auxiliary\Build\vcvarsarm64.bat")
    }

    foreach ($vcvars in $candidates) {
        if (-not (Test-Path $vcvars)) {
            Log "  skip missing: $vcvars"
            continue
        }
        Log "Importing VS env: $vcvars"
        cmd /c "`"$vcvars`" >nul 2>&1 && set" | ForEach-Object {
            if ($_ -match '^([^=]+)=(.*)$') {
                Set-Item -Path "env:$($matches[1])" -Value $matches[2]
            }
        }
        if (Get-Command link.exe -ErrorAction SilentlyContinue) {
            Log "link.exe: $((Get-Command link.exe).Source)"
            return $true
        }
    }
    return $false
}

function Refresh-Path {
    $paths = @(
        "$env:USERPROFILE\.cargo\bin",
        (Join-Path $Staging "node"),
        "$env:ProgramFiles\nodejs",
        "$env:LOCALAPPDATA\Programs\nodejs"
    )
    foreach ($root in $paths) {
        if (Test-Path $root) { $env:Path = "$root;$env:Path" }
    }
}

function Ensure-Node {
    $NodeDir = Join-Path $Staging "node"
    $nodeExe = Join-Path $NodeDir "node.exe"
    if (Test-Path $nodeExe) {
        try {
            $machine = Get-PeMachine $nodeExe
            $expected = if ((Get-RustHostArch) -eq "ARM64") { 0xAA64 } else { 0x8664 }
            if ($machine -eq $expected) {
                Log "Using pre-staged Node.js: $nodeExe"
                return $NodeDir
            }
            Log "Removing mismatched Node PE (0x$($machine.ToString('X4')))"
        } catch {
            Log "Removing invalid staged Node: $($_.Exception.Message)"
        }
        Remove-Item $NodeDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    $ver = "22.15.0"
    $osArch = Get-RustHostArch
    if ($osArch -eq "ARM64") {
        $zipUrl = "https://nodejs.org/dist/v$ver/node-v$ver-win-arm64.zip"
        Log "Downloading Node.js v$ver portable (arm64)..."
    } else {
        $zipUrl = "https://nodejs.org/dist/v$ver/node-v$ver-win-x64.zip"
        Log "Downloading Node.js v$ver portable (x64)..."
    }
    $zipPath = "$env:TEMP\node-win.zip"
    try {
        Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing -TimeoutSec 600
    } catch {
        Log "ERROR: Node download failed: $($_.Exception.Message)"
        return $null
    }
    $extractRoot = "$env:TEMP\node-extract"
    Remove-Item $extractRoot -Recurse -Force -ErrorAction SilentlyContinue
    Expand-Archive -Path $zipPath -DestinationPath $extractRoot -Force
    $extracted = Get-ChildItem $extractRoot -Directory | Select-Object -First 1
    if (-not $extracted) {
        Log "ERROR: Node zip extract failed"
        return $null
    }
    Remove-Item $NodeDir -Recurse -Force -ErrorAction SilentlyContinue
    Move-Item $extracted.FullName $NodeDir
    return $NodeDir
}

function Get-OsArch {
    $os = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    if ($os -eq 'Arm64') { return 'ARM64' }
    if ($os -eq 'X64') { return 'AMD64' }
    return $env:PROCESSOR_ARCHITECTURE
}

function Get-RustHostArch {
    if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        return Get-OsArch
    }
    $hostTriple = (& rustc -vV 2>&1 | Select-String '^host:' | ForEach-Object { $_.Line -replace '^host:\s*', '' })
    if ($hostTriple -like 'aarch64-*') { return 'ARM64' }
    if ($hostTriple -like 'x86_64-*') { return 'AMD64' }
    return Get-OsArch
}

function Ensure-Rust {
    Refresh-Path
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Log "Installing Rust via rustup..."
        $rustup = "$env:TEMP\rustup-init.exe"
        try {
            Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustup -UseBasicParsing -TimeoutSec 300
        } catch {
            Log "ERROR: rustup download failed: $($_.Exception.Message)"
            return $false
        }
        & $rustup -y --default-toolchain stable 2>&1 | ForEach-Object { Log $_ }
        Refresh-Path
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { return $false }
    Log "Host Rust: $(rustc -vV 2>&1 | Select-String '^host:' | ForEach-Object { $_.Line })"
    Log "Adding target $TargetTriple"
    & rustup target add $TargetTriple 2>&1 | ForEach-Object { Log $_ }
    return $true
}

function Find-CrossLinkExe {
    param([string]$HostArch, [string]$TargetArch)

    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $null }
    $installPath = & $vswhere -latest -products * -property installationPath 2>$null
    if (-not $installPath) { return $null }

    $hostLabel = switch ($HostArch) {
        "ARM64" { "Hostarm64" }
        default { "Hostx64" }
    }
    $targetLabel = switch ($TargetArch) {
        "x64" { "x64" }
        "arm64" { "arm64" }
        default { "x64" }
    }
    $pattern = Join-Path $installPath "VC\Tools\MSVC\*\bin\$hostLabel\$targetLabel\link.exe"
    $link = Get-ChildItem $pattern -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | Select-Object -First 1
    if ($link) { return $link.FullName }
    return $null
}

function Ensure-Msvc {
    $hostArch = Get-RustHostArch
    Log "Rust host arch: $hostArch (PROCESSOR_ARCHITECTURE=$env:PROCESSOR_ARCHITECTURE)"

    $needInstall = $false
    if ($hostArch -eq "ARM64") {
        if (-not (Import-VsDevEnv -Platform "arm64")) { $needInstall = $true }
    } else {
        if (-not (Import-VsDevEnv -Platform "x64")) { $needInstall = $true }
    }

    if ($needInstall) {
        Log "Installing VS 2022 Build Tools (MSVC + SDK, may take 15+ min)..."
        $bt = "$env:TEMP\vs_buildtools.exe"
        try {
            Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vs_buildtools.exe" -OutFile $bt -UseBasicParsing -TimeoutSec 600
        } catch {
            Log "ERROR: vs_buildtools download failed: $($_.Exception.Message)"
            return $false
        }
        $args = @(
            "--quiet", "--wait", "--norestart", "--nocache",
            "--add", "Microsoft.VisualStudio.Workload.VCTools",
            "--add", "Microsoft.VisualStudio.Component.Windows11SDK.22621",
            "--add", "Microsoft.VisualStudio.Component.VC.Tools.ARM64",
            "--add", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "--includeRecommended"
        )
        $proc = Start-Process -FilePath $bt -ArgumentList $args -Wait -PassThru
        Log "vs_buildtools exit code: $($proc.ExitCode)"
        if ($hostArch -eq "ARM64") {
            if (-not (Import-VsDevEnv -Platform "arm64")) { return $false }
        } else {
            if (-not (Import-VsDevEnv -Platform "x64")) { return $false }
        }
    }

    if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
        Log "ERROR: host link.exe not found after vcvars"
        return $false
    }
    Log "Host link.exe: $((Get-Command link.exe).Source)"

    if ($hostArch -eq "ARM64" -and $TargetTriple -eq "x86_64-pc-windows-msvc") {
        $cross = Find-CrossLinkExe -HostArch "ARM64" -TargetArch "x64"
        if (-not $cross) {
            Log "ERROR: ARM64->x64 cross link.exe not found (install VS C++ x64 tools)"
            return $false
        }
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = $cross
        Log "Cross linker: $cross"
    }

    return $true
}

function Get-PeMachine {
    param([string]$Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    return [BitConverter]::ToUInt16($bytes, $peOffset + 4)
}

Remove-Item $LogPath -Force -ErrorAction SilentlyContinue

if ($env:SMR_WINDOWS_USER -and $env:USERNAME -ne $env:SMR_WINDOWS_USER) {
    Log "ERROR: build must run as $env:SMR_WINDOWS_USER (current: $env:USERNAME)"
    exit 1
}

Log "==> Windows desktop (Tauri) build for $TargetTriple"
Log "OS arch: $(Get-OsArch)"

# Drop cached x64 Node on ARM hosts when a stale cache exists from prior builds.
if ((Get-OsArch) -eq "ARM64" -and (Test-Path (Join-Path $Staging "node\node.exe"))) {
    $nodeExe = Join-Path $Staging "node\node.exe"
    try {
        $machine = Get-PeMachine $nodeExe
        if ($machine -ne 0xAA64) {
            Remove-Item (Join-Path $Staging "node") -Recurse -Force -ErrorAction SilentlyContinue
        }
    } catch {
        Remove-Item (Join-Path $Staging "node") -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$SrcZip = Join-Path $Staging "smr-build-src.zip"
$BuildRoot = Join-Path $Staging "smr-build"
$OutDir = Join-Path $Staging "smr-desktop-out"

if (-not (Test-Path $SrcZip)) {
    Log "ERROR: missing $SrcZip"
    exit 1
}

# Preserve Rust target/ from a prior guest build before refreshing sources.
$TargetCache = Join-Path $Staging "smr-build-target-cache"
$OldTarget = Join-Path $Staging "smr-build\target"
if (Test-Path $OldTarget) {
    if (Test-Path $TargetCache) { Remove-Item $TargetCache -Recurse -Force -ErrorAction SilentlyContinue }
    Move-Item $OldTarget $TargetCache
    Log "Saved existing smr-build/target to cache"
}

Remove-Item $BuildRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $BuildRoot | Out-Null
Expand-Archive -Path $SrcZip -DestinationPath $BuildRoot -Force

# Reuse prior Rust target cache on guest (large speedup on retries).
$TargetCache = Join-Path $Staging "smr-build-target-cache"
$BuildTarget = Join-Path $BuildRoot "target"
if (Test-Path $TargetCache) {
    Log "Restoring cached target/ from prior build"
    Move-Item $TargetCache $BuildTarget
}
Set-Location $BuildRoot

$env:CARGO_TARGET_DIR = Join-Path $BuildRoot "target"
$env:CARGO_BUILD_TARGET = $TargetTriple
$env:PYTHONUTF8 = "1"

if (-not (Ensure-Rust)) {
    Log "ERROR: cargo not available"
    exit 1
}

$rustHost = Get-RustHostArch
Log "Rust host arch: $rustHost"

$nodeDir = Ensure-Node
if (-not $nodeDir) { exit 1 }
$env:Path = "$nodeDir;$env:Path"
Refresh-Path

if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
    Log "ERROR: npm not available after Node install"
    exit 1
}
Log "Node: $(node --version 2>&1)"

if (-not (Ensure-Msvc)) {
    Log "ERROR: MSVC environment not ready (link.exe / Windows SDK)"
    exit 1
}

$GuiDir = Join-Path $BuildRoot "gui"
if (-not (Test-Path (Join-Path $GuiDir "package.json"))) {
    Log "ERROR: gui/ missing in source bundle"
    exit 1
}

$FrontendDist = Join-Path $GuiDir "dist"
if (-not (Test-Path (Join-Path $FrontendDist "index.html"))) {
    Log "Creating minimal gui/dist (redirect stub)"
    New-Item -ItemType Directory -Force -Path $FrontendDist | Out-Null
    @'
<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>SafeRoute</title></head>
<body><p>Loading SafeRoute...</p>
<script>location.replace('http://127.0.0.1:8080/ui')</script></body></html>
'@ | Set-Content -Path (Join-Path $FrontendDist "index.html") -Encoding UTF8
}

function Ensure-NsisTools {
    $helper = Join-Path $BuildRoot "scripts\windows\ensure-nsis-tools.ps1"
    if (Test-Path $helper) {
        & $helper -BuildRoot $BuildRoot -InstallIfMissing
    }
    if (Get-Command makensis.exe -ErrorAction SilentlyContinue) {
        Log "NSIS ready: $((Get-Command makensis.exe).Source)"
        return $true
    }
    Log "WARNING: makensis.exe not found; NSIS bundle may fail"
    return $false
}

Push-Location $GuiDir
Log "Building smr CLI ($TargetTriple) for NSIS bundle..."
& cargo build --release --target $TargetTriple -p smr-cli 2>&1 | ForEach-Object { Log $_ }
if ($LASTEXITCODE -ne 0) {
    Pop-Location
    Log "ERROR: smr CLI build failed"
    exit 1
}
$Prepare = Join-Path $BuildRoot "scripts\windows\prepare-nsis-bundle.ps1"
if (Test-Path $Prepare) {
    & $Prepare -Root $BuildRoot -TargetTriple $TargetTriple 2>&1 | ForEach-Object { Log $_ }
} else {
    Log "ERROR: missing $Prepare"
    Pop-Location
    exit 1
}

Log "npm ci..."
npm ci --silent 2>&1 | ForEach-Object { Log $_ }
if ($LASTEXITCODE -ne 0) { npm install --silent 2>&1 | ForEach-Object { Log $_ } }

Ensure-NsisTools | Out-Null
$NsisBundle = Join-Path $BuildRoot "target\$TargetTriple\release\bundle\nsis"
if (Test-Path $NsisBundle) {
    Remove-Item $NsisBundle -Recurse -Force -ErrorAction SilentlyContinue
    Log "Cleared stale NSIS bundle: $NsisBundle"
}
Log "tauri build --target $TargetTriple --bundles nsis (desktop app + NSIS installer) ..."
npx tauri build --target $TargetTriple --bundles nsis 2>&1 | ForEach-Object { Log $_ }
$buildOk = ($LASTEXITCODE -eq 0)

if (-not $buildOk) {
    Ensure-NsisTools | Out-Null
    Log "Retry tauri NSIS bundle after PATH fix ..."
    npx tauri build --target $TargetTriple --bundles nsis 2>&1 | ForEach-Object { Log $_ }
    $buildOk = ($LASTEXITCODE -eq 0)
}

if (-not $buildOk) {
    if (Test-Path $BuildTarget) {
        if (Test-Path $TargetCache) { Remove-Item $TargetCache -Recurse -Force -ErrorAction SilentlyContinue }
        Move-Item $BuildTarget $TargetCache -ErrorAction SilentlyContinue
    }
    Log "ERROR: tauri NSIS build failed (stale setup.exe is not reused)"
    exit 1
}

Pop-Location

$Release = Join-Path $BuildRoot "target\$TargetTriple\release"
if (-not (Test-Path $Release)) {
    $Release = Join-Path $BuildRoot "target\release"
}
$BuiltExe = Get-ChildItem $Release -Filter "SafeRoute.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $BuiltExe) {
    $BuiltExe = Get-ChildItem $Release -Filter "smr-gui.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
}

Remove-Item $OutDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$AppExe = $BuiltExe
if (-not $AppExe) {
    foreach ($name in @("SafeRoute.exe", "smr-gui.exe")) {
        $AppExe = Get-ChildItem $Release -Filter $name -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($AppExe) { break }
    }
}
if (-not $AppExe) {
    Log "ERROR: desktop exe not found under $Release"
    exit 1
}

$machine = Get-PeMachine $AppExe.FullName
$expected = switch ($TargetTriple) {
    "x86_64-pc-windows-msvc" { 0x8664 }
    "aarch64-pc-windows-msvc" { 0xAA64 }
    default { $null }
}
if ($null -ne $expected -and $machine -ne $expected) {
    Log "ERROR: expected PE machine 0x$($expected.ToString('X4')) but got 0x$($machine.ToString('X4'))"
    exit 1
}
Log "PE machine: 0x$($machine.ToString('X4'))"

Copy-Item $AppExe.FullName (Join-Path $OutDir "SafeRoute.exe") -Force
Log "App exe: $($AppExe.FullName)"

$Setup = Get-ChildItem (Join-Path $Release "bundle\nsis") -Filter "*-setup.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $Setup) {
    Log "ERROR: NSIS setup.exe not found under $(Join-Path $Release 'bundle\nsis')"
    exit 1
}
Copy-Item $Setup.FullName (Join-Path $OutDir $Setup.Name) -Force
$Version = (Select-String -Path (Join-Path $BuildRoot "Cargo.toml") -Pattern '^version' | Select-Object -First 1).Line -replace '.*"(.*)".*', '$1'
$StableSetup = Join-Path $OutDir "SafeRoute_${Version}_x64-setup.exe"
Copy-Item $Setup.FullName $StableSetup -Force
Log "NSIS setup: $($Setup.Name) (+ $StableSetup)"

Log "DESKTOP_BUILD_OK"

if (Test-Path $BuildTarget) {
    if (Test-Path $TargetCache) { Remove-Item $TargetCache -Recurse -Force -ErrorAction SilentlyContinue }
    Move-Item $BuildTarget $TargetCache -ErrorAction SilentlyContinue
    Log "Saved target/ cache for incremental rebuilds"
}
exit 0
