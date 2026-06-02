#!/usr/bin/env bash
# Deploy and run full Windows install test via UTM guest agent (no SSH required).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
VM_ID="${SMR_UTM_VM:-Windows}"
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
ZIP="${ROOT}/dist/smr-*-windows-x86_64.zip"
PS1="${ROOT}/scripts/vm/windows-utm-full-test.ps1"
LOG_GUEST="C:/Users/Public/smr-test-result.txt"
KEYS_GUEST="C:/Users/Public/smr-keys.env"
ZIP_GUEST="C:/Users/Public/smr.zip"
PS1_GUEST="C:/Users/Public/windows-utm-full-test.ps1"
CFG_GUEST="C:/Users/Public/smr-vm-config.yaml"

if [[ ! -x "$UTMCTL" ]]; then
  echo "UTM not found at $UTMCTL" >&2
  exit 1
fi

ZIP_FILE="$(ls -t $ZIP 2>/dev/null | head -1 || true)"
if [[ -z "$ZIP_FILE" ]]; then
  echo "Missing Windows zip. Run: ./scripts/package-windows.sh" >&2
  exit 1
fi

KEYS_SRC="$(resolve_keys_file)" || {
  echo "Missing test keys — copy config/test.env.example to config/test.env and set API keys" >&2
  exit 1
}

echo "==> VM: $("$UTMCTL" list | awk '/started/ {print}' || true)"
echo "==> Guest IP: $("$UTMCTL" ip-address "$VM_ID" 2>/dev/null | head -1 || echo unknown)"

# Build keys env (do not commit)
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
python3 << PY
from pathlib import Path
glm = open("$TMP_KEYS").read().split("GLM_KEY=")[1].split()[0]
ds = open("$TMP_KEYS").read().split("DEEPSEEK_KEY=")[1].split()[0]
secrets = r"C:/Users/Public/smr-secrets"
vault = secrets + "/vault"
content_secret = "LOCAL-INSTALL-TEST-SECRET"
cfg = f'''server:
  listen: "127.0.0.1:8080"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce
  builtin_credential_presets: true

logging:
  level: info
  redact_content: true

fallback_groups:
  high:
    - id: glm-primary
      base_url: "https://open.bigmodel.cn/api/coding/paas/v4"
      model: "glm-4-flash"
      api_key: "{glm}"
      protocol: openai
      timeout_secs: 90
    - id: deepseek-fallback
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds}"
      protocol: openai
      timeout_secs: 90
  fallback-test:
    - id: dead-endpoint
      base_url: "http://127.0.0.1:9"
      model: "fake-model"
      api_key: "dead"
      timeout_secs: 3
    - id: deepseek-rescue
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds}"
      protocol: openai
      timeout_secs: 90
  glm-anthropic:
    - id: ds-anthropic
      base_url: "https://api.deepseek.com/anthropic"
      model: "deepseek-chat"
      api_key: "{ds}"
      protocol: anthropic
      timeout_secs: 90

content_rules:
  - id: install-test-secret
    enabled: true
    match_mode: full
    category: secret
    value: "{content_secret}"

file_rules:
  - id: install-secrets
    enabled: true
    path: "{secrets}"
    recursive: true
    trigger_window: 5
    match_mode: full
    formats: ["txt"]

path_protection_rules:
  - id: install-protected-secrets
    enabled: true
    path: "{secrets}"
    level: deny_access
  - id: install-protected-vault
    enabled: true
    path: "{vault}"
    level: deny_access

operation_rules:
  - id: block-rm-rf
    enabled: true
    operation: command_exec
    object:
      pattern: "rm -rf"
      is_regex: false
'''
Path("$TMP_CFG").write_text(cfg, encoding="utf-8")
print("Generated VM config")
PY

echo "==> Upload zip ($(du -h "$ZIP_FILE" | awk '{print $1}'))"
cat "$ZIP_FILE" | "$UTMCTL" file push "$VM_ID" "$ZIP_GUEST"

echo "==> Upload keys + config + test script"
cat "$TMP_KEYS" | "$UTMCTL" file push "$VM_ID" "$KEYS_GUEST"
cat "$TMP_CFG" | "$UTMCTL" file push "$VM_ID" "$CFG_GUEST"
cat "$PS1" | "$UTMCTL" file push "$VM_ID" "$PS1_GUEST"
rm -f "$TMP_KEYS" "$TMP_CFG"

echo "==> Run install + functional tests on guest (may take several minutes)..."
"$UTMCTL" exec "$VM_ID" --cmd powershell.exe -NoProfile -Command "Start-Process powershell.exe -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','${PS1_GUEST}' -WindowStyle Hidden" 2>/dev/null || true

DEADLINE=$((SECONDS + 900))
while (( SECONDS < DEADLINE )); do
  sleep 15
  "$UTMCTL" file pull "$VM_ID" "$LOG_GUEST" > "${ROOT}/dist/windows-utm-test.log" 2>/dev/null || true
  if grep -q "SUMMARY:" "${ROOT}/dist/windows-utm-test.log" 2>/dev/null; then
    break
  fi
  done_n=$(grep -cE '^\[[0-9:]{8}\] \[(PASS|FAIL)\]' "${ROOT}/dist/windows-utm-test.log" 2>/dev/null || echo 0)
  echo "... functional tests running (${SECONDS}s, checks=${done_n})"
done
cat "${ROOT}/dist/windows-utm-test.log" 2>/dev/null || echo "(no log pulled)"

FUNC_FAIL=0
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
