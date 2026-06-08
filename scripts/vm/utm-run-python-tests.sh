#!/usr/bin/env bash
# Upload and run blackbox + stress tests on Windows VM (windows-user SSH).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

PY_PS1="${ROOT}/scripts/vm/windows-run-python-tests.ps1"

vm_ssh_require
SUITE_GUEST="${GUEST_STAGING}/smr-test-suite"
PY_LOG="${GUEST_STAGING}/smr-python-test.log"
PY_PS1_GUEST="${GUEST_STAGING}/windows-run-python-tests.ps1"
echo "========== Upload Python test suite ($VM_SSH) =========="
vm_ssh_mkdir "${SUITE_GUEST}/scripts"

for f in test_common.py blackbox_test.py live_test.py; do
  vm_scp_to "${ROOT}/scripts/${f}" "${SUITE_GUEST}/scripts/${f}"
done
KEYS_SRC="$(resolve_keys_file)" || {
  echo "Missing test keys — copy config/test.env.example to config/test.env and set API keys" >&2
  exit 1
}
vm_scp_to "$KEYS_SRC" "${SUITE_GUEST}/test_model_api_key.txt"
vm_scp_to "$PY_PS1" "$PY_PS1_GUEST"

echo "========== Blackbox + stress on guest =========="
vm_ssh_bg "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${GUEST_STAGING}/windows-run-python-tests.ps1\""

DEADLINE=$((SECONDS + 2400))
while (( SECONDS < DEADLINE )); do
  sleep 30
  vm_scp_from "$PY_LOG" "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null || true
  if grep -q "Python tests PASSED" "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null; then
    echo "==> Python tests PASSED"
    cat "${ROOT}/dist/windows-utm-python-test.log"
    exit 0
  fi
  if grep -q "Python tests FAILED" "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null; then
    echo "==> Python tests FAILED" >&2
    cat "${ROOT}/dist/windows-utm-python-test.log"
    exit 1
  fi
  tail -3 "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null || true
  echo "... python tests running (${SECONDS}s)"
done

wait "${VM_SSH_BG_PID:-0}" 2>/dev/null || true

echo "==> Timeout waiting for Python tests" >&2
exit 1
