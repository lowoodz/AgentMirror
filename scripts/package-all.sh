#!/usr/bin/env bash
# Build all platform packages: macOS (arm64 + x86_64) and Windows (x86_64).
# Options: --clean (run clean-dist first), --cli-only (skip Tauri app/DMG/NSIS)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

DO_CLEAN=false
CLI_ONLY=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --clean) DO_CLEAN=true ;;
    --cli-only) CLI_ONLY=true ;;
    -h|--help)
      echo "Usage: $0 [--clean] [--cli-only]"
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [[ "$DO_CLEAN" == true ]]; then
  bash "${ROOT}/scripts/clean-dist.sh"
fi

echo "==> Sync admin UI"
bash "${ROOT}/scripts/sync-admin-ui.sh"

if [[ "$(uname -s)" == "Darwin" ]]; then
  if [[ "$CLI_ONLY" == true ]]; then
    bash "${ROOT}/scripts/package-macos.sh" --cli-only
  else
    bash "${ROOT}/scripts/package-macos.sh"
  fi
else
  if [[ "$CLI_ONLY" == true ]]; then
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "${ROOT}/scripts/package.ps1" -CliOnly
  else
    bash "${ROOT}/scripts/package.sh"
  fi
fi

if [[ "$CLI_ONLY" == true ]]; then
  bash "${ROOT}/scripts/package-windows.sh"
  # shellcheck source=dist-layout.sh
  source "${ROOT}/scripts/dist-layout.sh"
  dist_write_manifest
  echo ""
  echo "==> CLI-only packages in ${ROOT}/dist/"
  ls -lh "${ROOT}/dist/"*.tar.gz "${ROOT}/dist/"*.zip 2>/dev/null || true
  exit 0
fi

echo ""
# shellcheck source=vm/vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"
vm_ssh_init
VERSION="$(grep '^version' "${ROOT}/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
STABLE_SETUP="${ROOT}/dist/SafeRoute_${VERSION}_x64-setup.exe"
VM_GUI_BUILT=0

if vm_ssh_require 2>/dev/null; then
  echo "==> VM reachable via SSH ($VM_SSH): building Windows desktop (Tauri NSIS)"
  bash "${ROOT}/scripts/vm/package-windows-gui.sh"
  VM_GUI_BUILT=1
else
  echo "==> Windows VM SSH not reachable ($VM_SSH): cannot build Windows NSIS" >&2
fi

bash "${ROOT}/scripts/package-windows.sh"

if [[ "$VM_GUI_BUILT" == 1 ]]; then
  if [[ ! -f "$STABLE_SETUP" ]]; then
    echo "ERROR: NSIS setup.exe not found after VM build — check dist/windows-desktop-build.log" >&2
    exit 1
  fi
  echo ""
  echo "==> Windows NSIS installer: ${STABLE_SETUP}"
else
  echo "ERROR: full package requires Windows VM SSH + fresh NSIS build (set config/test.env, then re-run)" >&2
  if [[ -f "$STABLE_SETUP" ]]; then
    echo "       stale ${STABLE_SETUP} exists but was not rebuilt — refusing to ship it" >&2
  fi
  exit 1
fi

echo ""
echo "==> All packages in ${ROOT}/dist/"
ls -lh "${ROOT}/dist/"*.tar.gz "${ROOT}/dist/"*.zip "${ROOT}/dist/"*-setup.exe 2>/dev/null || true
# shellcheck source=dist-layout.sh
source "${ROOT}/scripts/dist-layout.sh"
dist_write_manifest
