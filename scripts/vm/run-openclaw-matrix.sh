#!/usr/bin/env bash
# Windows VM: install latest dist (optional), deploy portable matrix config, run OpenClaw matrix.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"

LOG_LOCAL="${ROOT}/dist/windows-openclaw-matrix.log"
KEEP_MATRIX_CONFIG=false
SKIP_INSTALL=false

usage() {
  cat <<'EOF'
Usage: scripts/vm/run-openclaw-matrix.sh [options]

Options:
  --skip-install           Skip NSIS/install smoke (SafeRoute already running on guest)
  --keep-matrix-config     Leave matrix smr.yaml on guest after run
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-install) SKIP_INSTALL=true; shift ;;
    --keep-matrix-config) KEEP_MATRIX_CONFIG=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown: $1" >&2; exit 2 ;;
  esac
done

vm_ssh_require
GUEST_WORK="${GUEST_STAGING}/openclaw-matrix"

vm_ssh_mkdir "$GUEST_WORK"

HOST_WORK="${ROOT}/dist/openclaw-matrix"
mkdir -p "$HOST_WORK"
python3 "${ROOT}/scripts/generate_openclaw_matrix_config.py" \
  --matrix-root "${GUEST_STAGING}/smr-matrix" \
  --output "${HOST_WORK}/smr.yaml" \
  --env-file "${HOST_WORK}/windows.env"

for f in openclaw_security_matrix_test.py openclaw_matrix_common.py test_common.py; do
  vm_scp_to "${ROOT}/scripts/${f}" "${GUEST_WORK}/${f}"
done
vm_scp_to "${HOST_WORK}/smr.yaml" "${GUEST_WORK}/smr.yaml"
vm_scp_to "${HOST_WORK}/windows.env" "${GUEST_WORK}/matrix.env"
# Keys for optional on-guest tooling (config is generated on Mac host).
if [[ -f "${ROOT}/test_model_api_key.txt" ]]; then
  vm_scp_to "${ROOT}/test_model_api_key.txt" "${GUEST_WORK}/test_model_api_key.txt"
fi

if [[ "$SKIP_INSTALL" == false ]]; then
  echo "==> Windows NSIS install (dist setup.exe)"
  SETUP="$(ls -t "${ROOT}"/dist/SafeRoute_*_x64-setup.exe 2>/dev/null | head -1)"
  [[ -n "$SETUP" ]] || { echo "Missing dist/SafeRoute_*_x64-setup.exe" >&2; exit 1; }
  vm_scp_to "$SETUP" "${GUEST_STAGING}/SafeRoute-setup.exe"
  vm_ssh "cmd.exe /c \"${GUEST_STAGING}/SafeRoute-setup.exe /S\" 2>nul || ${GUEST_STAGING}/SafeRoute-setup.exe /S"
  sleep 8
fi

REMOTE_PS="${GUEST_STAGING}/run-openclaw-matrix-remote.ps1"
vm_scp_to "${ROOT}/scripts/vm/run-openclaw-matrix-remote.ps1" "$REMOTE_PS"

KEEP_FLAG=""
[[ "$KEEP_MATRIX_CONFIG" == true ]] && KEEP_FLAG="-KeepMatrixConfig"

rm -f "$LOG_LOCAL"
vm_ssh "cmd.exe /c \"set SMR_GUEST_STAGING=${GUEST_STAGING}&& set SMR_GUEST_WORK=${GUEST_WORK}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${REMOTE_PS} ${KEEP_FLAG}\"" \
  2>&1 | tee "$LOG_LOCAL"

if grep -q "Summary: 10/10 passed" "$LOG_LOCAL"; then
  echo "==> Windows OpenClaw matrix PASSED"
  exit 0
fi
grep -qE "Summary: [0-9]+/10 passed" "$LOG_LOCAL" && exit 1
echo "==> Windows OpenClaw matrix did not complete cleanly" >&2
exit 1
