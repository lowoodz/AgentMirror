#!/usr/bin/env bash
# Copy embedded admin UI from smr-core into gui/dist (canonical source: crates/smr-core/assets).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${ROOT}/crates/smr-core/assets"
DST="${ROOT}/gui/dist"

mkdir -p "$DST"
cp "${SRC}/index.html" "${DST}/index.html"
echo "sync-admin-ui: ${SRC}/index.html -> ${DST}/index.html"
