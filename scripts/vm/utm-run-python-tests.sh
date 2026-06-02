#!/usr/bin/env bash
# Upload and run blackbox + stress tests on Windows UTM guest (skip functional).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
VM_ID="${SMR_UTM_VM:-Windows}"
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
SUITE_GUEST="C:/Users/Public/smr-test-suite"
PY_LOG="C:/Users/Public/smr-python-test.log"
PY_PS1="${ROOT}/scripts/vm/windows-run-python-tests.ps1"

echo "========== Upload Python test suite =========="
"$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "if not exist C:\Users\Public\smr-test-suite\scripts mkdir C:\Users\Public\smr-test-suite\scripts" 2>/dev/null || true

for f in test_common.py blackbox_test.py live_test.py; do
  cat "${ROOT}/scripts/${f}" | "$UTMCTL" file push "$VM_ID" "${SUITE_GUEST}/scripts/${f}"
done
KEYS_SRC="$(resolve_keys_file)" || {
  echo "Missing test keys — copy config/test.env.example to config/test.env and set API keys" >&2
  exit 1
}
cat "${KEYS_SRC}" | "$UTMCTL" file push "$VM_ID" "${SUITE_GUEST}/test_model_api_key.txt"
cat "$PY_PS1" | "$UTMCTL" file push "$VM_ID" "C:/Users/Public/windows-run-python-tests.ps1"

echo "========== Blackbox + stress on guest =========="
"$UTMCTL" exec "$VM_ID" --cmd powershell.exe -NoProfile -Command "Start-Process powershell.exe -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','C:/Users/Public/windows-run-python-tests.ps1' -WindowStyle Hidden" 2>/dev/null || true

DEADLINE=$((SECONDS + 2400))
while (( SECONDS < DEADLINE )); do
  sleep 30
  "$UTMCTL" file pull "$VM_ID" "$PY_LOG" > "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null || true
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

echo "==> Timeout waiting for Python tests" >&2
exit 1
