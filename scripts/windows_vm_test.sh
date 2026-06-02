#!/usr/bin/env bash
# Copy Windows release zip to a remote Windows host (OpenSSH) and run install + verify.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"

HOST="${SMR_WINDOWS_HOST:-}"
USER="${SMR_WINDOWS_USER:-}"
REMOTE_DIR="${SMR_WINDOWS_REMOTE_DIR:-/c/Users/Public/smr-test}"

if [[ -z "${HOST}" || -z "${USER}" ]]; then
  echo "Set SMR_WINDOWS_HOST and SMR_WINDOWS_USER in config/test.env (see config/test.env.example)" >&2
  exit 1
fi

ZIP="$(ls -t "${ROOT}"/dist/smr-*-windows-x86_64.zip 2>/dev/null | head -1)"
if [[ -z "${ZIP}" ]]; then
  echo "No Windows zip found. Run: ./scripts/package-windows.sh"
  exit 1
fi

echo "==> Upload ${ZIP} -> ${USER}@${HOST}:${REMOTE_DIR}"
ssh "${USER}@${HOST}" "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force -Path '${REMOTE_DIR//\//\\}' | Out-Null\""
scp "${ZIP}" "${USER}@${HOST}:${REMOTE_DIR}/smr.zip"

echo "==> Remote install + verify"
ssh "${USER}@${HOST}" "powershell -NoProfile -ExecutionPolicy Bypass -Command \"
  Set-Location '${REMOTE_DIR//\//\\}';
  Expand-Archive -Path smr.zip -DestinationPath . -Force;
  .\\install.ps1;
  .\\verify.ps1;
  Write-Host 'Windows VM install test passed.';
\""

echo "==> Done."
