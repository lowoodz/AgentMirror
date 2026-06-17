#!/usr/bin/env bash
# Post-install AgentMirror V2 test: install from dist/, launch with mock upstream, run API checks.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=macos/common.sh
source "${ROOT}/scripts/macos/common.sh"

BASE="${SMR_INSIGHT_TEST_BASE:-http://127.0.0.1:8080}"
PREFIX="${SMR_INSTALL_PREFIX:-$(mktemp -d "${TMPDIR:-/tmp}/smr-insight-installed.XXXXXX")}"
KEEP_PREFIX="${SMR_KEEP_INSTALL_PREFIX:-false}"
RUN_PID=""
MOCK_PID=""

cleanup() {
  if [[ -n "$RUN_PID" ]]; then
    kill "$RUN_PID" 2>/dev/null || true
    wait "$RUN_PID" 2>/dev/null || true
  fi
  if [[ -n "$MOCK_PID" ]]; then
    kill "$MOCK_PID" 2>/dev/null || true
  fi
  smr_stop_processes 0 2>/dev/null || true
  if [[ "$KEEP_PREFIX" != true ]]; then
    rm -rf "$PREFIX"
  fi
}
trap cleanup EXIT

eval "$(smr_dist_paths)"
smr_stop_processes
bash "${ROOT}/scripts/uninstall.sh" --quiet 2>/dev/null || true

stage="$(mktemp -d)"
tar -xzf "$CLI_TAR" -C "$stage"
install -d "${PREFIX}/bin" "${PREFIX}/etc/securemodelroute"
install -m 755 "${stage}/smr" "${PREFIX}/bin/smr"
[[ -d "${stage}/tools" ]] && cp -R "${stage}/tools" "${PREFIX}/bin/tools"
cfg="${PREFIX}/etc/securemodelroute/smr.yaml"
rm -rf "$stage"

app_stage="$(mktemp -d)"
tar -xzf "$APP_TAR" -C "$app_stage"
app_bundle="${app_stage}/AgentMirror.app"
[[ -d "$app_bundle" ]] || app_bundle="${app_stage}/SafeRoute.app"
[[ -d "$app_bundle" ]] || { echo "missing AgentMirror.app in $APP_TAR" >&2; exit 1; }
install -d "${PREFIX}/Applications"
cp -R "$app_bundle" "${PREFIX}/Applications/$(basename "$app_bundle")"
gui_bin="${PREFIX}/Applications/$(basename "$app_bundle")/Contents/MacOS/smr-gui"
rm -rf "$app_stage"

# Mock upstream + insight-enabled config (no live API keys required)
MOCK_PORT=$(python3 -c "import socket; s=socket.socket(); s.bind(('127.0.0.1',0)); print(s.getsockname()[1]); s.close()")
python3 "${ROOT}/scripts/insight_mock_server.py" --port "$MOCK_PORT" &
MOCK_PID=$!
sleep 0.3

cat > "$cfg" <<YAML
server:
  listen: "127.0.0.1:8080"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: observe

logging:
  level: info
  redact_content: false
  save_traffic_bodies: true

insight:
  enabled: true
  require_traffic_bodies: true
  llm_critic: false

fallback_groups:
  high:
    - id: mock-primary
      base_url: "http://127.0.0.1:${MOCK_PORT}/v1"
      model: "mock-model"
      api_key: "mock-key"
      protocol: openai
      timeout_secs: 30

content_rules:
  - id: insight-test-secret
    enabled: true
    match_mode: full
    category: secret
    value: "INSIGHT-MIRROR-TEST-SECRET-XYZZY"
YAML

echo "==> Installed to prefix=${PREFIX} (mock upstream :${MOCK_PORT})"
HOME="$PREFIX" SMR_CONFIG="$cfg" "$gui_bin" --background &
RUN_PID=$!

ok=0
for _ in $(seq 1 60); do
  if smr_curl_health_ok "$BASE"; then ok=1; break; fi
  kill -0 "$RUN_PID" 2>/dev/null || { echo "server exited early" >&2; exit 1; }
  sleep 1
done
[[ "$ok" -eq 1 ]] || { echo "health timeout on $BASE" >&2; exit 1; }

echo "==> Running AgentMirror installed API test"
python3 "${ROOT}/scripts/insight_mirror_test.py" --installed --base "$BASE"
echo "==> AgentMirror installed test PASSED"
