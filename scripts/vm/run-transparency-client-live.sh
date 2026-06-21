#!/usr/bin/env bash
# Windows VM: live OpenClaw + Claude transparency (direct vs SafeRoute).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"
vm_ssh_require

LOG_LOCAL="${ROOT}/dist/windows-transparency-client-live.log"
GUEST_WORK="${SMR_GUEST_STAGING}/transparency-client-live"
HOST_WORK="${ROOT}/dist/transparency-client-live"

mkdir -p "$HOST_WORK" "${ROOT}/dist"
rm -f "$LOG_LOCAL"

python3 -c "
import sys
sys.path.insert(0, '${ROOT}/scripts')
from transparency_client_live_test import build_transparency_config
from test_common import dump_yaml, parse_keys
parse_keys()
from pathlib import Path
out = Path('${HOST_WORK}/smr-transparency.yaml')
out.write_text(dump_yaml(build_transparency_config('127.0.0.1:8080')) + '\n', encoding='utf-8')
print(out)
"

vm_ssh "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force -Path '${GUEST_WORK//\//\\}' | Out-Null\""

GUEST_COUNT="${GUEST_WORK}/count-fixture"
vm_ssh "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force -Path '${GUEST_COUNT//\//\\}' | Out-Null; New-Item -ItemType File -Force -Path '${GUEST_COUNT//\//\\}\\a.txt','${GUEST_COUNT//\//\\}\\b.txt' | Out-Null\""

if [[ -f "${ROOT}/config/test.env" ]]; then
  vm_scp_to "${ROOT}/config/test.env" "${GUEST_WORK}/test.env"
fi

for f in transparency_client_live_test.py transparency_pass_through_test.py test_common.py generate_openclaw_saferoute_config.py openclaw_matrix_common.py; do
  vm_scp_to "${ROOT}/scripts/${f}" "${GUEST_WORK}/${f}"
done
vm_scp_to "${HOST_WORK}/smr-transparency.yaml" "${GUEST_WORK}/smr-transparency.yaml"

# Count dir is resolved on the guest (USERPROFILE). Do not pass host placeholder paths.
REMOTE_PS="${SMR_GUEST_STAGING}/run-transparency-client-live-remote.ps1"
vm_scp_to "${ROOT}/scripts/vm/run-transparency-client-live-remote.ps1" "$REMOTE_PS"

# Count dir is resolved on the guest (USERPROFILE). Do not pass host placeholder paths.
if vm_ssh "powershell -NoProfile -ExecutionPolicy Bypass -File \"${REMOTE_PS//\//\\}\" -GuestWork \"${GUEST_WORK//\//\\}\" -CountDir \"${GUEST_COUNT//\//\\}\"" | tee "$LOG_LOCAL"; then
  echo "==> Windows transparency client live PASSED"
  exit 0
fi
echo "==> Windows transparency client live FAILED (see $LOG_LOCAL)" >&2
exit 1
