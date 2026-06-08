#!/usr/bin/env bash
# Upload Setup payload + run installed-app blackbox on Windows VM (windows-user SSH).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

CLI="${ROOT}/dist/smr.exe"
GUI="${ROOT}/dist/windows-desktop/SafeRoute.exe"
SETUP_PS1="${ROOT}/scripts/vm/windows-app-installed-test.ps1"

[[ -s "$CLI" ]] || { echo "Missing dist/smr.exe" >&2; exit 1; }
[[ -s "$GUI" ]] || { echo "Missing dist/windows-desktop/SafeRoute.exe (run package-windows-gui.sh)" >&2; exit 1; }
KEYS_SRC="$(resolve_keys_file)" || {
  echo "Missing test keys — copy config/test.env.example to config/test.env and set API keys" >&2
  exit 1
}

vm_ssh_require
STAGE_GUEST="${GUEST_STAGING}/smr-app-test-stage"
TEST_ROOT_GUEST="${GUEST_STAGING}/smr-test-suite"
PS1_GUEST="${GUEST_STAGING}/windows-app-installed-test.ps1"
LOG_GUEST="${GUEST_STAGING}/smr-app-installed-test.log"

echo "==> Upload install payload to guest ($VM_SSH)"
vm_ssh_mkdir "$STAGE_GUEST"
vm_ssh_mkdir "${TEST_ROOT_GUEST}/scripts"
vm_scp_to "$CLI" "${STAGE_GUEST}/smr.exe"
vm_scp_to "$GUI" "${STAGE_GUEST}/SafeRoute.exe"
vm_scp_to "${ROOT}/config/smr.example.yaml" "${STAGE_GUEST}/smr.example.yaml"
vm_scp_to "${ROOT}/scripts/install.ps1" "${STAGE_GUEST}/install.ps1"

for f in test_common.py blackbox_test.py generate_test_config.py; do
  vm_scp_to "${ROOT}/scripts/${f}" "${TEST_ROOT_GUEST}/scripts/${f}"
done
vm_scp_to "$KEYS_SRC" "${TEST_ROOT_GUEST}/test_model_api_key.txt"
vm_scp_to "$SETUP_PS1" "$PS1_GUEST"

echo "==> Run installed-app test on guest (windows-user SSH)"
LOG_LOCAL="${ROOT}/dist/windows-utm-installed-app-test.log"
rm -f "$LOG_LOCAL"
LOG_WIN="${LOG_GUEST//\//\\}"
vm_ssh "cmd.exe /c \"del /q ${LOG_WIN} 2>nul & exit /b 0\""
vm_ssh_bg "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${PS1_GUEST}\""

DEADLINE=$((SECONDS + 2400))
while (( SECONDS < DEADLINE )); do
  sleep 20
  vm_scp_from "$LOG_GUEST" "$LOG_LOCAL" 2>/dev/null || true
  if grep -q "INSTALLED-APP TEST PASSED" "$LOG_LOCAL" 2>/dev/null; then
    echo ""
    cat "$LOG_LOCAL"
    exit 0
  fi
  if grep -qE "INSTALLED-APP TEST FAILED|ERROR:" "$LOG_LOCAL" 2>/dev/null; then
    echo ""
    cat "$LOG_LOCAL" >&2
    exit 1
  fi
  echo "... installed-app test running (${SECONDS}s)"
done

wait "${VM_SSH_BG_PID:-0}" 2>/dev/null || true

echo "Timeout waiting for guest test" >&2
[[ -s "$LOG_LOCAL" ]] && cat "$LOG_LOCAL" >&2
exit 1
