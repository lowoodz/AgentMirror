# Shared helpers for Windows package / install / test scripts.
$ErrorActionPreference = "Stop"

function Get-SmrRoot {
    param([string]$StartDir = $PSScriptRoot)
    $dir = $StartDir
    while ($dir) {
        if (Test-Path (Join-Path $dir "Cargo.toml")) { return $dir }
        $parent = Split-Path $dir -Parent
        if (-not $parent -or $parent -eq $dir) { break }
        $dir = $parent
    }
    throw "Could not find repo root (Cargo.toml) from $StartDir"
}

function Get-SmrVersion {
    param([string]$Root)
    $line = Select-String -Path (Join-Path $Root "Cargo.toml") -Pattern '^\s*version\s*=' |
        Select-Object -First 1
    if (-not $line) { throw "version not found in Cargo.toml" }
    if ($line.Line -match '"(.*)"') { return $matches[1] }
    throw "Could not parse version from: $($line.Line)"
}

function Set-SmrBuildEnv {
    param([string]$Root)
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
    $env:CARGO_TARGET_DIR = Join-Path $Root "target"
    $env:PYTHONUTF8 = "1"
    if ($env:CI -or $env:GITHUB_ACTIONS) {
        $env:CI = "true"
    }
}

function Stop-SmrProcesses {
    param([int]$GraceSec = 2)
    foreach ($name in @("smr", "SafeRoute", "smr-gui")) {
        Get-Process $name -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    if ($GraceSec -gt 0) { Start-Sleep -Seconds $GraceSec }
}

function Find-SmrAppExe {
    param([string]$ReleaseDir)
    foreach ($name in @("SafeRoute.exe", "smr-gui.exe")) {
        $hit = Get-ChildItem $ReleaseDir -Filter $name -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($hit) { return $hit }
    }
    return $null
}

function Get-SmrDistArtifacts {
    param([string]$Root)
    $version = Get-SmrVersion -Root $Root
    $dist = Join-Path $Root "dist"
    $winDesktop = Join-Path $dist "windows-desktop"
    @{
        Version = $version
        DistDir = $dist
        WinDesktopDir = $winDesktop
        WinDesktopExe = Join-Path $winDesktop "SafeRoute.exe"
        CliZip = Join-Path $dist "smr-$version-windows-x86_64.zip"
        AppZip = Join-Path $dist "smr-$version-windows-x86_64-app.zip"
        SetupExe = Join-Path $dist "SafeRoute_${version}_x64-setup.exe"
        Manifest = Join-Path $dist "LATEST-INSTALLERS.txt"
    }
}
