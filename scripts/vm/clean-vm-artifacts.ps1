# Remove SafeRoute build/test staging under windows-user ~\smr-staging (+ legacy C:\Users\Public\smr-*).
param(
    [switch]$KeepInstalled,
    [switch]$KeepPythonEmbed
)

$ErrorActionPreference = "Continue"
$Staging = if ($env:SMR_GUEST_STAGING) { $env:SMR_GUEST_STAGING } else { Join-Path $env:USERPROFILE "smr-staging" }
$Public = "C:\Users\Public"
$LogPath = Join-Path $Staging "smr-vm-clean.log"
New-Item -ItemType Directory -Force -Path $Staging | Out-Null

function Log([string]$Msg) {
    $line = "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $Msg"
    Write-Host $line
    Add-Content -Path $LogPath -Value $line -Encoding utf8
}

Log "==> clean-vm-artifacts user=$env:USERNAME staging=$Staging"

foreach ($name in @("smr", "SafeRoute", "smr-gui")) {
    Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object {
        Log "Stopping process $($_.Name) pid=$($_.Id)"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
}

if (Test-Path $Staging) {
    if ($KeepPythonEmbed) {
        Get-ChildItem -Path $Staging -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -ne "python312" } |
            ForEach-Object {
                Log "Removing $($_.FullName)"
                Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
            }
    } else {
        Log "Removing staging dir $Staging"
        Remove-Item -LiteralPath $Staging -Recurse -Force -ErrorAction SilentlyContinue
        New-Item -ItemType Directory -Force -Path $Staging | Out-Null
    }
}

# Legacy Public smr-* (pre-windows-user-staging layout)
foreach ($pat in @("smr-*", "build-windows-*.ps1", "fix-windows-ssh.ps1", "repair-vm-ssh.ps1", "setup-ssh-key.ps1")) {
    Get-ChildItem -Path $Public -Filter $pat -ErrorAction SilentlyContinue | ForEach-Object {
        Log "Removing legacy Public\$($_.Name)"
        Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if (-not $KeepInstalled) {
    $localRoots = @(
        "$env:LOCALAPPDATA\SafeRoute",
        "$env:LOCALAPPDATA\LLM-SafeRoute"
    )
    foreach ($root in $localRoots) {
        if (Test-Path $root) {
            Log "Removing installed copy $root"
            Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

Log "==> done (staging=$Staging)"
