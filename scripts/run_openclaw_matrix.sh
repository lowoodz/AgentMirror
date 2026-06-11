#!/usr/bin/env bash
# Deploy portable matrix smr.yaml, reload SafeRoute, run OpenClaw security matrix, restore config.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"

BASE="${SMR_BASE:-http://127.0.0.1:8080}"
KEEP_MATRIX_CONFIG=false
LOG="${ROOT}/dist/openclaw-matrix-test.log"

usage() {
  cat <<'EOF'
Usage: scripts/run_openclaw_matrix.sh [options]

Portable matrix test (same fixture layout on macOS / Linux / Windows).
Paths: SMR_MATRIX_ROOT or system temp (see config/test.env.example).

Options:
  --matrix-root PATH         Fixture + config root (default: SMR_MATRIX_ROOT or temp)
  --keep-matrix-config       Do not restore smr.yaml after the run
  --no-deploy                Skip config deploy (fixtures + env file only)
  --log PATH                 Log file (default: dist/openclaw-matrix-test.log)

Windows VM: scripts/vm/run-openclaw-matrix.sh
EOF
}

NO_DEPLOY=false
MATRIX_ROOT_ARG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --matrix-root) MATRIX_ROOT_ARG="$2"; shift 2 ;;
    --keep-matrix-config) KEEP_MATRIX_CONFIG=true; shift ;;
    --no-deploy) NO_DEPLOY=true; shift ;;
    --log) LOG="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 2 ;;
  esac
done

if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* || "$(uname -s)" == CYGWIN* ]]; then
  echo "On native Windows, run scripts/vm/run-openclaw-matrix-remote.ps1 or vm/run-openclaw-matrix.sh from Mac host." >&2
fi

CFG_DIR="$(python3 -c "import sys; sys.path.insert(0,'${ROOT}/scripts'); from openclaw_matrix_common import smr_config_dir; print(smr_config_dir())")"
CFG="${CFG_DIR}/smr.yaml"
BACKUP="${CFG_DIR}/smr.yaml.matrix-backup"
WORK="${ROOT}/dist/openclaw-matrix"
ENV_FILE="${WORK}/macos.env"
MATRIX_CFG="${WORK}/smr.yaml"

mkdir -p "$WORK" "$CFG_DIR"
rm -f "$LOG"

GEN_ARGS=(--output "$MATRIX_CFG" --env-file "$ENV_FILE" --fixtures)
[[ -n "$MATRIX_ROOT_ARG" ]] && GEN_ARGS+=(--matrix-root "$MATRIX_ROOT_ARG")

restore_config() {
  if [[ -f "$BACKUP" ]]; then
    cp "$BACKUP" "$CFG"
    rm -f "$BACKUP"
    curl -sf -X PUT "${BASE}/api/reload" >/dev/null || true
    "${ROOT}/scripts/wait-file-index-ready.sh" "$BASE" 180 || true
    echo "==> Restored smr.yaml from backup"
  fi
}

trap restore_config EXIT

# Do not inherit stale matrix paths from the shell environment.
unset SMR_MATRIX_ROOT SMR_MATRIX_DLP_DIR SMR_MATRIX_DLP_SECRET \
  SMR_MATRIX_PATH_DENY_ACCESS SMR_MATRIX_PATH_DENY_MODIFY \
  SMR_MATRIX_PATH_DENY_DELETE SMR_MATRIX_PATH_OPEN SMR_MATRIX_OPS_TMP \
  SMR_MATRIX_DLP_CANARY SMR_MATRIX_PLATFORM

python3 "${ROOT}/scripts/generate_openclaw_matrix_config.py" "${GEN_ARGS[@]}"

if [[ "$NO_DEPLOY" == false ]]; then
  cp "$CFG" "$BACKUP" 2>/dev/null || true
  cp "$MATRIX_CFG" "$CFG"
  echo "==> Deployed matrix smr.yaml to ${CFG}"
  curl -sf -X PUT "${BASE}/api/reload" >/dev/null
  "${ROOT}/scripts/wait-file-index-ready.sh" "$BASE" 180
fi

set +e
# shellcheck disable=SC1090
set -a && source "$ENV_FILE" && set +a
python3 "${ROOT}/scripts/openclaw_security_matrix_test.py" --env-file "$ENV_FILE" \
  2>&1 | tee "$LOG"
RC=${PIPESTATUS[0]}
set -e

if [[ "$KEEP_MATRIX_CONFIG" == true ]]; then
  trap - EXIT
  echo "==> Keeping matrix smr.yaml (--keep-matrix-config)"
else
  restore_config
  trap - EXIT
fi

exit "$RC"
