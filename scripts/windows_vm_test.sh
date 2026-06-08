#!/usr/bin/env bash
# Copy Windows release zip to VM (windows-user SSH) and run install + verify.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"
# shellcheck source=vm/vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

vm_ssh_init
if [[ -z "${SMR_WINDOWS_REMOTE_DIR:-}" ]]; then
  if [[ -n "${SMR_GUEST_STAGING:-}" ]]; then
    REMOTE_DIR="/$(echo "$SMR_GUEST_STAGING" | tr ':\\' '//')"
    REMOTE_DIR="${REMOTE_DIR}/smr-test"
  else
    REMOTE_DIR="/c/Users/${SMR_WINDOWS_USER}/smr-staging/smr-test"
  fi
else
  REMOTE_DIR="$SMR_WINDOWS_REMOTE_DIR"
fi

ZIP="$(ls -t "${ROOT}"/dist/smr-*-windows-x86_64.zip 2>/dev/null | head -1)"
if [[ -z "${ZIP}" ]]; then
  echo "No Windows zip found. Run: ./scripts/package-windows.sh"
  exit 1
fi

vm_ssh_require
echo "==> Upload ${ZIP} -> ${VM_SSH}:${REMOTE_DIR}"
vm_ssh "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force -Path '${REMOTE_DIR//\//\\}' | Out-Null\""
scp "${ZIP}" "${VM_SSH}:${REMOTE_DIR}/smr.zip"

echo "==> Remote install + verify ($VM_SSH)"
vm_ssh "powershell -NoProfile -ExecutionPolicy Bypass -Command \"
  Set-Location '${REMOTE_DIR//\//\\}';
  Expand-Archive -Path smr.zip -DestinationPath . -Force;
  .\\install.ps1;
  .\\verify.ps1;
  Write-Host 'Windows VM install test passed.';
\""

echo "==> Done."
