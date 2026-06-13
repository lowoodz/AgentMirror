# Idempotent: ensure embedded Python 3.12 exists under SMR_GUEST_STAGING/python312.
$ErrorActionPreference = "Stop"
$GuestStaging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
$EmbedDir = Join-Path $GuestStaging "python312"
$EmbedZip = Join-Path $GuestStaging "python-embed.zip"
$EmbedUrl = "https://www.python.org/ftp/python/3.12.8/python-3.12.8-embed-amd64.zip"
$GetPipUrl = "https://bootstrap.pypa.io/get-pip.py"
$LogPath = Join-Path $GuestStaging "bootstrap-python.log"

function Log([string]$Msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $Msg"
    Write-Host $line
    New-Item -ItemType Directory -Force -Path $GuestStaging | Out-Null
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
}

function Test-PythonExe([string]$Exe) {
    if (-not $Exe -or -not (Test-Path -LiteralPath $Exe)) { return $false }
    if ($Exe -match '\\WindowsApps\\') { return $false }
    & $Exe -c "import sys" 2>$null | Out-Null
    return ($LASTEXITCODE -eq 0)
}

$py = Join-Path $EmbedDir "python.exe"
if (Test-PythonExe $py) {
    Log "python312 already present at $py"
    exit 0
}

Log "Installing embedded Python to $EmbedDir"
if (-not (Test-Path $EmbedDir)) { New-Item -ItemType Directory -Force -Path $EmbedDir | Out-Null }

if (-not (Test-Path $EmbedZip)) {
    Log "Downloading $EmbedUrl (timeout 600s)..."
    Invoke-WebRequest -Uri $EmbedUrl -OutFile $EmbedZip -UseBasicParsing -TimeoutSec 600
} else {
    Log "Reusing cached zip $EmbedZip"
}

Expand-Archive -Path $EmbedZip -DestinationPath $EmbedDir -Force
$pth = Get-ChildItem -Path $EmbedDir -Filter "*._pth" | Select-Object -First 1
if ($pth) {
    $text = Get-Content $pth.FullName -Raw
    $text = $text -replace '#import site', 'import site'
    Set-Content -Path $pth.FullName -Value $text -Encoding ASCII
}

$getPip = Join-Path $EmbedDir "get-pip.py"
if (-not (Test-Path $getPip)) {
    Log "Downloading get-pip.py..."
    Invoke-WebRequest -Uri $GetPipUrl -OutFile $getPip -UseBasicParsing -TimeoutSec 180
}
& $py $getPip --no-warn-script-location 2>&1 | ForEach-Object { Log "get-pip: $_" }

if (-not (Test-PythonExe $py)) {
    Log "ERROR: bootstrap failed — python.exe not usable"
    exit 1
}
Log "Bootstrap OK: $py"
