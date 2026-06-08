#!/usr/bin/env bash
# Windows install smoke on VM via windows-user SSH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

ZIP="$(ls -t "${ROOT}"/dist/smr-*-windows-x86_64.zip 2>/dev/null | head -1)"
GUI="${ROOT}/dist/windows-desktop/SafeRoute.exe"
[[ -n "$ZIP" ]] || { echo "Missing dist/smr-*-windows-x86_64.zip" >&2; exit 1; }
[[ -s "$GUI" ]] || { echo "Missing dist/windows-desktop/SafeRoute.exe" >&2; exit 1; }

vm_ssh_require
PS1_GUEST="${GUEST_STAGING}/windows-install-smoke.ps1"
LOG_GUEST="${GUEST_STAGING}/smr-install-smoke.log"
STAGE_GUEST="${GUEST_STAGING}/smr-app-test-stage"

vm_ssh_mkdir "$STAGE_GUEST"
vm_scp_to "$ZIP" "${GUEST_STAGING}/smr.zip"
vm_scp_to "$GUI" "${STAGE_GUEST}/SafeRoute.exe"
vm_scp_to "${ROOT}/scripts/vm/windows-install-smoke.ps1" "$PS1_GUEST"

LOG_LOCAL="${ROOT}/dist/windows-install-smoke.log"
rm -f "$LOG_LOCAL"
vm_ssh "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${GUEST_STAGING}/windows-install-smoke.ps1\""

DEADLINE=$((SECONDS + 300))
while (( SECONDS < DEADLINE )); do
  sleep 5
  vm_scp_from "$LOG_GUEST" "$LOG_LOCAL" 2>/dev/null || true
  if grep -q "INSTALL SMOKE TEST PASSED" "$LOG_LOCAL" 2>/dev/null; then
    cat "$LOG_LOCAL"
    exit 0
  fi
  if grep -qE "ERROR:" "$LOG_LOCAL" 2>/dev/null; then
    cat "$LOG_LOCAL" >&2
    exit 1
  fi
done

echo "Timeout" >&2
[[ -s "$LOG_LOCAL" ]] && cat "$LOG_LOCAL" >&2
exit 1
