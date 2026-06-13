#!/usr/bin/env bash
# Ensure python312 embed exists on Windows UTM guest (idempotent).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

PS1="${ROOT}/scripts/vm/bootstrap-guest-python.ps1"
vm_ssh_init
vm_ssh_require
GUEST_PS1="${GUEST_STAGING}/bootstrap-guest-python.ps1"
trap vm_ssh_close EXIT

echo "==> Bootstrap guest Python ($VM_SSH) staging=${GUEST_STAGING}"
vm_scp_to "$PS1" "$GUEST_PS1"
vm_ssh "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${GUEST_PS1}\""
echo "==> Guest Python ready"
