#!/usr/bin/env bash
# Build Windows Tauri desktop via UTM VM (ARM64 native or x86_64 guest).
# For guaranteed x86_64 release: run on windows-latest CI or .\\scripts\\package.ps1 on x86_64 Windows.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
bash "${ROOT}/scripts/vm/package-windows-gui.sh"
