#!/usr/bin/env bash
# Build all platform packages: macOS (arm64 + x86_64) and Windows (x86_64).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "$(uname -s)" == "Darwin" ]]; then
  bash "${ROOT}/scripts/package-macos.sh"
else
  bash "${ROOT}/scripts/package.sh"
fi

echo ""
bash "${ROOT}/scripts/package-windows.sh"

echo ""
echo "==> All packages in ${ROOT}/dist/"
ls -lh "${ROOT}/dist/"*.tar.gz "${ROOT}/dist/"*.zip 2>/dev/null || true
