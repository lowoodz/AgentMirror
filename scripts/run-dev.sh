#!/usr/bin/env bash
# Run release-built smr with bundled doc-tools (pdftotext) for dev parity with installers.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

case "$(uname -m)" in
  arm64) SMR_TOOLS_DIR="${ROOT}/resources/doc-tools/darwin-aarch64" ;;
  x86_64) SMR_TOOLS_DIR="${ROOT}/resources/doc-tools/darwin-x86_64" ;;
  *)
    echo "Unsupported macOS arch for bundled doc-tools: $(uname -m)" >&2
    exit 2
    ;;
esac

if [[ ! -d "$SMR_TOOLS_DIR" ]]; then
  echo "Missing ${SMR_TOOLS_DIR}; run packaging vendor stage or copy poppler bundle." >&2
  exit 1
fi
export SMR_TOOLS_DIR

CONFIG="${SMR_CONFIG:-${HOME}/Library/Application Support/securemodelroute/smr.yaml}"
if [[ ! -f "$CONFIG" ]]; then
  mkdir -p "$(dirname "$CONFIG")"
  cp "${ROOT}/config/smr.example.yaml" "$CONFIG"
  echo "Created default config: $CONFIG"
fi

echo "SMR_TOOLS_DIR=$SMR_TOOLS_DIR"
echo "Config: $CONFIG"

cargo build --release --quiet

BIN="${ROOT}/target/release/smr"
exec "$BIN" --config "$CONFIG"
