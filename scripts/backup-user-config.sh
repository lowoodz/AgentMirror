#!/usr/bin/env bash
# Backup local smr.yaml before release uninstall (gitignored destination).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP_DIR="${ROOT}/config/user-config-backup"
STAMP="$(date '+%Y%m%dT%H%M%S')"
DEST="${BACKUP_DIR}/${STAMP}"

mkdir -p "$DEST"

copied=false
backup_file() {
  local src="$1"
  local name="$2"
  if [[ -f "$src" ]]; then
    cp "$src" "${DEST}/${name}"
    copied=true
    echo "Backed up: $src -> ${DEST}/${name}"
  fi
}

backup_file "${HOME}/Library/Application Support/securemodelroute/smr.yaml" "application-support-smr.yaml"
backup_file "${HOME}/.local/etc/securemodelroute/smr.yaml" "dot-local-smr.yaml"

if [[ "$copied" != true ]]; then
  echo "No user smr.yaml found to backup."
  exit 0
fi

ln -sfn "$STAMP" "${BACKUP_DIR}/latest"
echo "Backup saved under ${DEST}"
