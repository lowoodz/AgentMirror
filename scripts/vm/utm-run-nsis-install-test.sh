#!/usr/bin/env bash
# Upload NSIS setup + run install/uninstall smoke on Windows VM (windows-user SSH).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

SETUP=""
while IFS= read -r candidate; do
  [[ -f "$candidate" ]] || continue
  SETUP="$candidate"
  break
done < <(ls -t \
  "${ROOT}"/dist/SafeRoute_*_x64-setup.exe \
  "${ROOT}"/dist/windows-desktop/SafeRoute_*_x64-setup.exe \
  "${ROOT}"/dist/windows-desktop/*-setup.exe 2>/dev/null || true)

[[ -n "$SETUP" ]] || {
  echo "Missing NSIS setup.exe — run ./scripts/vm/package-windows-gui.sh first" >&2
  exit 1
}

vm_ssh_require
STAGE_GUEST="${GUEST_STAGING}/smr-nsis-test-stage"
PS1_GUEST="${GUEST_STAGING}/windows-nsis-install-test.ps1"
LOG_GUEST="${GUEST_STAGING}/smr-nsis-install-test.log"

echo "==> NSIS setup: $(basename "$SETUP") on $VM_SSH"
vm_ssh_mkdir "$STAGE_GUEST"
vm_scp_to "$SETUP" "${STAGE_GUEST}/$(basename "$SETUP")"
vm_scp_to "${ROOT}/config/smr.example.yaml" "${STAGE_GUEST}/smr.example.yaml"
vm_scp_to "${ROOT}/scripts/uninstall.ps1" "${STAGE_GUEST}/uninstall.ps1"
vm_scp_to "${ROOT}/scripts/vm/windows-nsis-install-test.ps1" "$PS1_GUEST"

LOG_LOCAL="${ROOT}/dist/windows-nsis-install-test.log"
rm -f "$LOG_LOCAL"
WORK_WIN="${GUEST_STAGING}/smr-nsis-test-work"
WORK_WIN="${WORK_WIN//\//\\}"
LOG_WIN="${LOG_GUEST//\//\\}"
vm_ssh "cmd.exe /c \"del /q ${LOG_WIN} 2>nul & rmdir /s /q ${WORK_WIN} 2>nul & exit /b 0\""

echo "==> Run NSIS install test on guest (windows-user interactive session)"
vm_ssh_bg "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${PS1_GUEST}\""

DEADLINE=$((SECONDS + 900))
while (( SECONDS < DEADLINE )); do
  sleep 5
  vm_scp_from "$LOG_GUEST" "$LOG_LOCAL" 2>/dev/null || true
  if grep -q "NSIS INSTALL TEST PASSED" "$LOG_LOCAL" 2>/dev/null; then
    cat "$LOG_LOCAL"
    exit 0
  fi
  if grep -qE '(^|\]) ERROR:' "$LOG_LOCAL" 2>/dev/null; then
    cat "$LOG_LOCAL" >&2
    exit 1
  fi
  echo "... NSIS install test running (${SECONDS}s)"
done

wait "${VM_SSH_BG_PID:-0}" 2>/dev/null || true

echo "Timeout waiting for NSIS install test" >&2
[[ -s "$LOG_LOCAL" ]] && cat "$LOG_LOCAL" >&2
exit 1
