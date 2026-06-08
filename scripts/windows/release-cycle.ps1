# End-to-end Windows release cycle (aligned with macOS scripts/macos/release-cycle.sh).
param(
    [ValidateSet("all", "preflight", "clean", "compile", "package", "verify", "test", "install", "installed", "install-smoke", "full-tests")]
    [string]$Phase = "all",
    [switch]$SkipClean,
    [switch]$SkipTests,
    [switch]$SkipInstalled,
    [switch]$KeepConfigOnClean,
    [switch]$WithApp,
    [switch]$WithoutApp,
    [switch]$WithSetup,
    [switch]$WithoutSetup,
    [switch]$CliOnly,
    [string]$InstallPrefix = "",
    [string]$LogPath = ""
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$Root = Get-SmrRoot -StartDir $PSScriptRoot
Set-SmrBuildEnv -Root $Root

# Legacy phase aliases
if ($Phase -eq "install-smoke") { $Phase = "install" }
if ($Phase -eq "full-tests") { $Phase = "test" }

# Artifact defaults: full cycle includes app + NSIS setup (match macOS release-cycle defaults)
$IncludeApp = $true
$IncludeSetup = $true
if ($CliOnly) {
    $IncludeApp = $false
    $IncludeSetup = $false
}
if ($WithApp) { $IncludeApp = $true }
if ($WithoutApp) { $IncludeApp = $false }
if ($WithSetup) { $IncludeSetup = $true }
if ($WithoutSetup) { $IncludeSetup = $false }

if (-not $LogPath) {
    $LogPath = Join-Path $Root "dist\windows-release-cycle.log"
}
New-Item -ItemType Directory -Force -Path (Split-Path $LogPath -Parent) | Out-Null

function Log($msg) {
    $line = "[$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

function Run-Step {
    param(
        [string]$Name,
        [scriptblock]$Action
    )
    Log "==> $Name"
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed (exit $LASTEXITCODE). See $LogPath"
    }
}

function Invoke-InstallSmoke {
    param([switch]$CliOnlyInstall)

    $art = Get-SmrDistArtifacts -Root $Root
    if (-not (Test-Path $art.CliZip)) {
        throw "Missing $($art.CliZip) — run -Phase package first"
    }
    $prefix = if ($InstallPrefix) { $InstallPrefix } else { Join-Path $env:TEMP "smr-release-cycle-$($art.Version)" }
    $smokeLog = Join-Path $Root "dist\windows-install-smoke.log"
    Run-Step "Install smoke (CLI zip$(if (-not $CliOnlyInstall) { ' + GUI' }))" {
        Stop-SmrProcesses
        $uninstall = Join-Path $Root "scripts\uninstall.ps1"
        if (Test-Path $uninstall) {
            $args = @("-ExecutionPolicy", "Bypass", "-File", $uninstall, "-Quiet")
            if ($KeepConfigOnClean) { $args += "-KeepConfig" }
            & powershell.exe -NoProfile @args
        }
        $smokeArgs = @(
            "-ExecutionPolicy", "Bypass", "-File",
            (Join-Path $Root "scripts\vm\windows-install-smoke.ps1"),
            "-ZipPath", $art.CliZip,
            "-LogPath", $smokeLog,
            "-Prefix", $prefix
        )
        if ($CliOnlyInstall) {
            $smokeArgs += "-CliOnly"
        } else {
            $appExe = Join-Path $art.DistDir "SafeRoute.exe"
            if (-not (Test-Path $appExe)) {
                $found = Find-SmrAppExe -ReleaseDir (Join-Path $Root "target\release")
                if ($found) { Copy-Item $found.FullName $appExe -Force }
            }
            if (-not (Test-Path $appExe)) {
                throw "Missing GUI exe for install smoke (use -CliOnly or -WithoutApp)"
            }
            $smokeArgs += @("-GuiExe", $appExe)
        }
        & powershell.exe -NoProfile @smokeArgs
    }
    Log "Install smoke log: $smokeLog"
}

$phases = @{
    preflight = { Run-Step "Preflight" { & "$PSScriptRoot\preflight.ps1" -RequireNode -RequireNsis -RequirePython } }
    clean = {
        if ($SkipClean) {
            Log "Skipping clean (-SkipClean)"
            return
        }
        Run-Step "Stop processes" { Stop-SmrProcesses }
        Run-Step "Uninstall previous install" {
            $uninstall = Join-Path $Root "scripts\uninstall.ps1"
            if (Test-Path $uninstall) {
                $args = @("-ExecutionPolicy", "Bypass", "-File", $uninstall, "-Quiet")
                if ($KeepConfigOnClean) { $args += "-KeepConfig" }
                & powershell.exe -NoProfile @args
            }
        }
    }
    compile = {
        Run-Step "Sync admin UI" {
            $uiSrc = Join-Path $Root "crates\smr-core\assets\index.html"
            $uiDst = Join-Path $Root "gui\dist\index.html"
            New-Item -ItemType Directory -Force -Path (Split-Path $uiDst) | Out-Null
            Copy-Item $uiSrc $uiDst -Force
        }
        Run-Step "Unit + smoke (verify.ps1)" { & (Join-Path $Root "scripts\verify.ps1") }
    }
    package = {
        Run-Step "Build packages (package.ps1)" {
            $env:CI = "true"
            if (-not $IncludeApp) {
                & (Join-Path $Root "scripts\package.ps1") -CliOnly
            } else {
                & (Join-Path $Root "scripts\package.ps1")
            }
        }
    }
    verify = {
        $vArgs = @()
        if ($IncludeSetup) { $vArgs += "-RequireSetup" }
        if ($IncludeApp) { $vArgs += "-RequireAppZip" }
        Run-Step "Verify dist artifacts (app=$IncludeApp, setup=$IncludeSetup)" {
            & "$PSScriptRoot\verify-package.ps1" @vArgs
        }
    }
    test = {
        if ($SkipTests) {
            Log "Skipping live tests (-SkipTests)"
            return
        }
        Run-Step "Full test suite (run_all_tests.ps1)" {
            & (Join-Path $Root "scripts\run_all_tests.ps1")
        }
    }
    install = { Invoke-InstallSmoke -CliOnlyInstall:(-not $IncludeApp) }
    installed = {
        if ($SkipInstalled) {
            Log "Skipping installed tests (-SkipInstalled)"
            return
        }
        if (-not $IncludeApp) {
            Log "SKIP installed-app tests: -WithoutApp / -CliOnly (no tray GUI package)"
            return
        }
        Invoke-InstallSmoke -CliOnlyInstall:(-not $IncludeApp)
        Log "Note: full tray blackbox on Windows guest — run from macOS: scripts/run_installed_app_tests.sh + UTM"
    }
}

Log "Windows release cycle (phase=$Phase, app=$IncludeApp, setup=$IncludeSetup) root=$Root"
Log "Log file: $LogPath"

try {
    if ($Phase -eq "all") {
        foreach ($key in @("preflight", "clean", "compile", "package", "verify", "test", "install", "installed")) {
            & $phases[$key]
        }
    } elseif ($phases.ContainsKey($Phase)) {
        & $phases[$Phase]
    } else {
        throw "Unknown phase: $Phase"
    }
    Log "RELEASE CYCLE PASSED"
    exit 0
} catch {
    Log "ERROR: $($_.Exception.Message)"
    Write-Error $_.Exception.Message
}
