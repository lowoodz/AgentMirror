#!/usr/bin/env bash
# Download poppler-windows on the Mac/Linux build host so the UTM guest need not fetch GitHub.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-${ROOT}/resources/doc-tools}"
POPPLER_VERSION="24.08.0-0"
STAGE="${OUT}/windows-x64"
BIN="${STAGE}/bin"
LIB="${STAGE}/lib"
CACHE="${ROOT}/dist/vendor-cache"
ZIP_NAME="Release-${POPPLER_VERSION}.zip"
ZIP_URL="https://github.com/oschwartz10612/poppler-windows/releases/download/v${POPPLER_VERSION}/${ZIP_NAME}"
ZIP_PATH="${CACHE}/${ZIP_NAME}"
EXTRACT="${CACHE}/poppler-${POPPLER_VERSION}"

if [[ -f "${BIN}/pdftotext.exe" ]]; then
  echo "==> prefetch-poppler-windows: already staged at ${STAGE}"
  exit 0
fi

mkdir -p "$CACHE" "$BIN" "$LIB"
rm -rf "$STAGE"
mkdir -p "$BIN" "$LIB"

if [[ ! -f "$ZIP_PATH" ]]; then
  echo "==> Download poppler-windows ${POPPLER_VERSION}"
  curl -fsSL -o "$ZIP_PATH" "$ZIP_URL"
fi

if [[ ! -d "$EXTRACT" ]]; then
  tmp="$(mktemp -d)"
  unzip -q "$ZIP_PATH" -d "$tmp"
  top="$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -1)"
  [[ -n "$top" ]] || { echo "poppler zip extract failed" >&2; exit 1; }
  mv "$top" "$EXTRACT"
  rm -rf "$tmp"
fi

POPPLER_BIN="${EXTRACT}/Library/bin"
[[ -f "${POPPLER_BIN}/pdftotext.exe" ]] || { echo "pdftotext.exe missing in ${POPPLER_BIN}" >&2; exit 1; }

cp -f "${POPPLER_BIN}/pdftotext.exe" "${BIN}/pdftotext.exe"
for dll in "${POPPLER_BIN}"/*.dll; do
  [[ -f "$dll" ]] || continue
  base="$(basename "$dll")"
  cp -f "$dll" "${LIB}/${base}"
  cp -f "$dll" "${BIN}/${base}"
done

echo "==> prefetch-poppler-windows: staged at ${STAGE} ($(du -sh "${STAGE}" | awk '{print $1}'))"
# Do not touch resources/doc-tools/current (macOS Tauri bundle symlink).
