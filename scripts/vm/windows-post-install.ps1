# Post-install on Windows VM: optional Rust for verify.ps1
# OpenSSH must be configured manually — see scripts/vm/WINDOWS_VM.md § SSH 手动配置
# Run as Administrator in PowerShell:
#   Set-ExecutionPolicy Bypass -Scope Process -Force
#   .\windows-post-install.ps1
$ErrorActionPreference = "Stop"

Write-Host "==> OpenSSH: skipped (configure manually — see WINDOWS_VM.md § SSH 手动配置)"

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
