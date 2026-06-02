#!/usr/bin/env bash
# Upload Setup payload + run installed-app blackbox on Windows UTM guest.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VM_ID="${SMR_UTM_VM:-Windows}"
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
STAGE_GUEST="C:/Users/Public/smr-app-test-stage"
TEST_ROOT_GUEST="C:/Users/Public/smr-test-suite"
PS1_GUEST="C:/Users/Public/windows-app-installed-test.ps1"
LOG_GUEST="C:/Users/Public/smr-app-installed-test.log"

CLI="${ROOT}/dist/smr.exe"
GUI="${ROOT}/dist/windows-desktop/SecureModelRoute.exe"
SETUP_PS1="${ROOT}/scripts/vm/windows-app-installed-test.ps1"

[[ -s "$CLI" ]] || { echo "Missing dist/smr.exe" >&2; exit 1; }
[[ -s "$GUI" ]] || { echo "Missing dist/windows-desktop/SecureModelRoute.exe" >&2; exit 1; }
[[ -f "${ROOT}/test_model_api_key.txt" ]] || { echo "Missing test_model_api_key.txt — copy from test_model_api_key.example.txt" >&2; exit 1; }

echo "==> Upload install payload to guest"
"$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "mkdir C:\\Users\\Public\\smr-app-test-stage 2>nul & mkdir C:\\Users\\Public\\smr-test-suite\\scripts 2>nul" 2>/dev/null || true
cat "$CLI" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/smr.exe"
cat "$GUI" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/SecureModelRoute.exe"
cat "${ROOT}/config/smr.example.yaml" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/smr.example.yaml"
cat "${ROOT}/scripts/install.ps1" | "$UTMCTL" file push "$VM_ID" "${STAGE_GUEST}/install.ps1"

for f in test_common.py blackbox_test.py generate_test_config.py; do
  cat "${ROOT}/scripts/${f}" | "$UTMCTL" file push "$VM_ID" "${TEST_ROOT_GUEST}/scripts/${f}"
done
cat "${ROOT}/test_model_api_key.txt" | "$UTMCTL" file push "$VM_ID" "${TEST_ROOT_GUEST}/test_model_api_key.txt"
cat "$SETUP_PS1" | "$UTMCTL" file push "$VM_ID" "$PS1_GUEST"

echo "==> Run installed-app test on guest"
"$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "del /q C:\\Users\\Public\\smr-app-installed-test.log 2>nul" 2>/dev/null || true
"$UTMCTL" exec "$VM_ID" --cmd powershell.exe -NoProfile -Command "Start-Process powershell.exe -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','C:/Users/Public/windows-app-installed-test.ps1' -WindowStyle Hidden" 2>/dev/null || true

LOG_LOCAL="${ROOT}/dist/windows-utm-installed-app-test.log"
rm -f "$LOG_LOCAL"
DEADLINE=$((SECONDS + 2400))
while (( SECONDS < DEADLINE )); do
  sleep 20
  "$UTMCTL" file pull "$VM_ID" "$LOG_GUEST" > "$LOG_LOCAL" 2>/dev/null || true
  if grep -q "INSTALLED-APP TEST PASSED" "$LOG_LOCAL" 2>/dev/null; then
    echo ""
    cat "$LOG_LOCAL"
    exit 0
  fi
  if grep -q "INSTALLED-APP TEST FAILED" "$LOG_LOCAL" 2>/dev/null; then
    echo ""
    cat "$LOG_LOCAL" >&2
    exit 1
  fi
  if grep -q "^ERROR:" "$LOG_LOCAL" 2>/dev/null; then
    echo ""
    cat "$LOG_LOCAL" >&2
    exit 1
  fi
  echo "... installed-app test running (${SECONDS}s)"
done

echo "Timeout waiting for guest test" >&2
[[ -s "$LOG_LOCAL" ]] && cat "$LOG_LOCAL" >&2
exit 1
