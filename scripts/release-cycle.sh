#!/usr/bin/env bash
# Cross-platform release cycle entry (macOS host → macos/release-cycle.sh; Windows → windows/release-cycle.ps1).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/release-cycle.sh [phase] [options]

macOS entry — delegates to scripts/macos/release-cycle.sh on Darwin.

For Mac full validation (mac + Windows zip + UTM NSIS + VM tests):
  ./scripts/release-full.sh

Phases: all | preflight | clean | compile | package | verify | test | install | installed

Artifact options (CLI always; app + installer optional):
  --with-app / --without-app / --no-app
  --with-dmg / --without-dmg / --no-dmg   (macOS DMG)
  --cli-only                              CLI tar only

Other: --skip-clean --skip-tests --skip-installed --keep-config --log PATH

On Windows, use PowerShell instead:
  .\scripts\windows\release-cycle.ps1
  Flags: -WithApp -WithoutApp -WithSetup -WithoutSetup -CliOnly
        -SkipClean -SkipTests -SkipInstalled -KeepConfigOnClean

See .cursor/skills/release-cycle/SKILL.md
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac

case "$(uname -s)" in
  Darwin)
    exec bash "${ROOT}/scripts/macos/release-cycle.sh" "$@"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    echo "On Windows, run in PowerShell:" >&2
    echo "  Set-ExecutionPolicy Bypass -Scope Process -Force" >&2
    echo "  .\scripts\windows\release-cycle.ps1 [-CliOnly | -WithApp | -WithoutApp | -WithSetup | -WithoutSetup]" >&2
    echo "See: .cursor/skills/release-cycle/SKILL.md" >&2
    exit 2
    ;;
  *)
    echo "Release cycle requires macOS or Windows host." >&2
    echo "  macOS:   ./scripts/release-cycle.sh [--cli-only ...]" >&2
    echo "  Windows: .\scripts\windows\release-cycle.ps1 [-CliOnly ...]" >&2
    exit 2
    ;;
esac
