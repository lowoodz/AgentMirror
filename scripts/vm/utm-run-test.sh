#!/usr/bin/env bash
# Deploy and run full Windows install test via windows-user SSH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

ZIP="${ROOT}/dist/smr-*-windows-x86_64.zip"
PS1="${ROOT}/scripts/vm/windows-utm-full-test.ps1"

ZIP_FILE="$(ls -t $ZIP 2>/dev/null | head -1 || true)"
if [[ -z "$ZIP_FILE" ]]; then
  echo "Missing Windows zip. Run: ./scripts/package-windows.sh" >&2
  exit 1
fi

KEYS_SRC="$(resolve_keys_file)" || {
  echo "Missing test keys — copy config/test.env.example to config/test.env and set API keys" >&2
  exit 1
}

vm_ssh_require
LOG_GUEST="${GUEST_STAGING}/smr-test-result.txt"
KEYS_GUEST="${GUEST_STAGING}/smr-keys.env"
ZIP_GUEST="${GUEST_STAGING}/smr.zip"
PS1_GUEST="${GUEST_STAGING}/windows-utm-full-test.ps1"
CFG_GUEST="${GUEST_STAGING}/smr-vm-config.yaml"
SECRETS_GUEST="${GUEST_STAGING}/smr-secrets"

echo "==> Windows functional test on $VM_SSH"

TMP_KEYS="$(mktemp)"
python3 << PY
import re
from pathlib import Path
text = Path("$KEYS_SRC").read_text(encoding="utf-8")
glm = re.search(r"GLM\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
ds = re.search(r"Deepseek\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
if not glm or not ds:
    raise SystemExit("Could not parse keys")
Path("$TMP_KEYS").write_text(f"GLM_KEY={glm.group(1)}\nDEEPSEEK_KEY={ds.group(1)}\n", encoding="utf-8")
print("Parsed API keys OK")
PY

TMP_CFG="$(mktemp)"
export SMR_GLM_API_KEY="$(grep '^GLM_KEY=' "$TMP_KEYS" | cut -d= -f2-)"
export SMR_DEEPSEEK_API_KEY="$(grep '^DEEPSEEK_KEY=' "$TMP_KEYS" | cut -d= -f2-)"
python3 "${ROOT}/scripts/generate_test_config.py" "$TMP_CFG" "$SECRETS_GUEST"

echo "==> Upload zip ($(du -h "$ZIP_FILE" | awk '{print $1}'))"
vm_scp_to "$ZIP_FILE" "$ZIP_GUEST"
vm_scp_to "$TMP_KEYS" "$KEYS_GUEST"
vm_scp_to "$TMP_CFG" "$CFG_GUEST"
vm_scp_to "$PS1" "$PS1_GUEST"
vm_scp_to "${ROOT}/scripts/test_common.py" "${GUEST_STAGING}/test_common.py"
vm_scp_to "${ROOT}/scripts/vm/windows-file-session-check.py" "${GUEST_STAGING}/windows-file-session-check.py"
rm -f "$TMP_KEYS" "$TMP_CFG"

echo "==> Run install + functional tests on guest"
vm_ssh_bg "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${PS1_GUEST}\""

DEADLINE=$((SECONDS + 900))
while (( SECONDS < DEADLINE )); do
  sleep 15
  vm_scp_from "$LOG_GUEST" "${ROOT}/dist/windows-utm-test.log" 2>/dev/null || true
  if grep -q "SUMMARY:" "${ROOT}/dist/windows-utm-test.log" 2>/dev/null; then
    break
  fi
  done_n=$(grep -cE '^\[[0-9:]{8}\] \[(PASS|FAIL)\]' "${ROOT}/dist/windows-utm-test.log" 2>/dev/null || echo 0)
  echo "... functional tests running (${SECONDS}s, checks=${done_n})"
done
wait "${VM_SSH_BG_PID:-0}" 2>/dev/null || true
cat "${ROOT}/dist/windows-utm-test.log" 2>/dev/null || echo "(no log pulled)"

if grep -q "SUMMARY:" "${ROOT}/dist/windows-utm-test.log" 2>/dev/null; then
  tail -3 "${ROOT}/dist/windows-utm-test.log"
  if grep -E "SUMMARY: ([0-9]+)/\1 PASSED" "${ROOT}/dist/windows-utm-test.log" >/dev/null 2>&1; then
    echo "==> Windows UTM functional test PASSED"
    exit 0
  fi
  echo "==> Windows UTM functional test incomplete (see log)" >&2
  exit 1
fi

echo "==> Windows UTM functional test FAILED (see dist/windows-utm-test.log)" >&2
exit 1
