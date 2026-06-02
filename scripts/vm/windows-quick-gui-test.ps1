$log = 'C:/Users/Public/quick-gui.txt'
Remove-Item $log -Force -ErrorAction SilentlyContinue
function L($m) { Add-Content $log $m -Encoding UTF8 }

try {
    $gui = 'C:/Users/testuser/AppData/Local/Programs/SecureModelRoute/SecureModelRoute.exe'
    $stage = 'C:/Users/Public/smr-install-stage/SecureModelRoute.exe'
    $cfg = 'C:/Users/testuser/.local/etc/securemodelroute/smr.yaml'
    L "stage=$(Test-Path $stage) gui_before=$(Test-Path $gui)"
    if (Test-Path $stage) {
        New-Item -ItemType Directory -Force -Path (Split-Path $gui) | Out-Null
        Copy-Item $stage $gui -Force
    }
    L "gui_after=$(Test-Path $gui)"
    if (-not (Test-Path $gui)) { throw "GUI missing at $gui" }
    Get-Process smr, SecureModelRoute -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    $env:SMR_CONFIG = $cfg
    $p = Start-Process -FilePath $gui -PassThru -WindowStyle Normal
    Start-Sleep -Seconds 12
    L $(if ($p.HasExited) { "exit=$($p.ExitCode)" } else { 'running' })
    try {
        L "health=$((Invoke-WebRequest -Uri 'http://127.0.0.1:8080/health' -TimeoutSec 4).Content)"
    } catch {
        L "health=fail"
    }
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    L 'OK'
} catch {
    L "ERR=$($_.Exception.Message)"
}
