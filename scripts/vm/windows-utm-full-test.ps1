# Full install + functional smoke test on Windows (UTM guest agent / manual).
# Logs to C:\Users\Public\smr-test-result.txt
param(
    [string]$ZipPath = "C:\Users\Public\smr.zip",
    [string]$KeysPath = "C:\Users\Public\smr-keys.env",
    [string]$LogPath = "C:\Users\Public\smr-test-result.txt"
)

$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"

function Log($msg) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    Write-Host $line
}

function Check($name, $ok, $detail) {
    $mark = if ($ok) { "PASS" } else { "FAIL" }
    Log "[$mark] ${name}: $detail"
    return [bool]$ok
}

$results = @()
$Base = "http://127.0.0.1:8080"
$ContentSecret = "LOCAL-INSTALL-TEST-SECRET"
$FileProbeSecret = "LOCAL-INSTALL-FILE-SECRET-XYZ"
$WorkDir = "C:\Users\Public\smr-work"
$SecretsDir = "C:\Users\Public\smr-secrets"
$VaultDir = Join-Path $SecretsDir "vault"
$Prefix = "C:\Users\Public\smr-home"
$BinDir = Join-Path $Prefix "bin"
$ConfDir = Join-Path $Prefix "etc\securemodelroute"
$SmrExe = Join-Path $BinDir "smr.exe"
$Config = Join-Path $ConfDir "smr.yaml"
$SmrLog = "C:\Users\Public\smr-service.log"
$SmrErr = "C:\Users\Public\smr-service.err"

Remove-Item $LogPath -Force -ErrorAction SilentlyContinue
Log "==> SecureModelRoute Windows UTM full test"
Log "Arch: $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
Log "Process: $([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture)"

# Parse keys file (optional sanity check)
if (Test-Path $KeysPath) {
    Log "Keys file present"
}

# Stop prior smr
Get-Process smr -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

function Write-Utf8NoBom($Path, $Value) {
    $enc = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $Value, $enc)
}

# Prepare secrets for file DLP / path protection (no BOM — Set-Content UTF8 adds BOM and breaks full-match DLP)
New-Item -ItemType Directory -Force -Path $VaultDir | Out-Null
Write-Utf8NoBom (Join-Path $SecretsDir "probe.txt") "LOCAL-INSTALL-FILE-SECRET-XYZ"
Write-Utf8NoBom (Join-Path $SecretsDir "project.txt") "project-data"
Write-Utf8NoBom (Join-Path $VaultDir "secret.txt") "vault-secret-data"

# Extract + install (fixed prefix for guest-agent SYSTEM account)
if (-not (Test-Path $ZipPath)) { Log "ERROR: zip not found: $ZipPath"; exit 1 }
Remove-Item $WorkDir -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive -Path $ZipPath -DestinationPath $WorkDir -Force
New-Item -ItemType Directory -Force -Path $BinDir, $ConfDir | Out-Null
Copy-Item (Join-Path $WorkDir "smr.exe") $SmrExe -Force
Log "Installed smr.exe -> $SmrExe"
if (-not (Test-Path $SmrExe)) { Log "ERROR: smr.exe missing after copy"; exit 1 }

# Use pre-generated config pushed from host (smr-vm-config.yaml)
$ConfigSrc = "C:\Users\Public\smr-vm-config.yaml"
if (-not (Test-Path $ConfigSrc)) { Log "ERROR: config not found: $ConfigSrc"; exit 1 }
Copy-Item $ConfigSrc $Config -Force
Log "Installed config $Config"

# Start service
Remove-Item $SmrLog,$SmrErr -Force -ErrorAction SilentlyContinue
$proc = Start-Process -FilePath $SmrExe -ArgumentList @("--config", $Config) -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $SmrLog -RedirectStandardError $SmrErr
Start-Sleep -Seconds 5
if ($proc.HasExited) {
    Log "ERROR: smr exited early code=$($proc.ExitCode)"
    if (Test-Path $SmrErr) { Get-Content $SmrErr | ForEach-Object { Log "stderr: $_" } }
    if (Test-Path $SmrLog) { Get-Content $SmrLog | ForEach-Object { Log "stdout: $_" } }
}

function Wait-Ready {
    for ($i = 0; $i -lt 60; $i++) {
        try {
            $h = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 3
            if ("$h" -match "OK") {
                $st = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 3
                if ($st.file_index_ready) { return $true }
            }
        } catch {}
        Start-Sleep -Seconds 1
    }
    return $false
}

$results += Check "service_ready" (Wait-Ready) "pid=$($proc.Id)"

function Latest-Audit {
    try {
        $r = Invoke-RestMethod -Uri "$Base/api/audits?limit=1" -TimeoutSec 10
        if ($r.audits -and $r.audits.Count -gt 0) { return $r.audits[0] }
    } catch {}
    return $null
}

try {
    $health = Invoke-RestMethod -Uri "$Base/health" -TimeoutSec 5
    $results += Check "health" ("$health" -match "OK") "value=$health"
} catch { Log "ERROR health: $($_.Exception.Message)"; $results += $false }

try {
    $status = Invoke-RestMethod -Uri "$Base/api/status" -TimeoutSec 5
    $results += Check "status_api" ($status.file_index_ready -eq $true) "security=$($status.security_enabled)"
} catch { Log "ERROR status: $($_.Exception.Message)"; $results += $false }

try {
    $ui = Invoke-WebRequest -Uri "$Base/ui" -TimeoutSec 10 -UseBasicParsing
    $results += Check "web_ui" ($ui.Content -match "SafeRoute|SecureModelRoute") "bytes=$($ui.Content.Length)"
} catch { Log "ERROR ui: $($_.Exception.Message)"; $results += $false }

try {
    $body = @{
        model = "deepseek-chat"
        messages = @(@{ role = "user"; content = "Reply exactly: install-ok" })
        max_tokens = 16
    } | ConvertTo-Json -Depth 5 -Compress
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $chat = Invoke-RestMethod -Uri "$Base/v1/chat/completions" -Method Post -Body $body -ContentType "application/json" -TimeoutSec 120
    $sw.Stop()
    $reply = $chat.choices[0].message.content
    $results += Check "chat_route" ($reply.Length -gt 0) "$([int]$sw.ElapsedMilliseconds)ms reply=$($reply.Substring(0, [Math]::Min(30, $reply.Length)))"
} catch { Log "ERROR chat: $($_.Exception.Message)"; $results += $false }

try {
    $streamBody = @{
        model = "deepseek-chat"
        messages = @(@{ role = "user"; content = "Count 1 2 3 briefly." })
        max_tokens = 24
        stream = $true
    } | ConvertTo-Json -Depth 5 -Compress
    $streamBodyFile = "C:\Users\Public\smr-stream-body.json"
    $streamFile = "C:\Users\Public\smr-stream-out.txt"
    [System.IO.File]::WriteAllText($streamBodyFile, $streamBody, [System.Text.UTF8Encoding]::new($false))
    $sw.Restart()
    & curl.exe -s -N -X POST "$Base/v1/chat/completions" -H "Content-Type: application/json" --data-binary "@$streamBodyFile" -o $streamFile --max-time 120
    $sw.Stop()
    $raw = Get-Content -Path $streamFile -Raw -ErrorAction SilentlyContinue
    if (-not $raw) { $raw = "" }
    $chunks = @([regex]::Matches($raw, '(?m)^data: ')).Count
    if ($chunks -eq 0 -and $raw -match 'data:') { $chunks = 1 }
    if ($chunks -lt 1) { Log "streaming raw preview: $($raw.Substring(0, [Math]::Min(120, $raw.Length)))" }
    $results += Check "streaming" ($chunks -ge 1) "$([int]$sw.ElapsedMilliseconds)ms chunks=$chunks bytes=$($raw.Length)"
} catch { Log "ERROR streaming: $($_.Exception.Message)"; $results += $false }

try {
    $dlpBody = @{
        model = "deepseek-chat"
        messages = @(@{ role = "user"; content = "My secret is $ContentSecret" })
        max_tokens = 12
    } | ConvertTo-Json -Depth 5 -Compress
    $sw.Restart()
    Invoke-RestMethod -Uri "$Base/v1/chat/completions" -Method Post -Body $dlpBody -ContentType "application/json" -Headers @{ "X-SMR-Session-Id" = "win-install-dlp" } -TimeoutSec 120 | Out-Null
    $sw.Stop()
    $audit = Latest-Audit
    $dlp = if ($audit) { [int]$audit.dlp_replacements } else { 0 }
    $results += Check "content_dlp" ($dlp -gt 0) "$([int]$sw.ElapsedMilliseconds)ms dlp=$dlp"
} catch { Log "ERROR dlp: $($_.Exception.Message)"; $results += $false }

try {
    $fbBody = @{
        model = "deepseek-chat"
        messages = @(@{ role = "user"; content = "Say ok" })
        max_tokens = 8
    } | ConvertTo-Json -Depth 5 -Compress
    $sw.Restart()
    Invoke-RestMethod -Uri "$Base/v1/chat/completions" -Method Post -Body $fbBody -ContentType "application/json" -Headers @{ "X-SMR-Fallback-Group" = "fallback-test" } -TimeoutSec 120 | Out-Null
    $sw.Stop()
    $audit = Latest-Audit
    $chain = if ($audit) { @($audit.fallback_chain) } else { @() }
    $results += Check "fallback" ($chain.Count -ge 2) "$([int]$sw.ElapsedMilliseconds)ms chain=$($chain -join ',')"
} catch { Log "ERROR fallback: $($_.Exception.Message)"; $results += $false }

try {
    $anthBody = @{
        model = "deepseek-chat"
        max_tokens = 16
        messages = @(@{ role = "user"; content = "Say hi" })
    } | ConvertTo-Json -Depth 5 -Compress
    $sw.Restart()
    $anth = Invoke-RestMethod -Uri "$Base/v1/messages" -Method Post -Body $anthBody -ContentType "application/json" -Headers @{ "X-SMR-Fallback-Group" = "glm-anthropic" } -TimeoutSec 120
    $sw.Stop()
    $anthText = ($anth | ConvertTo-Json -Compress)
    $results += Check "anthropic_api" ($anthText -match "content") "$([int]$sw.ElapsedMilliseconds)ms"
} catch { Log "ERROR anthropic: $($_.Exception.Message)"; $results += $false }

try {
    $audits = Invoke-RestMethod -Uri "$Base/api/audits?limit=3" -TimeoutSec 10
    $n = @($audits.audits).Count
    $results += Check "audit_log" ($n -gt 0) "records=$n"
} catch { Log "ERROR audit: $($_.Exception.Message)"; $results += $false }

try {
    $probePath = (Join-Path $SecretsDir "probe.txt") -replace '\\','/'
    $probeTrigger = @{
        model = "deepseek-chat"
        messages = @(
            @{
                role = "assistant"
                content = $null
                tool_calls = @(
                    @{
                        id = "call_probe"
                        type = "function"
                        function = @{
                            name = "read_file"
                            arguments = (@{ path = $probePath } | ConvertTo-Json -Compress)
                        }
                    }
                )
            }
        )
        max_tokens = 8
    } | ConvertTo-Json -Depth 8 -Compress
    Invoke-RestMethod -Uri "$Base/v1/chat/completions" -Method Post -Body $probeTrigger -ContentType "application/json" -Headers @{ "X-SMR-Session-Id" = "win-install-file-session" } -TimeoutSec 120 -ErrorAction SilentlyContinue | Out-Null
} catch {
    Log "file_session trigger HTTP note: $($_.Exception.Message)"
}
try {
    $fileBody = @{
        model = "deepseek-chat"
        messages = @(@{ role = "user"; content = "file probe secret: $FileProbeSecret" })
        max_tokens = 16
    } | ConvertTo-Json -Depth 5 -Compress
    $sw.Restart()
    Invoke-RestMethod -Uri "$Base/v1/chat/completions" -Method Post -Body $fileBody -ContentType "application/json" -Headers @{ "X-SMR-Session-Id" = "win-install-file-session" } -TimeoutSec 120 | Out-Null
    $sw.Stop()
    $audit = Latest-Audit
    $dlp = if ($audit) { [int]$audit.dlp_replacements } else { 0 }
    $results += Check "file_session_dlp" ($dlp -gt 0) "$([int]$sw.ElapsedMilliseconds)ms dlp=$dlp"
} catch { Log "ERROR file_session_dlp: $($_.Exception.Message)"; $results += $false }

try {
    $pathStr = (Join-Path $SecretsDir "project.txt") -replace '\\','/'
    $pathBody = @{
        model = "deepseek-chat"
        messages = @(
            @{
                role = "assistant"
                content = $null
                tool_calls = @(
                    @{
                        id = "call_1"
                        type = "function"
                        function = @{
                            name = "read_file"
                            arguments = (@{ path = $pathStr } | ConvertTo-Json -Compress)
                        }
                    }
                )
            }
        )
        max_tokens = 8
    } | ConvertTo-Json -Depth 8 -Compress
    $sw.Restart()
    try {
        Invoke-RestMethod -Uri "$Base/v1/chat/completions" -Method Post -Body $pathBody -ContentType "application/json" -TimeoutSec 120 | Out-Null
    } catch {
        Log "path_protection HTTP note: $($_.Exception.Message)"
    }
    $sw.Stop()
    $audit = Latest-Audit
    $blocks = if ($audit -and $audit.safety_blocks) { [int]$audit.safety_blocks } else { 0 }
    $results += Check "path_protection" ($blocks -gt 0) "$([int]$sw.ElapsedMilliseconds)ms blocks=$blocks path=$pathStr"
} catch { Log "ERROR path_protection: $($_.Exception.Message)"; $results += $false }

if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }

$passed = ($results | Where-Object { $_ }).Count
$total = $results.Count
Log ""
Log "SUMMARY: $passed/$total PASSED"
if ($passed -eq $total) { exit 0 } else { exit 1 }
