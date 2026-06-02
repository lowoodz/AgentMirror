# Load config/test.env into the current PowerShell session (no-op if missing).
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$EnvFile = if ($env:SMR_TEST_ENV) { $env:SMR_TEST_ENV } else { Join-Path $Root "config\test.env" }

if (Test-Path $EnvFile) {
    Get-Content $EnvFile | ForEach-Object {
        $line = $_.Trim()
        if (-not $line -or $line.StartsWith("#")) { return }
        $idx = $line.IndexOf("=")
        if ($idx -lt 1) { return }
        $key = $line.Substring(0, $idx).Trim()
        $val = $line.Substring($idx + 1).Trim().Trim('"').Trim("'")
        if ($key -and [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($key))) {
            Set-Item -Path "Env:$key" -Value $val
        }
    }
}

function Test-SmrKeys {
    if ($env:SMR_GLM_API_KEY -and $env:SMR_DEEPSEEK_API_KEY) { return $true }
    $keysFile = if ($env:SMR_KEYS_FILE) { $env:SMR_KEYS_FILE } else { Join-Path $Root "test_model_api_key.txt" }
    return Test-Path $keysFile
}
