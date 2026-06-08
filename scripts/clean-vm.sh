#!/usr/bin/env bash
# Remove build/test staging on the Windows UTM guest via windows-user SSH only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=vm/vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

PS1="${ROOT}/scripts/vm/clean-vm-artifacts.ps1"

KEEP_INSTALLED=""
KEEP_PYTHON=""
for arg in "$@"; do
  case "$arg" in
    --keep-installed) KEEP_INSTALLED="-KeepInstalled" ;;
    --keep-python) KEEP_PYTHON="-KeepPythonEmbed" ;;
  esac
done

vm_ssh_init
vm_ssh_require
GUEST_PS1="${GUEST_STAGING}/clean-vm-artifacts.ps1"
trap vm_ssh_close EXIT
echo "==> VM clean via windows-user SSH ($VM_SSH) staging=${GUEST_STAGING}"

vm_scp_to "$PS1" "$GUEST_PS1"
vm_ssh "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${GUEST_PS1} ${KEEP_INSTALLED} ${KEEP_PYTHON}\""

if ! ssh "${VM_SSH_MUX_OPTS[@]}" "$VM_SSH" "echo ok" >/dev/null 2>&1; then
  echo "WARNING: SSH check failed after clean — fix manually in UTM console" >&2
  exit 1
fi
