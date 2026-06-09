#!/usr/bin/env bash
# Stop smr / tray GUI on the Windows VM and ensure :8080 is free.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

PS1="${ROOT}/scripts/vm/stop-guest-smr.ps1"
vm_ssh_require
GUEST_PS1="${GUEST_STAGING}/stop-guest-smr.ps1"
vm_scp_to "$PS1" "$GUEST_PS1"
vm_ssh "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${GUEST_PS1}"
