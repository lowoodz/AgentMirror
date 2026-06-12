#!/usr/bin/env bash
# Deploy portable matrix smr.yaml, reload SafeRoute, run strict OpenClaw E2E matrix, restore config.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"

BASE="${SMR_BASE:-http://127.0.0.1:8080}"
KEEP_MATRIX_CONFIG=false
LOG="${ROOT}/dist/openclaw-matrix-test.log"
SMR_BIN="${SMR_BIN:-${ROOT}/target/release/smr}"
SMR_PROC=""

usage() {
  cat <<'EOF'
Usage: scripts/run_openclaw_matrix.sh [options]

Strict E2E: openclaw agent -> SafeRoute -> upstream LLM (high group from test keys).
No HTTP replay fallbacks unless --allow-replay is passed to the Python test.

Options:
  --matrix-root PATH         Fixture + config root (default: SMR_MATRIX_ROOT or temp)
  --keep-matrix-config       Do not restore smr.yaml after the run
  --no-deploy                Skip config deploy (fixtures + env file only)
  --allow-replay             Legacy mode: allow HTTP replay when OpenClaw skips exec
  --no-restart-smr           Do not stop :8080 / restart SMR_BIN before deploy
  --log PATH                 Log file (default: dist/openclaw-matrix-test.log)

Windows VM: scripts/vm/run-openclaw-matrix.sh
EOF
}

NO_DEPLOY=false
MATRIX_ROOT_ARG=""
ALLOW_REPLAY=false
RESTART_SMR=true
while [[ $# -gt 0 ]]; do
  case "$1" in
    --matrix-root) MATRIX_ROOT_ARG="$2"; shift 2 ;;
    --keep-matrix-config) KEEP_MATRIX_CONFIG=true; shift ;;
    --no-deploy) NO_DEPLOY=true; shift ;;
    --allow-replay) ALLOW_REPLAY=true; shift ;;
    --no-restart-smr) RESTART_SMR=false; shift ;;
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
  restore_openclaw
  if [[ -f "$BACKUP" ]]; then
    cp "$BACKUP" "$CFG"
    rm -f "$BACKUP"
    if [[ -n "$SMR_PROC" ]] && kill -0 "$SMR_PROC" 2>/dev/null; then
      curl -sf -X PUT "${BASE}/api/reload" >/dev/null || true
      "${ROOT}/scripts/wait-file-index-ready.sh" "$BASE" 180 || true
    fi
    echo "==> Restored smr.yaml from backup"
  fi
  if [[ -n "$SMR_PROC" ]] && kill -0 "$SMR_PROC" 2>/dev/null; then
    kill "$SMR_PROC" 2>/dev/null || true
    wait "$SMR_PROC" 2>/dev/null || true
    SMR_PROC=""
  fi
}

stop_listeners_8080() {
  if command -v lsof >/dev/null 2>&1; then
    local pids
    pids="$(lsof -ti tcp:8080 -sTCP:LISTEN 2>/dev/null || true)"
    if [[ -n "$pids" ]]; then
      echo "==> Stopping listeners on :8080 ($pids)"
      # shellcheck disable=SC2086
      kill $pids 2>/dev/null || true
      sleep 2
    fi
  fi
}

ensure_smr() {
  if [[ "$RESTART_SMR" == true ]]; then
    stop_listeners_8080
  elif curl -sf "${BASE}/health" >/dev/null 2>&1; then
    return 0
  fi
  if [[ ! -x "$SMR_BIN" ]]; then
    echo "SafeRoute not on ${BASE} and missing ${SMR_BIN}; run: cargo build --release -p smr-cli" >&2
    return 1
  fi
  echo "==> Starting SafeRoute CLI: ${SMR_BIN} --config ${CFG}"
  "$SMR_BIN" --config "$CFG" >>"${LOG}.smr" 2>&1 &
  SMR_PROC=$!
  "${ROOT}/scripts/wait-file-index-ready.sh" "$BASE" 180
}

OPENCLAW_BACKUP=""
restore_openclaw() {
  if [[ -n "$OPENCLAW_BACKUP" && -f "$OPENCLAW_BACKUP" ]]; then
    python3 "${ROOT}/scripts/patch_openclaw_saferoute.py" --restore "$OPENCLAW_BACKUP" || true
    rm -f "$OPENCLAW_BACKUP"
    echo "==> Restored openclaw.json from backup"
  fi
}

trap restore_config EXIT

patch_openclaw_for_matrix() {
  OPENCLAW_BACKUP="$(python3 "${ROOT}/scripts/patch_openclaw_saferoute.py" 2>/dev/null | tail -1 || true)"
}

patch_openclaw_for_matrix

# Do not inherit stale matrix paths from the shell environment.
unset SMR_MATRIX_ROOT SMR_MATRIX_DLP_DIR SMR_MATRIX_DLP_SECRET \
  SMR_MATRIX_DLP_SSH_PUB SMR_MATRIX_DLP_OUT_SSH SMR_MATRIX_DLP_OUT_CONTENT \
  SMR_MATRIX_CONTENT_SECRET SMR_MATRIX_SSH_NEEDLE \
  SMR_MATRIX_PATH_DENY_ACCESS SMR_MATRIX_PATH_DENY_MODIFY \
  SMR_MATRIX_PATH_DENY_DELETE SMR_MATRIX_PATH_OPEN SMR_MATRIX_OPS_TMP \
  SMR_MATRIX_DLP_CANARY SMR_MATRIX_PLATFORM

python3 "${ROOT}/scripts/generate_openclaw_matrix_config.py" "${GEN_ARGS[@]}"

if [[ "$NO_DEPLOY" == false ]]; then
  cp "$CFG" "$BACKUP" 2>/dev/null || true
  cp "$MATRIX_CFG" "$CFG"
  echo "==> Deployed matrix smr.yaml to ${CFG}"
  ensure_smr
  curl -sf -X PUT "${BASE}/api/reload" >/dev/null
  "${ROOT}/scripts/wait-file-index-ready.sh" "$BASE" 180
fi

set +e
# shellcheck disable=SC1090
set -a && source "$ENV_FILE" && set +a
PY_ARGS=(--env-file "$ENV_FILE")
[[ "$ALLOW_REPLAY" == true ]] && PY_ARGS+=(--allow-replay)
python3 "${ROOT}/scripts/openclaw_security_matrix_test.py" "${PY_ARGS[@]}" \
  2>&1 | tee "$LOG"
RC=${PIPESTATUS[0]}
set -e

if [[ "$KEEP_MATRIX_CONFIG" == true ]]; then
  restore_openclaw
  trap - EXIT
  echo "==> Keeping matrix smr.yaml (--keep-matrix-config)"
else
  restore_config
  trap - EXIT
fi

exit "$RC"
