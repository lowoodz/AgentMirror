$bin = Join-Path $env:USERPROFILE '.local\bin'
$cfg = Join-Path $env:USERPROFILE '.local\etc\securemodelroute\smr.yaml'
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $path) { $path = '' }
if ($path -notlike ('*' + $bin + '*')) {
    $newPath = ($path.TrimEnd(';') + ';' + $bin).Trim(';')
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
}
[Environment]::SetEnvironmentVariable('SMR_CONFIG', $cfg, 'User')
