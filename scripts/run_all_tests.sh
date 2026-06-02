#!/usr/bin/env bash
# Run unit tests, smoke verify, install functional, black-box, and stress tests.
# Host matrix (macOS/Linux): verify.sh → install_functional_test.py → blackbox_test.py → live_test.py
# Windows host: scripts/run_all_tests.ps1 (same four Python stages + verify.ps1)
# Windows UTM: utm-run-all-tests.sh → windows-utm-full-test.ps1 + guest blackbox/stress
# Installed-app: run_installed_app_tests.sh → macOS tray + Windows UTM attach blackbox (27 scenarios)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
cd "$ROOT"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

echo "========== 1/5 Unit + smoke (verify.sh) =========="
bash scripts/verify.sh

if ! has_test_keys; then
  echo "Skip live tests: copy config/test.env.example to config/test.env and set SMR_GLM_API_KEY / SMR_DEEPSEEK_API_KEY"
  exit 0
fi

echo ""
echo "========== 2/5 Install functional smoke =========="
python3 scripts/install_functional_test.py

echo ""
echo "========== 3/5 Black-box scenarios =========="
python3 scripts/blackbox_test.py

echo ""
echo "========== 4/5 Stress tests =========="
python3 scripts/live_test.py

echo ""
echo "========== All host test suites passed =========="
