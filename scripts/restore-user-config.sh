#!/usr/bin/env bash
# Restore smr.yaml from the latest backup (see backup-user-config.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP_DIR="${ROOT}/config/user-config-backup"
LATEST="${BACKUP_DIR}/latest"

if [[ ! -e "$LATEST" ]]; then
  echo "No config backup found at ${BACKUP_DIR}/latest" >&2
  exit 1
fi

SNAP="$(readlink "$LATEST" 2>/dev/null || basename "$LATEST")"
SRC_DIR="${BACKUP_DIR}/${SNAP}"

restore_one() {
  local backup_file="$1"
  local dest="$2"
  if [[ ! -f "$backup_file" ]]; then
    return 0
  fi
  mkdir -p "$(dirname "$dest")"
  cp "$backup_file" "$dest"
  echo "Restored: $dest"
}

restored=false
if [[ -f "${SRC_DIR}/application-support-smr.yaml" ]]; then
  restore_one "${SRC_DIR}/application-support-smr.yaml" \
    "${HOME}/Library/Application Support/securemodelroute/smr.yaml"
  restored=true
fi
if [[ -f "${SRC_DIR}/dot-local-smr.yaml" ]]; then
  restore_one "${SRC_DIR}/dot-local-smr.yaml" \
    "${HOME}/.local/etc/securemodelroute/smr.yaml"
  restored=true
fi

if [[ "$restored" != true ]]; then
  echo "Backup ${SRC_DIR} contains no smr.yaml files." >&2
  exit 1
fi

echo "Config restore complete (file index will rebuild on next start)."
