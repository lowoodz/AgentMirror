# Locate makensis.exe for Tauri NSIS bundling and prepend to PATH.
param(
    [string]$BuildRoot = "",
    [switch]$InstallIfMissing
)

function Write-NsisLog($msg) {
    if ($PSBoundParameters.ContainsKey('LogCallback')) {
        & $LogCallback $msg
    } else {
        Write-Host $msg
    }
}

function Get-NsisTreeFromRoot([string]$Root) {
    if (-not $Root -or -not (Test-Path $Root)) { return $null }
    $direct = Join-Path $Root "NSIS"
    if (Test-Path (Join-Path $direct "makensis.exe")) {
        return $direct
    }
    $makensis = Get-ChildItem $Root -Recurse -Filter "makensis.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($makensis) {
        if ($makensis.Directory.Name -ieq "Bin") {
            return $makensis.Directory.Parent.FullName
        }
        return $makensis.Directory.FullName
    }
    return $null
}

function Copy-NsisTree([string]$SourceRoot, [string]$DestRoot) {
    $src = Get-NsisTreeFromRoot $SourceRoot
    if (-not $src) { return $false }
    New-Item -ItemType Directory -Force -Path $DestRoot | Out-Null
    $destNsis = Join-Path $DestRoot "NSIS"
    if (Test-Path $destNsis) { Remove-Item $destNsis -Recurse -Force -ErrorAction SilentlyContinue }
    Copy-Item -LiteralPath $src -Destination $destNsis -Recurse -Force
    Write-NsisLog "Seeded NSIS cache: $src -> $destNsis"
    return $true
}

function Seed-TauriNsisCaches {
    $systemTauri = "C:\WINDOWS\system32\config\systemprofile\AppData\Local\tauri"
    $userTauri = "$env:LOCALAPPDATA\tauri"
    $sources = @($userTauri, $systemTauri)

    if (-not (Get-NsisTreeFromRoot $userTauri)) {
        foreach ($src in $sources) {
            if ($src -eq $userTauri) { continue }
            if (Copy-NsisTree $src $userTauri) { break }
        }
    }

    if ($BuildRoot) {
        $localTools = Join-Path $BuildRoot "target\.tauri"
        if (-not (Get-NsisTreeFromRoot $localTools)) {
            foreach ($src in @($userTauri, $systemTauri)) {
                if (Copy-NsisTree $src $localTools) { break }
            }
        }
    }
}

Seed-TauriNsisCaches

$searchRoots = @()
if ($BuildRoot) {
    $searchRoots += Join-Path $BuildRoot "target\.tauri"
}
$searchRoots += @(
    "$env:LOCALAPPDATA\tauri",
    "C:\WINDOWS\system32\config\systemprofile\AppData\Local\tauri"
)

foreach ($root in $searchRoots) {
    if (-not (Test-Path $root)) { continue }
    $makensis = Get-ChildItem $root -Recurse -Filter "makensis.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($makensis) {
        $binDir = $makensis.Directory.FullName
        $nsisRoot = if ($makensis.Directory.Name -ieq "Bin") { $makensis.Directory.Parent.FullName } else { $makensis.Directory.FullName }
        $env:PATH = "$binDir;$env:PATH"
        $env:NSISDIR = $nsisRoot
        Write-NsisLog "NSIS makensis: $($makensis.FullName)"
        return $true
    }
}

foreach ($sys in @(
    "${env:ProgramFiles(x86)}\NSIS\Bin\makensis.exe",
    "$env:ProgramFiles\NSIS\Bin\makensis.exe"
)) {
    if (Test-Path $sys) {
        $binDir = Split-Path $sys
        $env:PATH = "$binDir;$env:PATH"
        $env:NSISDIR = Split-Path $binDir
        Write-NsisLog "NSIS makensis: $sys"
        return $true
    }
}

if ($InstallIfMissing -and (Get-Command winget -ErrorAction SilentlyContinue)) {
    Write-NsisLog "Installing NSIS via winget..."
    winget install -e --id NSIS.NSIS --accept-package-agreements --accept-source-agreements --silent 2>&1 | ForEach-Object { Write-NsisLog $_ }
    foreach ($sys in @(
        "${env:ProgramFiles(x86)}\NSIS\Bin\makensis.exe",
        "$env:ProgramFiles\NSIS\Bin\makensis.exe"
    )) {
        if (Test-Path $sys) {
            $binDir = Split-Path $sys
            $env:PATH = "$binDir;$env:PATH"
            $env:NSISDIR = Split-Path $binDir
            Write-NsisLog "NSIS makensis: $sys"
            return $true
        }
    }
}

Write-NsisLog "WARNING: makensis.exe not found; NSIS bundle may fail"
return $false
