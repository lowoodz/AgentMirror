#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

echo "==> Building SecureModelRoute (release)"
cargo build --release

if [[ -f "$ROOT/gui/package.json" ]]; then
  echo "==> Building desktop app (Tauri)"
  if command -v npm >/dev/null 2>&1; then
  (cd "$ROOT/gui" && npm ci --silent && npm run build --silent) || {
    echo "Warning: Tauri build failed or skipped; CLI package will still be produced."
  }
  else
    echo "Warning: npm not found; skipping desktop app build."
  fi
fi

BIN="$ROOT/target/release/smr"
OUT="$ROOT/dist"
mkdir -p "$OUT"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
ARCH="$(uname -m)"
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
PKG="smr-${VERSION}-${OS}-${ARCH}"

cp "$BIN" "$OUT/smr"
cp config/smr.example.yaml "$OUT/smr.example.yaml"
cp README.md "$OUT/README.md"
cp scripts/install.sh "$OUT/install.sh"
chmod +x "$OUT/install.sh"

tar -czf "$OUT/${PKG}.tar.gz" -C "$OUT" smr smr.example.yaml README.md install.sh

APP_BUNDLE="$ROOT/target/release/bundle/macos/SecureModelRoute.app"
if [[ -d "$APP_BUNDLE" ]]; then
  PKG_APP="smr-${VERSION}-${OS}-${ARCH}-app"
  tar -czf "$OUT/${PKG_APP}.tar.gz" -C "$(dirname "$APP_BUNDLE")" SecureModelRoute.app
  echo "==> Desktop app: $OUT/${PKG_APP}.tar.gz"
fi

echo "==> Package: $OUT/${PKG}.tar.gz"
echo "==> Binary:  $OUT/smr"
echo "==> Arch:    ${OS}-${ARCH}"

ls -lh "$OUT/${PKG}.tar.gz" "$OUT/smr"
