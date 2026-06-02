#!/usr/bin/env bash
# Full Windows UTM test suite: functional (PS1) + blackbox + stress (Python on guest).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
VM_ID="${SMR_UTM_VM:-Windows}"
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
SUITE_GUEST="C:/Users/Public/smr-test-suite"
PY_LOG="C:/Users/Public/smr-python-test.log"
PY_PS1="${ROOT}/scripts/vm/windows-run-python-tests.ps1"

echo "========== Phase 1: Functional install test =========="
FUNC_OK=0
if bash "${ROOT}/scripts/vm/utm-run-test.sh"; then
  FUNC_OK=1
else
  echo "Functional test did not fully pass; continuing with blackbox/stress..." >&2
fi

echo ""
echo "========== Phase 2: Upload Python test suite =========="
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

echo ""
echo "========== Phase 3: Blackbox + stress on guest =========="
"$UTMCTL" exec "$VM_ID" --cmd powershell.exe -NoProfile -Command "Start-Process powershell.exe -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','C:/Users/Public/windows-run-python-tests.ps1' -WindowStyle Hidden" 2>/dev/null || true

DEADLINE=$((SECONDS + 3600))
while (( SECONDS < DEADLINE )); do
  sleep 30
  "$UTMCTL" file pull "$VM_ID" "$PY_LOG" > "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null || true
  if grep -q "Python tests PASSED" "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null; then
    break
  fi
  if grep -q "Python tests FAILED" "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null; then
    break
  fi
  echo "... python tests running (${SECONDS}s)"
done
echo ""
echo "----- blackbox + stress log -----"
cat "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null || echo "(no log)"

if [[ "$FUNC_OK" -eq 1 ]] && grep -q "Python tests PASSED" "${ROOT}/dist/windows-utm-python-test.log" 2>/dev/null; then
  echo ""
  echo "========== ALL WINDOWS UTM TESTS PASSED =========="
  exit 0
fi

echo ""
echo "========== SOME TESTS FAILED ==========" >&2
echo "  functional: dist/windows-utm-test.log" >&2
echo "  blackbox/stress: dist/windows-utm-python-test.log" >&2
exit 1
