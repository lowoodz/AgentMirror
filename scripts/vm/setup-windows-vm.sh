#!/usr/bin/env bash
# Bootstrap Windows x86_64 VM on macOS (Apple Silicon) for SecureModelRoute testing.
# - Installs UTM (free) if missing
# - Downloads Windows 11 x64 ISO (evaluation)
# - Creates UTM QEMU VM with bridged networking (matches windows_vm_test.sh SSH settings)
#
# Usage:
#   ./scripts/vm/setup-windows-vm.sh install-utm
#   ./scripts/vm/setup-windows-vm.sh download-iso
#   ./scripts/vm/setup-windows-vm.sh create-vm
#   ./scripts/vm/setup-windows-vm.sh all
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VM_DIR="${SMR_VM_DIR:-${HOME}/VMs/SecureModelRoute-Win11-x64}"
ISO_PATH="${VM_DIR}/isos/Win11_24H2_English_x64.iso"
VM_NAME="${SMR_VM_NAME:-SecureModelRoute-Win11-x64}"
UTM_DMG="${VM_DIR}/UTM.dmg"
UTM_URL="${SMR_UTM_URL:-https://github.com/utmapp/UTM/releases/download/v4.6.5/UTM.dmg}"
MAS_APP_ID="1538878817"

log() { printf '==> %s\n' "$*"; }
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "Missing command: $1"
}

install_utm() {
  if [[ -d /Applications/UTM.app ]]; then
    log "UTM already installed at /Applications/UTM.app"
    return 0
  fi

  mkdir -p "${VM_DIR}/isos"

  if command -v mas >/dev/null 2>&1; then
    log "Installing UTM from Mac App Store (mas)..."
    mas install "${MAS_APP_ID}" || true
    if [[ -d /Applications/UTM.app ]]; then
      return 0
    fi
  fi

  if command -v brew >/dev/null 2>&1; then
    log "Trying: brew install --cask utm"
    if brew install --cask utm 2>/dev/null; then
      [[ -d /Applications/UTM.app ]] && return 0
    fi
    log "brew install failed (check Homebrew permissions). Falling back to direct download."
  fi

  log "Downloading UTM.dmg from GitHub..."
  need_cmd curl
  curl -fL --retry 5 --retry-delay 3 --http1.1 \
    -H "User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" \
    -o "${UTM_DMG}" "${UTM_URL}"

  log "Mounting DMG and copying UTM.app..."
  need_cmd hdiutil
  mount_point="$(hdiutil attach "${UTM_DMG}" -nobrowse | awk '/\/Volumes\// {print $3; exit}')"
  [[ -n "${mount_point}" ]] || die "Failed to mount ${UTM_DMG}"
  cp -R "${mount_point}/UTM.app" /Applications/
  hdiutil detach "${mount_point}" -quiet
  log "UTM installed to /Applications/UTM.app"
}

download_iso() {
  mkdir -p "${VM_DIR}/isos"
  if [[ -f "${ISO_PATH}" ]]; then
    log "ISO already exists: ${ISO_PATH}"
    return 0
  fi

  log "Fetching Windows 11 x64 ISO via Microsoft download API..."
  need_cmd python3
  python3 "${ROOT}/scripts/vm/download-win11-iso.py" "${ISO_PATH}"
  [[ -f "${ISO_PATH}" ]] || die "ISO download failed"
  log "ISO ready: ${ISO_PATH}"
}

create_vm() {
  [[ -d /Applications/UTM.app ]] || die "UTM not installed. Run: $0 install-utm"
  [[ -f "${ISO_PATH}" ]] || die "ISO missing. Run: $0 download-iso"

  log "Creating UTM VM '${VM_NAME}' (QEMU x86_64, bridged network)..."
  export SMR_VM_NAME="${VM_NAME}" SMR_VM_ISO="${ISO_PATH}"
  osascript "${ROOT}/scripts/vm/create-utm-vm.applescript"

  log "VM created. Open UTM, start '${VM_NAME}', and complete Windows setup."
  log "After install, on Windows run (Admin PowerShell):"
  log "  Set-ExecutionPolicy Bypass -Scope Process; .\\scripts\\vm\\windows-post-install.ps1"
  log "Then from macOS: ./scripts/windows_vm_test.sh"
}

print_ssh_hint() {
  cat <<EOF

SSH (after Windows is up with OpenSSH):
  Set in config/test.env (copy from config/test.env.example):
    SMR_WINDOWS_HOST=<VM-LAN-IP or ~/.ssh/config Host alias>
    SMR_WINDOWS_USER=<Windows login name>

  Example ~/.ssh/config:
    Host smr-win-vm
      HostName 192.168.1.100
      User your-windows-user
      IdentityFile ~/.ssh/id_rsa

Test: ./scripts/windows_vm_test.sh

Note: On Apple Silicon, x86_64 Windows runs under QEMU emulation (slow but matches
x86_64-pc-windows-gnu). For faster iteration, consider Windows 11 ARM + same zip
(most x86_64 CLI binaries run via WOW64); use create-utm-vm-arm.applescript if added.

EOF
}

usage() {
  cat <<EOF
Usage: $0 <command>

Commands:
  install-utm   Install UTM (App Store / brew / GitHub DMG)
  download-iso  Download Windows 11 x64 evaluation ISO
  create-vm     Create UTM VM with ISO attached
  all           install-utm + download-iso + create-vm

Environment:
  SMR_VM_DIR       VM files directory (default: ~/VMs/SecureModelRoute-Win11-x64)
  SMR_VM_NAME      UTM VM display name
  SMR_UTM_URL      Override UTM.dmg download URL

  Windows SSH test vars live in config/test.env (see config/test.env.example):
  SMR_WINDOWS_HOST, SMR_WINDOWS_USER, SMR_WINDOWS_REMOTE_DIR

EOF
}

main() {
  local cmd="${1:-}"
  case "${cmd}" in
    install-utm) install_utm ;;
    download-iso) download_iso ;;
    create-vm) create_vm; print_ssh_hint ;;
    all)
      install_utm
      download_iso
      create_vm
      print_ssh_hint
      ;;
    ""|-h|--help) usage ;;
    *) die "Unknown command: ${cmd}. Run: $0 --help" ;;
  esac
}

main "$@"
