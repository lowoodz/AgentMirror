#!/usr/bin/env bash
# Verify windows-x64 doc-tools bundle (pdftotext + VC++ runtime) before NSIS/zip packaging.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=windows-pe-x64.sh
source "$(cd "$(dirname "$0")" && pwd)/windows-pe-x64.sh"

BIN="${1:-${ROOT}/resources/doc-tools/windows-x64/bin}"
verify_windows_doc_tools_bin "$BIN"
