#!/usr/bin/env bash
# Live OpenClaw + Claude transparency (direct vs SafeRoute). Requires config/test.env keys.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"

if ! has_test_keys; then
  echo "Missing keys — copy config/test.env.example to config/test.env" >&2
  exit 1
fi

# TODO default paths: macOS repo root; override via SMR_TRANSPARENCY_COUNT_DIR in test.env
export SMR_TRANSPARENCY_COUNT_DIR="${SMR_TRANSPARENCY_COUNT_DIR:-${ROOT}}"
export PYTHONUNBUFFERED=1

LOG="${ROOT}/dist/transparency-client-live-macos.log"
mkdir -p "${ROOT}/dist"
exec > >(tee "$LOG") 2>&1

echo "==> HTTP wire transparency (mock upstream, SSE + JSON)"
python3 "${ROOT}/scripts/transparency_pass_through_test.py" --release

echo ""
echo "==> Client live E2E (openclaw + claude, direct vs SafeRoute)"
python3 "${ROOT}/scripts/transparency_client_live_test.py" "$@"
