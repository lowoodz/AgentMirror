#!/usr/bin/env bash
# Full Windows VM test suite via windows-user SSH: functional + NSIS + blackbox/stress.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

vm_ssh_require

stop_guest_smr() {
  echo "==> Stop guest SMR / free :8080"
  bash "${ROOT}/scripts/vm/stop-guest-smr.sh" || {
    echo "WARNING: stop-guest-smr failed; continuing anyway" >&2
  }
}

stop_guest_smr

echo "========== Phase 1: Functional install test ($VM_SSH) =========="
FUNC_OK=0
if bash "${ROOT}/scripts/vm/utm-run-test.sh"; then
  FUNC_OK=1
else
  echo "Functional test did not fully pass; continuing with blackbox/stress..." >&2
fi

stop_guest_smr

echo ""
echo "========== Phase 1b: NSIS install/uninstall smoke =========="
NSIS_OK=1
if compgen -G "${ROOT}/dist/SafeRoute_*_x64-setup.exe" > /dev/null || compgen -G "${ROOT}/dist/windows-desktop/*-setup.exe" > /dev/null; then
  NSIS_OK=0
  if bash "${ROOT}/scripts/vm/utm-run-nsis-install-test.sh"; then
    NSIS_OK=1
  else
    echo "NSIS install test did not pass; continuing..." >&2
  fi
else
  echo "Skip NSIS test (no *-setup.exe in dist/)" >&2
fi

stop_guest_smr

echo ""
echo "========== Phase 2–3: Transparency + blackbox + stress =========="
PY_OK=0
if bash "${ROOT}/scripts/vm/utm-run-python-tests.sh"; then
  PY_OK=1
fi

if [[ "$FUNC_OK" -eq 1 && "$NSIS_OK" -eq 1 && "$PY_OK" -eq 1 ]]; then
  echo ""
  echo "========== Phase 4: OpenClaw strict matrix (12 cases) =========="
  OPENCLAW_OK=0
  if bash "${ROOT}/scripts/vm/run-openclaw-matrix.sh" --skip-install; then
    OPENCLAW_OK=1
  else
    echo "OpenClaw matrix did not pass; see dist/windows-openclaw-matrix.log" >&2
  fi
else
  OPENCLAW_OK=0
  echo "Skip OpenClaw matrix (prior VM phases failed)" >&2
fi

if [[ "$FUNC_OK" -eq 1 && "$NSIS_OK" -eq 1 && "$PY_OK" -eq 1 && "$OPENCLAW_OK" -eq 1 ]]; then
  echo ""
  echo "========== ALL WINDOWS VM TESTS PASSED =========="
  exit 0
fi

echo ""
echo "========== SOME TESTS FAILED ==========" >&2
echo "  functional: dist/windows-utm-test.log" >&2
echo "  nsis: dist/windows-nsis-install-test.log" >&2
echo "  blackbox/stress/transparency: dist/windows-utm-python-test.log" >&2
echo "  openclaw: dist/windows-openclaw-matrix.log" >&2
exit 1
