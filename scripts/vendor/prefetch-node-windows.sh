#!/usr/bin/env bash
# Download portable Node.js for Windows ARM64 guest builds (UTM VM may not reach nodejs.org).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CACHE="${ROOT}/dist/vendor-cache"
VER="22.15.0"
ARCH="${1:-arm64}"
case "$ARCH" in
  arm64) ZIP="node-v${VER}-win-arm64.zip"; OUT="${CACHE}/node-win-${VER}-arm64" ;;
  x64)   ZIP="node-v${VER}-win-x64.zip"; OUT="${CACHE}/node-win-${VER}-x64" ;;
  *) echo "Unsupported arch: $ARCH" >&2; exit 2 ;;
esac
URL="https://nodejs.org/dist/v${VER}/${ZIP}"
ZIP_PATH="${CACHE}/${ZIP}"

mkdir -p "$CACHE"
if [[ ! -f "$ZIP_PATH" ]]; then
  echo "==> Download ${ZIP}"
  curl -fsSL -o "$ZIP_PATH" "$URL"
fi

if [[ ! -x "${OUT}/node.exe" ]]; then
  rm -rf "$OUT"
  tmp="$(mktemp -d)"
  unzip -q "$ZIP_PATH" -d "$tmp"
  top="$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -1)"
  [[ -n "$top" ]] || { echo "node zip extract failed" >&2; exit 1; }
  mv "$top" "$OUT"
  rm -rf "$tmp"
fi

echo "==> prefetch-node-windows: ${OUT} ($(du -sh "${OUT}" | awk '{print $1}'))"
echo "$OUT"
