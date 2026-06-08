#!/usr/bin/env bash
# Remove stale dist/ artifacts and regenerate LATEST-INSTALLERS.txt.
# Also cleans Windows UTM guest staging when reachable (see clean-vm.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=dist-layout.sh
source "${ROOT}/scripts/dist-layout.sh"
dist_clean
bash "${ROOT}/scripts/clean-vm.sh" "$@"
