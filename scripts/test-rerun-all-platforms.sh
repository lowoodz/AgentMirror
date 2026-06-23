#!/usr/bin/env bash
# Re-run whitebox (verify), blackbox, OpenClaw matrix, and Claude Code transparency on macOS + Windows VM.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

LOG="${ROOT}/dist/test-rerun-all-platforms.log"
mkdir -p "${ROOT}/dist/test-runs"
: > "$LOG"

FAIL=0
PASSED=0

log() {
  local line="[$(date '+%Y-%m-%d %H:%M:%S')] $*"
  echo "$line" | tee -a "$LOG"
}

run_step() {
  local name="$1"
  shift
  log "========== ${name} =========="
  if "$@" >>"$LOG" 2>&1; then
    log "PASSED: ${name}"
    PASSED=$((PASSED + 1))
  else
    log "FAILED: ${name}"
    FAIL=$((FAIL + 1))
  fi
}

log "test-rerun-all-platforms root=${ROOT}"

if ! has_test_keys; then
  log "ERROR: missing API keys — set config/test.env"
  exit 1
fi

# --- macOS ---
run_step "macOS whitebox (verify.sh)" bash "${ROOT}/scripts/verify.sh"
run_step "macOS blackbox" python3 "${ROOT}/scripts/blackbox_test.py"

if has_openclaw; then
  run_step "macOS OpenClaw matrix (12 cases)" \
    bash "${ROOT}/scripts/run_openclaw_matrix.sh" \
    --log "${ROOT}/dist/openclaw-matrix-macos.log"
else
  log "SKIP macOS OpenClaw matrix: openclaw not in PATH"
  FAIL=$((FAIL + 1))
fi

FIXTURE="${ROOT}/dist/transparency-count-fixture"
mkdir -p "$FIXTURE"
touch "${FIXTURE}/a.txt" "${FIXTURE}/b.txt"
run_step "macOS transparency (OpenClaw + Claude Code live)" \
  env SMR_TRANSPARENCY_COUNT_DIR="${FIXTURE}" \
  bash "${ROOT}/scripts/run_transparency_client_live.sh" --client-only

# --- Windows VM ---
# shellcheck source=vm/vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"
if ! vm_ssh_require 2>/dev/null; then
  log "ERROR: Windows VM SSH unavailable (${VM_SSH:-windows-vm})"
  exit 1
fi

bash "${ROOT}/scripts/vm/stop-guest-smr.sh" >>"$LOG" 2>&1 || true
run_step "Windows functional install (prereq for guest blackbox)" \
  bash "${ROOT}/scripts/vm/utm-run-test.sh"
bash "${ROOT}/scripts/vm/stop-guest-smr.sh" >>"$LOG" 2>&1 || true
run_step "Windows transparency + blackbox + stress" \
  bash "${ROOT}/scripts/vm/utm-run-python-tests.sh"
run_step "Windows OpenClaw matrix (12 cases)" \
  bash "${ROOT}/scripts/vm/run-openclaw-matrix.sh" --skip-install
run_step "Windows transparency (OpenClaw + Claude Code live)" \
  bash "${ROOT}/scripts/vm/run-transparency-client-live.sh"

log ""
log "========== SUMMARY =========="
log "Passed steps: ${PASSED}"
log "Failed steps: ${FAIL}"
log "Full log: ${LOG}"

if [[ "$FAIL" -eq 0 ]]; then
  log "ALL PLATFORMS PASSED"
  exit 0
fi
log "SOME STEPS FAILED"
exit 1
