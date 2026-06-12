#!/usr/bin/env bash
# Run unit tests, smoke verify, transparency pass-through, install functional, black-box, and stress tests.
# Host matrix (macOS/Linux): verify.sh → transparency → install_functional → blackbox → live_test
# Windows host: scripts/run_all_tests.ps1 (same stages + verify.ps1)
# Windows UTM: utm-run-all-tests.sh → guest transparency + blackbox/stress
# Installed-app: run_installed_app_tests.sh → macOS tray + Windows UTM attach blackbox (27 scenarios)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
cd "$ROOT"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

echo "========== 1/6 Unit + smoke (verify.sh) =========="
bash scripts/verify.sh

echo ""
echo "========== 2/6 Transparency pass-through (mock) =========="
python3 scripts/transparency_pass_through_test.py --release

if ! has_test_keys; then
  echo "Skip live API tests: copy config/test.env.example to config/test.env and set SMR_GLM_API_KEY / SMR_DEEPSEEK_API_KEY"
  exit 0
fi

echo ""
echo "========== 3/6 Install functional smoke =========="
python3 scripts/install_functional_test.py

echo ""
echo "========== 4/6 Black-box scenarios =========="
python3 scripts/blackbox_test.py

echo ""
echo "========== 5/6 Stress tests =========="
python3 scripts/live_test.py

echo ""
echo "========== All host test suites passed =========="
