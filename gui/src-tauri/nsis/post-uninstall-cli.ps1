$bin = Join-Path $env:USERPROFILE '.local\bin'
foreach ($name in @('smr.exe', 'securemodelroute.cmd', 'SafeRoute.cmd', 'SecureModelRoute.cmd', 'smr-service.cmd')) {
    $p = Join-Path $bin $name
    if (Test-Path $p) { Remove-Item $p -Force }
}
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($path) {
    $parts = $path -split ';' | Where-Object { $_ -and ($_ -ne $bin) }
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
}
