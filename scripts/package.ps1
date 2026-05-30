# Build Windows x86_64 release package (run on Windows).
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $Root

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CARGO_TARGET_DIR = Join-Path $Root "target"

Write-Host "==> Building SecureModelRoute (release, full workspace)"
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if (Test-Path (Join-Path $Root "gui\package.json")) {
    Write-Host "==> Building desktop app (Tauri)"
    if (Get-Command npm -ErrorAction SilentlyContinue) {
        Push-Location (Join-Path $Root "gui")
        npm ci --silent 2>$null; if ($LASTEXITCODE -ne 0) { npm install --silent }
        npm run build --silent
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "Tauri build failed; CLI package will still be produced."
        }
        Pop-Location
    } else {
        Write-Warning "npm not found; skipping desktop app build."
    }
}

$Bin = Join-Path $Root "target\release\smr.exe"
$Out = Join-Path $Root "dist"
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$Version = (Select-String -Path (Join-Path $Root "Cargo.toml") -Pattern '^version' | Select-Object -First 1).Line -replace '.*"(.*)".*', '$1'
$Pkg = "smr-$Version-windows-x86_64"

Copy-Item $Bin (Join-Path $Out "smr.exe") -Force
Copy-Item (Join-Path $Root "config\smr.example.yaml") (Join-Path $Out "smr.example.yaml") -Force
Copy-Item (Join-Path $Root "README.md") (Join-Path $Out "README.md") -Force
Copy-Item (Join-Path $Root "scripts\install.ps1") (Join-Path $Out "install.ps1") -Force
Copy-Item (Join-Path $Root "scripts\verify.ps1") (Join-Path $Out "verify.ps1") -Force

$ZipItems = @(
    (Join-Path $Out "smr.exe"),
    (Join-Path $Out "smr.example.yaml"),
    (Join-Path $Out "README.md"),
    (Join-Path $Out "install.ps1"),
    (Join-Path $Out "verify.ps1")
)

$GuiSetup = Get-ChildItem (Join-Path $Root "target\release\bundle\nsis\*-setup.exe") -ErrorAction SilentlyContinue | Select-Object -First 1
if ($GuiSetup) {
    $GuiName = "SecureModelRoute-$Version-x64-setup.exe"
    Copy-Item $GuiSetup.FullName (Join-Path $Out $GuiName) -Force
    $ZipItems += (Join-Path $Out $GuiName)
    Write-Host "==> Desktop installer: $GuiName"
}

$Zip = Join-Path $Out "$Pkg.zip"
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path $ZipItems -DestinationPath $Zip -Force

Write-Host "==> Package: $Zip"
Write-Host "==> Binary:  $(Join-Path $Out 'smr.exe')"
Get-Item $Zip, (Join-Path $Out "smr.exe") | Format-Table Name, Length -AutoSize
