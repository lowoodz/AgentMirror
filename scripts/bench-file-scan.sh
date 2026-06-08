#!/usr/bin/env bash
# Benchmark file DLP scan against local test-data/ PDFs (charset skip + 1s budget).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ ! -d "$ROOT/test-data" ]]; then
  echo "test-data/ not found at $ROOT/test-data" >&2
  echo "Place large PDFs there (e.g. annual reports) and re-run." >&2
  exit 1
fi

echo "==> test-data PDFs:"
find "$ROOT/test-data" -maxdepth 1 -name '*.pdf' -exec ls -lh {} \;

echo
echo "==> cargo test (ignored large_pdf scenarios)"
cargo test -p smr-core large_pdf -- --ignored --nocapture
