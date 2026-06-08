#!/usr/bin/env bash
# Build macOS release packages for arm64 and x86_64.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "package-macos.sh is for macOS hosts only" >&2
  exit 1
fi

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
OUT="${ROOT}/dist"
mkdir -p "${OUT}"

CLI_ONLY=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cli-only) CLI_ONLY=true ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

pack_one() {
  local rust_target="$1"
  local arch_label="$2"
  local bin="${ROOT}/target/${rust_target}/release/smr"
  local pkg="smr-${VERSION}-darwin-${arch_label}"
  local stage="${OUT}/stage-${arch_label}"

  echo "==> Building ${rust_target} (release)"
  rustup target add "${rust_target}" >/dev/null 2>&1 || true
  cargo build --release --target "${rust_target}" -p smr-cli

  rm -rf "${stage}"
  mkdir -p "${stage}"
  cp "${bin}" "${stage}/smr"
  cp config/smr.example.yaml "${stage}/smr.example.yaml"
  cp README.md "${stage}/README.md"
  cp scripts/install.sh "${stage}/install.sh"
  cp scripts/verify.sh "${stage}/verify.sh"
  chmod +x "${stage}/install.sh" "${stage}/verify.sh"

  tar -czf "${OUT}/${pkg}.tar.gz" -C "${stage}" .
  rm -rf "${stage}"

  cp "${bin}" "${OUT}/smr-${arch_label}"
  echo "==> Package: ${OUT}/${pkg}.tar.gz ($(file "${bin}" | sed 's/.*: //'))"
  ls -lh "${OUT}/${pkg}.tar.gz"
}

# Optional Tauri (native host arch only)
if [[ "$CLI_ONLY" != true ]] && [[ -f "$ROOT/gui/package.json" ]] && command -v npm >/dev/null 2>&1; then
  echo "==> Sync admin UI assets"
  bash "${ROOT}/scripts/sync-admin-ui.sh"
  echo "==> Building desktop app (Tauri, host arch)"
  (cd "$ROOT/gui" && npm ci --silent && npm run build --silent) || {
    echo "Warning: Tauri build failed or skipped."
  }
  APP_BUNDLE=""
  APP_NAME=""
  for name in SafeRoute.app; do
    if [[ -d "$ROOT/target/release/bundle/macos/${name}" ]]; then
      APP_BUNDLE="$ROOT/target/release/bundle/macos/${name}"
      APP_NAME="$name"
      break
    fi
  done
  if [[ -n "$APP_BUNDLE" ]]; then
    app_bin="$APP_BUNDLE/Contents/MacOS/smr-gui"
    if file "$app_bin" 2>/dev/null | grep -q 'arm64'; then
      host_arch="arm64"
    else
      host_arch="x86_64"
    fi
    PKG_APP="smr-${VERSION}-darwin-${host_arch}-app"
    rm -f "${OUT}/${PKG_APP}.tar.gz"
    tar -czf "${OUT}/${PKG_APP}.tar.gz" -C "$(dirname "$APP_BUNDLE")" "$APP_NAME"
    echo "==> Desktop app: ${OUT}/${PKG_APP}.tar.gz (${APP_NAME})"
    for dmg in "${ROOT}/target/release/bundle/dmg/"*.dmg; do
      if [[ -f "$dmg" ]]; then
        stable="${OUT}/SafeRoute_${VERSION}_${host_arch}.dmg"
        cp "$dmg" "$stable"
        echo "==> Desktop DMG: ${stable}"
        break
      fi
    done
  fi
elif [[ "$CLI_ONLY" == true ]]; then
  echo "==> Skipping desktop app (--cli-only)"
fi

pack_one "aarch64-apple-darwin" "arm64"
pack_one "x86_64-apple-darwin" "x86_64"

# Default smr symlink for local smoke: native arch
native="$(uname -m)"
if [[ "$native" == "arm64" ]]; then
  cp "${OUT}/smr-arm64" "${OUT}/smr"
else
  cp "${OUT}/smr-x86_64" "${OUT}/smr"
fi

echo ""
echo "==> macOS packages ready: darwin-arm64 + darwin-x86_64"

# shellcheck source=dist-layout.sh
source "${ROOT}/scripts/dist-layout.sh"
dist_write_manifest
