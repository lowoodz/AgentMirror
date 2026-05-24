#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

echo "==> Building SecureModelRoute (release)"
cargo build --release

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
echo "==> Package: $OUT/${PKG}.tar.gz"
echo "==> Binary:  $OUT/smr"

ls -lh "$OUT/${PKG}.tar.gz" "$OUT/smr"
