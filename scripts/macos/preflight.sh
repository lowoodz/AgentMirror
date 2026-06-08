#!/usr/bin/env bash
# Preflight checks before macOS package / install / test.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=macos/common.sh
source "${ROOT}/scripts/macos/common.sh"

REQUIRE_NODE=false
REQUIRE_PYTHON=false
QUIET=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --require-node) REQUIRE_NODE=true ;;
    --require-python) REQUIRE_PYTHON=true ;;
    --quiet) QUIET=true ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

failures=()

ok() {
  local cond="$1" msg="$2"
  if [[ "$QUIET" != true ]]; then
    if [[ "$cond" == 1 ]]; then echo "[OK] $msg"; else echo "[FAIL] $msg"; fi
  fi
  [[ "$cond" == 1 ]] || failures+=("$msg")
}

[[ "$(uname -s)" == "Darwin" ]] && ok 1 "host: macOS ($(uname -m))" || ok 0 "host must be macOS"

smr_set_build_env
command -v cargo >/dev/null && ok 1 "cargo: $(cargo --version)" || ok 0 "cargo not in PATH (~/.cargo/bin)"

if [[ "$REQUIRE_NODE" == true ]] || [[ -f "${ROOT}/gui/package.json" ]]; then
  command -v npm >/dev/null && ok 1 "npm: v$(npm --version 2>/dev/null)" || ok 0 "npm required for GUI/DMG build"
fi

if [[ "$REQUIRE_PYTHON" == true ]]; then
  command -v python3 >/dev/null && ok 1 "python3: $(python3 --version 2>&1)" || ok 0 "python3 required for blackbox/functional tests"
fi

expected="${ROOT}/target"
if [[ -n "${CARGO_TARGET_DIR:-}" && "${CARGO_TARGET_DIR}" != "$expected" ]]; then
  ok 0 "CARGO_TARGET_DIR=${CARGO_TARGET_DIR} (expected ${expected} — stale binary risk)"
else
  ok 1 "CARGO_TARGET_DIR=${expected}"
fi

if [[ ${#failures[@]} -gt 0 ]]; then
  echo "" >&2
  echo "Preflight failed:" >&2
  printf ' - %s\n' "${failures[@]}" >&2
  exit 1
fi

[[ "$QUIET" != true ]] && echo "" && echo "Preflight passed."
exit 0
