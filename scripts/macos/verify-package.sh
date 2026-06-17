#!/usr/bin/env bash
# Verify dist/ artifacts after scripts/package-macos.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=macos/common.sh
source "${ROOT}/scripts/macos/common.sh"

REQUIRE_APP=false
REQUIRE_DMG=false
QUIET=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --require-app) REQUIRE_APP=true ;;
    --require-dmg) REQUIRE_DMG=true ;;
    --quiet) QUIET=true ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

eval "$(smr_dist_paths)"
failures=()

check_file() {
  local path="$1" label="$2"
  if [[ ! -f "$path" ]]; then
    failures+=("Missing ${label}: ${path}")
    return
  fi
  local size
  size=$(stat -f%z "$path" 2>/dev/null || stat -c%s "$path" 2>/dev/null || echo 0)
  if [[ "$size" -lt 1024 ]]; then
    failures+=("Suspiciously small ${label} (${size} bytes): ${path}")
    return
  fi
  if [[ "$QUIET" != true ]]; then
    echo "[OK] ${label} ($(ls -lh "$path" | awk '{print $5}')): $(basename "$path")"
  fi
}

check_file "$CLI_TAR" "CLI tar"
if smr_host_is_apple_silicon; then
  check_file "${ROOT}/dist/smr-${VERSION}-darwin-x86_64.tar.gz" "CLI tar (x86_64 cross)"
  if [[ -f "${ROOT}/dist/smr-x86_64" ]]; then
    if ! file "${ROOT}/dist/smr-x86_64" 2>/dev/null | grep -qE 'x86_64|386'; then
      failures+=("dist/smr-x86_64 is not an x86_64 binary")
    elif [[ "$QUIET" != true ]]; then
      echo "[OK] x86_64 CLI binary ($(ls -lh "${ROOT}/dist/smr-x86_64" | awk '{print $5}')): smr-x86_64"
    fi
  fi
  if [[ "$REQUIRE_APP" == true ]]; then
    check_file "${ROOT}/dist/smr-${VERSION}-darwin-x86_64-app.tar.gz" "App tar (x86_64 cross)"
  fi
  if [[ "$REQUIRE_DMG" == true ]]; then
    check_file "${ROOT}/dist/AgentMirror_${VERSION}_x86_64.dmg" "DMG (x86_64 cross)"
  fi
fi
[[ "$REQUIRE_APP" == true ]] && check_file "$APP_TAR" "App tar"
if [[ "$REQUIRE_DMG" == true ]]; then
  check_file "$DMG" "DMG"
fi

# Also accept Tauri DMG under target if not copied to dist yet
if [[ "$REQUIRE_DMG" == true && ! -f "$DMG" ]]; then
  for candidate in "${ROOT}/target/release/bundle/dmg/"*.dmg; do
    [[ -f "$candidate" ]] && check_file "$candidate" "Tauri DMG (target)"
    break
  done
fi

if [[ ${#failures[@]} -gt 0 ]]; then
  echo "Package verification failed:" >&2
  printf ' - %s\n' "${failures[@]}" >&2
  exit 1
fi

[[ "$QUIET" != true ]] && echo "" && echo "Package verification passed (v${VERSION}, ${arch:-$(smr_native_arch)})."
exit 0
