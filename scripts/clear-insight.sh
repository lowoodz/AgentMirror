#!/usr/bin/env bash
# Clear AgentMirror derived data (runs, events, graphs, reports).
# Does NOT delete traffic snapshots or audits table rows.
#
# Usage:
#   ./scripts/clear-insight.sh              # clear only (offline or via API)
#   ./scripts/clear-insight.sh --replay     # clear + rebuild from saved traffic
#   ./scripts/clear-insight.sh --offline    # force SQLite/files (SMR must be stopped)
#   SMR_BASE=http://127.0.0.1:8080 ./scripts/clear-insight.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPLAY=false
OFFLINE=false
BASE="${SMR_BASE:-http://127.0.0.1:8080}"
LIMIT="${SMR_INSIGHT_REPLAY_LIMIT:-5000}"

usage() {
  sed -n '2,9p' "$0"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --replay) REPLAY=true; shift ;;
    --offline) OFFLINE=true; shift ;;
    --base) BASE="$2"; shift 2 ;;
    --limit) LIMIT="$2"; shift 2 ;;
    *) echo "Unknown option: $1" >&2; usage 1 ;;
  esac
done

resolve_config_dir() {
  if [[ -n "${SMR_CONFIG_DIR:-}" ]]; then
    echo "${SMR_CONFIG_DIR}"
    return
  fi
  if [[ "$(uname -s)" == "Darwin" ]] && [[ -d "${HOME}/Library/Application Support/securemodelroute" ]]; then
    echo "${HOME}/Library/Application Support/securemodelroute"
  elif [[ -d "${HOME}/.config/securemodelroute" ]]; then
    echo "${HOME}/.config/securemodelroute"
  else
    echo "${HOME}/.config/securemodelroute"
  fi
}

clear_offline() {
  local cfg_dir db graphs daily
  cfg_dir="$(resolve_config_dir)"
  db="${cfg_dir}/data/smr.db"
  graphs="${cfg_dir}/data/insight/graphs"
  daily="${cfg_dir}/data/insight/daily"

  if [[ ! -f "${db}" ]]; then
    echo "No insight database at ${db}; nothing to clear." >&2
    exit 0
  fi

  if [[ "${REPLAY}" == true ]]; then
    echo "Replay requires a running LLM-SafeRoute instance (traffic + pipeline)." >&2
    echo "Use: ${BASE} with --replay and without --offline." >&2
    exit 1
  fi

  echo "Clearing AgentMirror offline (keeping audits + traffic)…"
  echo "  DB: ${db}"

  sqlite3 "${db}" <<'SQL'
DELETE FROM insight_events;
DELETE FROM insight_reports;
DELETE FROM insight_daily_reports;
DELETE FROM insight_processed_audits;
DELETE FROM insight_runs;
DELETE FROM insight_agents;
SQL

  if [[ -d "${graphs}" ]]; then
    rm -f "${graphs}"/*.json 2>/dev/null || true
  fi
  if [[ -d "${daily}" ]]; then
    rm -f "${daily}"/*.md 2>/dev/null || true
  fi

  echo "Done. Traffic dir untouched: ${cfg_dir}/traffic"
}

clear_via_api() {
  local payload
  if [[ "${REPLAY}" == true ]]; then
    payload="{\"replay_from_traffic\":true,\"limit\":${LIMIT}}"
    echo "Resetting AgentMirror and replaying up to ${LIMIT} audits from traffic…"
  else
    payload='{"replay_from_traffic":false}'
    echo "Resetting AgentMirror via ${BASE}/api/insight/reset …"
  fi
  curl -sfS -X POST "${BASE}/api/insight/reset" \
    -H 'Content-Type: application/json' \
    -d "${payload}"
  echo
}

if [[ "${OFFLINE}" == true ]]; then
  clear_offline
  exit 0
fi

if curl -sfS --max-time 2 "${BASE}/api/status" >/dev/null 2>&1; then
  clear_via_api
else
  echo "LLM-SafeRoute not reachable at ${BASE}; using offline clear." >&2
  clear_offline
fi
