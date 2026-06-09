# Stop SafeRoute listeners and free :8080 before the next VM test phase.
$ErrorActionPreference = "Continue"

foreach ($name in @("smr", "SafeRoute", "smr-gui")) {
    Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "Stopping $($_.Name) pid=$($_.Id)"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
}

Start-Sleep -Seconds 2

$conn = Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue
foreach ($c in $conn) {
    if ($c.OwningProcess) {
        Write-Host "Killing pid $($c.OwningProcess) still listening on :8080"
        Stop-Process -Id $c.OwningProcess -Force -ErrorAction SilentlyContinue
    }
}

Start-Sleep -Seconds 1
if (Get-NetTCPConnection -LocalPort 8080 -State Listen -ErrorAction SilentlyContinue) {
    Write-Host "WARNING: :8080 still in use"
    exit 1
}
Write-Host "Port 8080 free"
exit 0
