# Post-install on Windows VM: OpenSSH, firewall, optional Rust for verify.ps1
# Run as Administrator in PowerShell:
#   Set-ExecutionPolicy Bypass -Scope Process -Force
#   .\windows-post-install.ps1
$ErrorActionPreference = "Stop"

Write-Host "==> Enable OpenSSH Server"
$cap = Get-WindowsCapability -Online | Where-Object Name -like "OpenSSH.Server*"
if ($cap.State -ne "Installed") {
    Add-WindowsCapability -Online -Name $cap.Name
}
Set-Service -Name sshd -StartupType Automatic
Start-Service sshd
New-NetFirewallRule -Name "OpenSSH-Server-In-TCP" -DisplayName "OpenSSH Server (sshd)" `
    -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 -ErrorAction SilentlyContinue | Out-Null

Write-Host "==> sshd listening on port 22"
Get-NetTCPConnection -LocalPort 22 -State Listen -ErrorAction SilentlyContinue | Format-Table

$sshDir = Join-Path $env:USERPROFILE ".ssh"
New-Item -ItemType Directory -Force -Path $sshDir | Out-Null

$pubKeyPath = Join-Path $PSScriptRoot "authorized_keys"
if (Test-Path $pubKeyPath) {
    Copy-Item $pubKeyPath (Join-Path $sshDir "authorized_keys") -Force
    Write-Host "==> Installed authorized_keys from $pubKeyPath"
} else {
    $macPub = "$env:USERPROFILE\.ssh\id_rsa.pub"
    Write-Host "Tip: copy your Mac public key to VM:"
    Write-Host "  scp ~/.ssh/id_rsa.pub ${env:USERNAME}@<vm-ip>:$sshDir/authorized_keys"
}

Write-Host "==> Optional: install Rust (for verify.ps1 cargo test on VM)"
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Installing rustup (user scope)..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile "$env:TEMP\rustup-init.exe"
    & "$env:TEMP\rustup-init.exe" -y --default-toolchain stable
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
}

Write-Host "==> Hostname and IP"
hostname
Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -notlike "127.*" } |
    Select-Object InterfaceAlias, IPAddress | Format-Table

Write-Host @"

Done. From macOS:
  1. Note the VM IPv4 above (bridged LAN).
  2. Set SMR_WINDOWS_HOST / SMR_WINDOWS_USER in config/test.env (see config/test.env.example)
  3. ./scripts/windows_vm_test.sh

"@
