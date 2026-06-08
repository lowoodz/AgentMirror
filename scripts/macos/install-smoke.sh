#!/usr/bin/env bash
# Install CLI + app from dist/ tarballs (no cargo rebuild). Smoke health check.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=macos/common.sh
source "${ROOT}/scripts/macos/common.sh"

PREFIX="${SMR_INSTALL_PREFIX:-}"
LOG_PATH="${ROOT}/dist/macos-install-smoke.log"
BASE="http://127.0.0.1:8080"
CLI_ONLY=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --log) LOG_PATH="$2"; shift 2 ;;
    --base) BASE="$2"; shift 2 ;;
    --cli-only) CLI_ONLY=true ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

eval "$(smr_dist_paths)"
mkdir -p "$(dirname "$LOG_PATH")"
: > "$LOG_PATH"

log() {
  local line="[$(date '+%H:%M:%S')] $*"
  echo "$line" | tee -a "$LOG_PATH"
}

if [[ -z "$PREFIX" ]]; then
  PREFIX="$(mktemp -d "${TMPDIR:-/tmp}/smr-install-smoke.XXXXXX")"
fi

smr_stop_processes
bash "${ROOT}/scripts/uninstall.sh" --quiet 2>/dev/null || true
export SMR_INSTALL_PREFIX="$PREFIX"

[[ -f "$CLI_TAR" ]] || { log "ERROR: missing $CLI_TAR"; exit 1; }
if [[ "$CLI_ONLY" != true ]]; then
  [[ -f "$APP_TAR" ]] || { log "ERROR: missing $APP_TAR (use --cli-only for CLI-only smoke)"; exit 1; }
fi

stage="$(mktemp -d)"
tar -xzf "$CLI_TAR" -C "$stage"
install -d "${PREFIX}/bin" "${PREFIX}/etc/securemodelroute"
install -m 755 "${stage}/smr" "${PREFIX}/bin/smr"
if [[ ! -f "${PREFIX}/etc/securemodelroute/smr.yaml" ]]; then
  install -m 644 "${stage}/smr.example.yaml" "${PREFIX}/etc/securemodelroute/smr.yaml"
fi
rm -rf "$stage"

cfg="${PREFIX}/etc/securemodelroute/smr.yaml"
log "Installed CLI to ${PREFIX}/bin/smr"

if [[ "$CLI_ONLY" == true ]]; then
  HOME="$PREFIX" SMR_CONFIG="$cfg" "${PREFIX}/bin/smr" --config "$cfg" &
  run_pid=$!
  cleanup() {
    kill "$run_pid" 2>/dev/null || true
    wait "$run_pid" 2>/dev/null || true
    smr_stop_processes 0
  }
  trap cleanup EXIT
else
app_stage="$(mktemp -d)"
tar -xzf "$APP_TAR" -C "$app_stage"
app_bundle=""
for name in SafeRoute.app; do
  if [[ -d "${app_stage}/${name}" ]]; then
    app_bundle="${app_stage}/${name}"
    break
  fi
done
[[ -n "$app_bundle" ]] || { log "ERROR: no .app in $APP_TAR"; exit 1; }

apps_dir="${PREFIX}/Applications"
install -d "$apps_dir"
rm -rf "${apps_dir}/SafeRoute.app" "${apps_dir}/SecureModelRoute.app"
cp -R "$app_bundle" "${apps_dir}/$(basename "$app_bundle")"
gui_bin="${apps_dir}/$(basename "$app_bundle")/Contents/MacOS/smr-gui"

log "Installed app to ${apps_dir}/$(basename "$app_bundle")"

HOME="$PREFIX" SMR_CONFIG="$cfg" "$gui_bin" --background &
run_pid=$!
cleanup() {
  kill "$run_pid" 2>/dev/null || true
  wait "$run_pid" 2>/dev/null || true
  smr_stop_processes 0
}
trap cleanup EXIT
fi

ok=0
for _ in $(seq 1 60); do
  if smr_curl_health_ok "${BASE}"; then ok=1; break; fi
  kill -0 "$run_pid" 2>/dev/null || { log "ERROR: server process exited early"; exit 1; }
  sleep 1
done
[[ "$ok" -eq 1 ]] || { log "ERROR: health check failed on ${BASE}"; exit 1; }

ui_ok=0
for _ in $(seq 1 30); do
  if smr_curl_ui_ok "${BASE}"; then ui_ok=1; break; fi
  sleep 1
done
[[ "$ui_ok" -eq 1 ]] || { log "ERROR: UI check failed on ${BASE}/ui"; exit 1; }
log "INSTALL SMOKE PASSED (prefix=${PREFIX})"
exit 0
