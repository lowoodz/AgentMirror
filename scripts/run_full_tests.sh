#!/usr/bin/env bash
# Full test matrix: macOS host + optional Windows UTM VM.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
cd "$ROOT"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

LOG_DIR="${ROOT}/dist/test-runs"
mkdir -p "$LOG_DIR"
STAMP="$(date +%Y%m%d-%H%M%S)"
SUMMARY="${LOG_DIR}/full-${STAMP}.log"

failures=0

run_step() {
  local name="$1"
  shift
  echo ""
  echo "################################################################"
  echo "# ${name}"
  echo "################################################################"
  set +e
  "$@" 2>&1 | tee "${LOG_DIR}/${STAMP}-${name// /-}.log"
  local rc=${PIPESTATUS[0]}
  set -e
  if [[ "$rc" -eq 0 ]]; then
    echo ">>> ${name}: PASSED" | tee -a "$SUMMARY"
  else
    echo ">>> ${name}: FAILED" | tee -a "$SUMMARY"
    failures=$((failures + 1))
  fi
}

echo "Full test run started: $(date)" | tee "$SUMMARY"

run_step "1-verify" bash scripts/verify.sh
run_step "2-transparency" python3 scripts/transparency_pass_through_test.py --release

if ! has_test_keys; then
  echo ">>> SKIP live API tests: copy config/test.env.example to config/test.env and set API keys" | tee -a "$SUMMARY"
  exit 1
fi

run_step "3-install-functional" python3 scripts/install_functional_test.py
run_step "4-blackbox" python3 scripts/blackbox_test.py
run_step "5-stress" python3 scripts/live_test.py

# shellcheck source=vm/vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"
vm_ssh_init
if [[ "${SMR_SKIP_VM_TESTS:-0}" != "1" ]] && ssh "${VM_SSH_OPTS[@]}" "$VM_SSH" "echo ok" >/dev/null 2>&1; then
  if ls dist/smr-*-windows-x86_64.zip >/dev/null 2>&1; then
    run_step "5-windows-utm" bash scripts/vm/utm-run-all-tests.sh
  else
    echo ">>> SKIP VM tests: no dist/smr-*-windows-x86_64.zip (run package-windows.sh)" | tee -a "$SUMMARY"
  fi
else
  echo ">>> SKIP VM tests: SSH to ${VM_SSH:-${SMR_WINDOWS_USER:-}@${SMR_WINDOWS_HOST:-windows-vm}} unavailable" | tee -a "$SUMMARY"
fi

echo ""
echo "================================================================"
if [[ "$failures" -eq 0 ]]; then
  echo "ALL FULL TESTS PASSED"
  echo "Logs: ${LOG_DIR}/${STAMP}-*.log"
  exit 0
fi

echo "${failures} test stage(s) FAILED — see ${LOG_DIR}/${STAMP}-*.log"
exit 1
