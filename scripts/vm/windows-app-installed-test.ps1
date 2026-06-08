# Install tray GUI from staged payload (runs as windows-user via SSH).
param(
    [string]$StageDir = "",
    [string]$Prefix = "",
    [string]$SecretsDir = "",
    [string]$ConfigPath = "",
    [string]$LogPath = "",
    [string]$TestRoot = "",
    [string]$Base = "http://127.0.0.1:8080"
)

$GuestStaging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
if (-not $StageDir) { $StageDir = Join-Path $GuestStaging "smr-app-test-stage" }
if (-not $Prefix) { $Prefix = Join-Path $GuestStaging "smr-app-test-home" }
if (-not $SecretsDir) { $SecretsDir = Join-Path $GuestStaging "smr-app-test-secrets" }
if (-not $ConfigPath) { $ConfigPath = Join-Path $GuestStaging "smr-app-test-home\smr.yaml" }
if (-not $LogPath) { $LogPath = Join-Path $GuestStaging "smr-app-installed-test.log" }
if (-not $TestRoot) { $TestRoot = Join-Path $GuestStaging "smr-test-suite" }

$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"

function Test-PythonExe {
    param([string]$Exe)
    if (-not $Exe -or -not (Test-Path -LiteralPath $Exe)) { return $false }
    if ($Exe -match '\\WindowsApps\\') { return $false }
    try {
        & $Exe -c "import sys" 2>$null | Out-Null
        return ($LASTEXITCODE -eq 0)
    } catch {
        return $false
    }
}

function Find-Python {
    $fixed = Join-Path $GuestStaging "python312\python.exe"
    if (Test-PythonExe $fixed) { return $fixed }
    foreach ($cmd in @("python", "py", "python3")) {
        $c = Get-Command $cmd -ErrorAction SilentlyContinue
        if ($c -and (Test-PythonExe $c.Source)) { return $c.Source }
    }
    $roots = @(
        (Join-Path $GuestStaging "python312"),
        "$env:LOCALAPPDATA\Programs\Python",
        "C:\Program Files\Python312",
        "C:\Program Files\Python311"
    )
    foreach ($root in $roots) {
        $exe = Join-Path $root "python.exe"
        if (Test-PythonExe $exe) { return $exe }
        if (-not (Test-Path $root)) { continue }
        $found = Get-ChildItem -Path $root -Recurse -Filter python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found -and (Test-PythonExe $found.FullName)) { return $found.FullName }
    }
    return $null
}

function Install-EmbeddedPython {
    $EmbedDir = Join-Path $GuestStaging "python312"
    $EmbedZip = Join-Path $GuestStaging "python-embed.zip"
    $EmbedUrl = "https://www.python.org/ftp/python/3.12.8/python-3.12.8-embed-amd64.zip"
    $GetPipUrl = "https://bootstrap.pypa.io/get-pip.py"
    Log "Downloading embedded Python..."
    if (-not (Test-Path $EmbedDir)) { New-Item -ItemType Directory -Force -Path $EmbedDir | Out-Null }
    try {
        Invoke-WebRequest -Uri $EmbedUrl -OutFile $EmbedZip -UseBasicParsing -TimeoutSec 300
    } catch {
        Log "ERROR: python download failed: $($_.Exception.Message)"
        return $null
    }
    Expand-Archive -Path $EmbedZip -DestinationPath $EmbedDir -Force
    $pth = Get-ChildItem -Path $EmbedDir -Filter "*._pth" | Select-Object -First 1
    if ($pth) {
        $text = Get-Content $pth.FullName -Raw
        $text = $text -replace '#import site', 'import site'
        Set-Content -Path $pth.FullName -Value $text -Encoding ASCII
    }
    $getPip = Join-Path $EmbedDir "get-pip.py"
    Invoke-WebRequest -Uri $GetPipUrl -OutFile $getPip -UseBasicParsing -TimeoutSec 120
    $py = Join-Path $EmbedDir "python.exe"
    & $py $getPip --no-warn-script-location 2>&1 | ForEach-Object { Log "get-pip: $_" }
    if (Test-Path $py) { return $py }
    return $null
}

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

function Stop-ListeningServer {
    for ($attempt = 0; $attempt -lt 12; $attempt++) {
        foreach ($name in @("smr", "smr-gui", "SafeRoute")) {
            Get-Process $name -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        }
        cmd.exe /c "taskkill /F /IM smr.exe /T 2>nul & taskkill /F /IM SafeRoute.exe /T 2>nul" | Out-Null
        Start-Sleep -Seconds 2
        $alive = $false
        try {
            $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 2
            if ("$h" -match "OK") { $alive = $true }
        } catch {}
        if (-not $alive) {
            Log "Port $Base free after $($attempt + 1) stop attempt(s)"
            return $true
        }
        Log "WARN: server still on $Base, retry stop"
    }
    Log "ERROR: could not free $Base — stale smr/SafeRoute still listening"
    return $false
}

Remove-Item $LogPath -Force -ErrorAction SilentlyContinue
Log "==> Windows installed-app black-box test"

if (-not (Stop-ListeningServer)) { exit 1 }

Remove-Item $Prefix -Recurse -Force -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Force -Path $StageDir, $Prefix, $SecretsDir, (Split-Path $ConfigPath) | Out-Null
Set-Content -Path (Join-Path $SecretsDir "project.txt") -Value "probe-secret-data" -Encoding UTF8

$hasCli = $false
$hasGui = $false
$guiName = $null
$guiName = "SafeRoute.exe"
if (Test-Path (Join-Path $StageDir "smr.exe")) { $hasCli = $true }
if (Test-Path (Join-Path $StageDir $guiName)) { $hasGui = $true }
if (-not $hasCli) {
    Log "ERROR: missing staged smr.exe under $StageDir"
    exit 1
}
if (-not $hasGui) {
    Log "ERROR: missing staged SafeRoute.exe under $StageDir"
    exit 1
}

$BinDir = Join-Path $Prefix "bin"
$GuiDir = Join-Path $Prefix "Programs\SafeRoute"
New-Item -ItemType Directory -Force -Path $BinDir, $GuiDir | Out-Null

function Copy-VerifiedFile {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    if (-not (Test-Path $Source)) {
        Log "ERROR: missing source file $Source"
        exit 1
    }
    if (Test-Path $Destination) {
        Remove-Item $Destination -Force -ErrorAction Stop
    }
    $staging = "$Destination.new"
    if (Test-Path $staging) { Remove-Item $staging -Force -ErrorAction Stop }
    Copy-Item $Source $staging -Force -ErrorAction Stop
    Move-Item $staging $Destination -Force -ErrorAction Stop
    $srcLen = (Get-Item $Source).Length
    $dstLen = (Get-Item $Destination).Length
    if ($srcLen -ne $dstLen) {
        Log "ERROR: copy size mismatch for $Destination (src=$srcLen dst=$dstLen)"
        exit 1
    }
    Log "Copied $(Split-Path $Destination -Leaf) src=$srcLen dst=$dstLen"
}

Copy-VerifiedFile (Join-Path $StageDir "smr.exe") (Join-Path $BinDir "smr.exe")
Copy-VerifiedFile (Join-Path $StageDir $guiName) (Join-Path $GuiDir "SafeRoute.exe")
Log "Installed CLI -> $BinDir"
Log "Installed GUI  -> $GuiDir"

$AppExe = Join-Path $GuiDir "SafeRoute.exe"
if (-not (Test-Path $AppExe)) {
    Log "ERROR: desktop app missing at $AppExe"
    exit 1
}

$python = Find-Python
if (-not $python) {
    $python = Install-EmbeddedPython
}
if (-not $python) {
    Log "ERROR: python not found on guest (bootstrap failed)"
    exit 1
}
Log "Using Python: $python"

Log "==> Write test config -> $ConfigPath"
& $python (Join-Path $TestRoot "scripts\generate_test_config.py") $ConfigPath $SecretsDir 2>&1 | ForEach-Object { Log $_ }
if (-not (Test-Path $ConfigPath)) {
    Log "ERROR: config not created"
    exit 1
}

Log "==> Launch tray GUI (SMR_CONFIG, --background)"
$env:SMR_FORCE_SERVER = "1"
$env:SMR_CONFIG = $ConfigPath
$gui = Start-Process -FilePath $AppExe -ArgumentList @("--background") -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 10

function Wait-Ready {
    for ($i = 0; $i -lt 90; $i++) {
        try {
            $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 3
            if ("$h" -match "OK") {
                $st = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 3
                if ($st.file_index_ready) { return $true }
            }
        } catch {}
        if ($gui.HasExited) {
            Log "ERROR: GUI exited during startup code=$($gui.ExitCode)"
            return $false
        }
        Start-Sleep -Seconds 1
    }
    return $false
}

if (-not (Wait-Ready)) {
    Log "ERROR: installed GUI server not ready"
    if (-not $gui.HasExited) { Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue }
    exit 1
}
Log "Server ready pid=$($gui.Id)"

$stagedGuiPath = Join-Path $StageDir $guiName
$stagedGuiLen = (Get-Item $stagedGuiPath).Length
$installedGuiLen = (Get-Item $AppExe).Length
if ($stagedGuiLen -ne $installedGuiLen) {
    Log "ERROR: installed SafeRoute.exe size ($installedGuiLen) != staged ($stagedGuiLen)"
    exit 1
}
try {
    $procPath = (Get-Process -Id $gui.Id -ErrorAction Stop).Path
    if ((Resolve-Path $procPath).Path -ne (Resolve-Path $AppExe).Path) {
        Log "ERROR: GUI process path mismatch expected=$AppExe got=$procPath"
        exit 1
    }
} catch {
    Log "WARN: could not verify GUI process path: $($_.Exception.Message)"
}

try {
    $ui = Invoke-WebRequest -Uri "$Base/ui" -TimeoutSec 15 -UseBasicParsing
    if ($ui.Content -notmatch "SafeRoute") {
        Log "ERROR: admin UI missing marker"
        exit 1
    }
    Log "Admin UI OK bytes=$($ui.Content.Length)"
} catch {
    Log "ERROR: admin UI: $($_.Exception.Message)"
    exit 1
}

Log "==> Tray smoke: GUI process alive while API responds"
if ($gui.HasExited) {
    Log "ERROR: GUI exited early"
    exit 1
}
$health2 = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 5
Log "Health in background mode: $health2"

Log "==> blackbox_test.py (attach mode, 27 scenarios)"
$env:SMR_ATTACH = "1"
$env:SMR_BASE = $Base
$env:SMR_KEYS_FILE = Join-Path $TestRoot "test_model_api_key.txt"
$env:PYTHONUTF8 = "1"
Set-Location $TestRoot
& $python (Join-Path $TestRoot "scripts\blackbox_test.py") 2>&1 | ForEach-Object { Log $_ }
$bb = $LASTEXITCODE

if (-not $gui.HasExited) {
    Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
}
Get-Process smr -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

if ($bb -ne 0) {
    Log "INSTALLED-APP TEST FAILED (blackbox exit=$bb)"
    exit 1
}

Log "INSTALLED-APP TEST PASSED"
exit 0
