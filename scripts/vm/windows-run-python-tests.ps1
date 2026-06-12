# Run blackbox + stress Python tests on Windows VM (after smr is installed).
$ErrorActionPreference = "Continue"
$GuestStaging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
$LogPath = Join-Path $GuestStaging "smr-python-test.log"
$TestRoot = Join-Path $GuestStaging "smr-test-suite"
$SmrBin = Join-Path $GuestStaging "smr-home\bin\smr.exe"
$EmbedDir = Join-Path $GuestStaging "python312"
$EmbedZip = Join-Path $GuestStaging "python-embed.zip"
$EmbedUrl = "https://www.python.org/ftp/python/3.12.8/python-3.12.8-embed-amd64.zip"
$GetPipUrl = "https://bootstrap.pypa.io/get-pip.py"

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

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
    $fixed = Join-Path $EmbedDir "python.exe"
    if (Test-PythonExe $fixed) { return $fixed }
    foreach ($cmd in @("python", "py", "python3")) {
        $c = Get-Command $cmd -ErrorAction SilentlyContinue
        if ($c -and (Test-PythonExe $c.Source)) { return $c.Source }
    }
    $roots = @(
        $EmbedDir,
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
    Log "Downloading embedded Python from python.org..."
    if (-not (Test-Path $EmbedDir)) { New-Item -ItemType Directory -Force -Path $EmbedDir | Out-Null }
    try {
        Invoke-WebRequest -Uri $EmbedUrl -OutFile $EmbedZip -UseBasicParsing -TimeoutSec 300
    } catch {
        Log "ERROR: download failed: $($_.Exception.Message)"
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

Remove-Item $LogPath -Force -ErrorAction SilentlyContinue
Log "==> Python test suite on Windows"
Log "Account: $env:USERNAME"

if (-not (Test-Path $SmrBin)) {
    Log "ERROR: missing $SmrBin - run functional test first"
    exit 1
}

if (-not (Test-Path $TestRoot)) {
    Log "ERROR: missing test suite at $TestRoot"
    exit 1
}

$python = Find-Python
if (-not $python) {
    Log "Trying winget..."
    winget install --id Python.Python.3.12 -e --accept-source-agreements --accept-package-agreements --silent 2>&1 | ForEach-Object { Log "winget: $_" }
    $env:Path = "$EmbedDir;$EmbedDir\Scripts;$env:LOCALAPPDATA\Programs\Python\Python312;$env:LOCALAPPDATA\Programs\Python\Python312\Scripts;C:\Program Files\Python312;C:\Program Files\Python312\Scripts;$env:Path"
    $python = Find-Python
}

if (-not $python) {
    $python = Install-EmbeddedPython
}

if (-not $python) {
    Log "ERROR: Python not available"
    exit 1
}

Log "Using Python: $(& $python --version 2>&1) at $python"

Get-Process smr, SafeRoute, smr-gui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

& $python -m pip install --quiet --disable-pip-version-check openai 2>&1 | ForEach-Object { Log "pip: $_" }

$env:SMR_BIN = $SmrBin
$env:SMR_KEYS_FILE = Join-Path $TestRoot "test_model_api_key.txt"
$env:PYTHONUTF8 = "1"
$env:PYTHONUNBUFFERED = "1"
Set-Location $TestRoot

Log "==> transparency_pass_through_test.py (--release)"
& $python (Join-Path $TestRoot "scripts\transparency_pass_through_test.py") --release 2>&1 | ForEach-Object { Log $_ }
$transparency = $LASTEXITCODE

Log "==> blackbox_test.py"
Get-Process smr, SafeRoute, smr-gui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
& $python (Join-Path $TestRoot "scripts\blackbox_test.py") 2>&1 | ForEach-Object { Log $_ }
$bb = $LASTEXITCODE

Log "==> live_test.py (stress)"
Get-Process smr, SafeRoute, smr-gui -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 3
$env:SMR_STRESS_TOTAL = "20"
$env:SMR_STRESS_STREAM_TOTAL = "10"
$env:SMR_STRESS_WORKERS = "4"
$env:SMR_STRESS_STREAM_WORKERS = "2"
$env:SMR_STRESS_MIN_SUCCESS = "0.75"
& $python (Join-Path $TestRoot "scripts\live_test.py") 2>&1 | ForEach-Object { Log $_ }
$stress = $LASTEXITCODE

Log ""
if ($transparency -eq 0 -and $bb -eq 0 -and $stress -eq 0) {
    Log "Python tests PASSED"
    exit 0
}
Log "Python tests FAILED (transparency=$transparency blackbox=$bb stress=$stress)"
exit 1
