# Build Windows x86_64 release package (run on Windows).
param(
    [switch]$CliOnly
)

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $Root

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CARGO_TARGET_DIR = Join-Path $Root "target"

Write-Host "==> Building SafeRoute (release, full workspace)"
cargo build --release -p smr-cli
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Version = (Select-String -Path (Join-Path $Root "Cargo.toml") -Pattern '^version' | Select-Object -First 1).Line -replace '.*"(.*)".*', '$1'
$StableSetupName = "SafeRoute_${Version}_x64-setup.exe"
$NsisOk = $false

if (-not $CliOnly -and (Test-Path (Join-Path $Root "gui\package.json"))) {
    $uiSrc = Join-Path $Root "crates\smr-core\assets\index.html"
    $uiDst = Join-Path $Root "gui\dist\index.html"
    New-Item -ItemType Directory -Force -Path (Split-Path $uiDst) | Out-Null
    Copy-Item $uiSrc $uiDst -Force
    Write-Host "==> Sync admin UI: $uiSrc -> $uiDst"
    Write-Host "==> Building desktop app (Tauri, NSIS bundle)"
    if (Get-Command npm -ErrorAction SilentlyContinue) {
        & (Join-Path $Root "scripts\windows\prepare-nsis-bundle.ps1") -Root $Root
        Push-Location (Join-Path $Root "gui")
        npm ci --silent 2>$null; if ($LASTEXITCODE -ne 0) { npm install --silent }
        & (Join-Path $Root "scripts\windows\ensure-nsis-tools.ps1") -BuildRoot $Root -InstallIfMissing
        npm run tauri -- build --bundles nsis --ci
        if ($LASTEXITCODE -eq 0) {
            $NsisOk = $true
        } else {
            Write-Warning "Tauri NSIS build failed; retrying after NSIS PATH refresh..."
            & (Join-Path $Root "scripts\windows\ensure-nsis-tools.ps1") -BuildRoot $Root -InstallIfMissing
            npm run tauri -- build --bundles nsis --ci
            if ($LASTEXITCODE -eq 0) { $NsisOk = $true }
        }
        Pop-Location
        if (-not $NsisOk) {
            Write-Error "Tauri NSIS build failed. Install NSIS or rerun after `winget install NSIS.NSIS`."
        }
    } else {
        Write-Warning "npm not found; skipping desktop app build."
    }
} elseif ($CliOnly) {
    Write-Host "==> Skipping desktop app (-CliOnly)"
}

$Bin = Join-Path $Root "target\release\smr.exe"
$Out = Join-Path $Root "dist"
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$Pkg = "smr-$Version-windows-x86_64"

Copy-Item $Bin (Join-Path $Out "smr.exe") -Force
Copy-Item (Join-Path $Root "config\smr.example.yaml") (Join-Path $Out "smr.example.yaml") -Force
Copy-Item (Join-Path $Root "README.md") (Join-Path $Out "README.md") -Force
Copy-Item (Join-Path $Root "scripts\install.ps1") (Join-Path $Out "install.ps1") -Force
Copy-Item (Join-Path $Root "scripts\uninstall.ps1") (Join-Path $Out "uninstall.ps1") -Force
Copy-Item (Join-Path $Root "scripts\verify.ps1") (Join-Path $Out "verify.ps1") -Force

$ZipItems = @(
    (Join-Path $Out "smr.exe"),
    (Join-Path $Out "smr.example.yaml"),
    (Join-Path $Out "README.md"),
    (Join-Path $Out "install.ps1"),
    (Join-Path $Out "uninstall.ps1"),
    (Join-Path $Out "verify.ps1")
)

$GuiSetup = Get-ChildItem (Join-Path $Root "target\release\bundle\nsis\*-setup.exe") -ErrorAction SilentlyContinue | Select-Object -First 1
if ($GuiSetup) {
    Copy-Item $GuiSetup.FullName (Join-Path $Out $StableSetupName) -Force
    $ZipItems += (Join-Path $Out $StableSetupName)
    Write-Host "==> Desktop installer: $StableSetupName"
} elseif ($NsisOk) {
    Write-Error "NSIS build reported success but *-setup.exe not found under target\release\bundle\nsis"
}

$AppExe = $null
if (-not $CliOnly) {
. (Join-Path $Root "scripts\windows\common.ps1")
$AppHit = Find-SmrAppExe -ReleaseDir (Join-Path $Root "target\release")
if ($AppHit) { $AppExe = $AppHit }
if ($AppExe) {
    $PortableName = "SafeRoute.exe"
    Copy-Item $AppExe.FullName (Join-Path $Out $PortableName) -Force
    $winDesktop = Join-Path $Out "windows-desktop"
    New-Item -ItemType Directory -Force -Path $winDesktop | Out-Null
    Copy-Item $AppExe.FullName (Join-Path $winDesktop $PortableName) -Force
    $AppZip = Join-Path $Out "smr-$Version-windows-x86_64-app.zip"
    if (Test-Path $AppZip) { Remove-Item $AppZip -Force }
    $AppZipItems = @((Join-Path $Out $PortableName))
    if ($GuiSetup) { $AppZipItems += (Join-Path $Out $StableSetupName) }
    Compress-Archive -Path $AppZipItems -DestinationPath $AppZip -Force
    Write-Host "==> Desktop app package: $AppZip"
}
}

$Zip = Join-Path $Out "$Pkg.zip"
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path $ZipItems -DestinationPath $Zip -Force

Write-Host "==> Package: $Zip"
Write-Host "==> Binary:  $(Join-Path $Out 'smr.exe')"
Get-Item $Zip, (Join-Path $Out "smr.exe") | Format-Table Name, Length -AutoSize
