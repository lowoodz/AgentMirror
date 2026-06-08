#!/usr/bin/env bash
# Clean uninstall SafeRoute on macOS (CLI, GUI, LaunchAgents).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=macos/common.sh
source "${ROOT}/scripts/macos/common.sh"

KEEP_CONFIG=false
QUIET=false

for arg in "$@"; do
  case "$arg" in
    --keep-config) KEEP_CONFIG=true ;;
    --quiet) QUIET=true ;;
  esac
done

log() {
  if [[ "$QUIET" != true ]]; then echo "$@"; fi
}

PREFIX="${SMR_INSTALL_PREFIX:-${HOME}/.local}"
BINDIR="${PREFIX}/bin"
CONFDIR="${PREFIX}/etc/securemodelroute"

smr_stop_processes 1

log "==> Removing CLI install under ${PREFIX}"
for f in smr securemodelroute; do
  [[ -f "${BINDIR}/${f}" ]] && rm -f "${BINDIR}/${f}" && log "    removed ${BINDIR}/${f}"
done

log "==> Removing desktop apps"
for app in SafeRoute.app SecureModelRoute.app; do
  for dir in "${HOME}/Applications/${app}" "/Applications/${app}"; do
    if [[ -d "$dir" ]]; then
      rm -rf "$dir"
      log "    removed $dir"
    fi
  done
done

log "==> Removing LaunchAgents"
for label in com.securemodelroute.smr com.securemodelroute.gui; do
  plist="${HOME}/Library/LaunchAgents/${label}.plist"
  if [[ -f "$plist" ]]; then
    launchctl unload "$plist" 2>/dev/null || true
    rm -f "$plist"
    log "    removed $plist"
  fi
done

if [[ "$KEEP_CONFIG" != true ]]; then
  for dir in "$CONFDIR" "${HOME}/Library/Application Support/securemodelroute"; do
    if [[ -d "$dir" ]]; then
      rm -rf "$dir"
      log "    removed config $dir"
    fi
  done
else
  log "    kept config (--keep-config)"
fi

log "Done."
