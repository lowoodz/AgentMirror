#!/usr/bin/env bash
# Full Windows VM test suite via windows-user SSH: functional + NSIS + blackbox/stress.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

vm_ssh_require

echo "========== Phase 1: Functional install test ($VM_SSH) =========="
FUNC_OK=0
if bash "${ROOT}/scripts/vm/utm-run-test.sh"; then
  FUNC_OK=1
else
  echo "Functional test did not fully pass; continuing with blackbox/stress..." >&2
fi

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

echo ""
echo "========== Phase 2–3: Blackbox + stress =========="
PY_OK=0
if bash "${ROOT}/scripts/vm/utm-run-python-tests.sh"; then
  PY_OK=1
fi

if [[ "$FUNC_OK" -eq 1 && "$NSIS_OK" -eq 1 && "$PY_OK" -eq 1 ]]; then
  echo ""
  echo "========== ALL WINDOWS VM TESTS PASSED =========="
  exit 0
fi

echo ""
echo "========== SOME TESTS FAILED ==========" >&2
echo "  functional: dist/windows-utm-test.log" >&2
echo "  nsis: dist/windows-nsis-install-test.log" >&2
echo "  blackbox/stress: dist/windows-utm-python-test.log" >&2
exit 1
