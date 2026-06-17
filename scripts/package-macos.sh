#!/usr/bin/env bash
# Build macOS release packages for arm64 and x86_64.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck source=macos/common.sh
source "${ROOT}/scripts/macos/common.sh"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "package-macos.sh is for macOS hosts only" >&2
  exit 1
fi

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
OUT="${ROOT}/dist"
DOC_TOOLS="${ROOT}/resources/doc-tools"
mkdir -p "${OUT}"

mac_native_arch_label() {
  smr_native_arch
}

link_doc_tools_current() {
  local arch_label="$1"
  ln -sfn "darwin-${arch_label}" "${DOC_TOOLS}/current"
  echo "==> doc-tools/current -> darwin-${arch_label} (Tauri bundle)"
}

echo "==> Stage bundled document tools (poppler pdftotext) per macOS arch"
stage_macos_tools() {
  local label="$1"
  local dest="${DOC_TOOLS}/darwin-${label}"
  if [[ -f "${dest}/bin/pdftotext" ]]; then
    echo "    darwin-${label} already staged ($(du -sh "${dest}" | awk '{print $1}')), reuse"
    return 0
  fi
  echo "    staging darwin-${label}..."
  bash "${ROOT}/scripts/vendor/stage-doc-tools.sh" "${DOC_TOOLS}" "${label}"
}
for label in arm64 x86_64; do
  stage_macos_tools "${label}"
done
native_arch="$(mac_native_arch_label)"
link_doc_tools_current "${native_arch}"

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
  if [[ -d "${ROOT}/resources/doc-tools" ]]; then
    local tools_src="${ROOT}/resources/doc-tools/darwin-${arch_label}"
    if [[ -f "${tools_src}/bin/pdftotext" ]]; then
      cp -R "${tools_src}" "${stage}/tools"
      echo "==> Bundled doc tools in ${pkg} (from ${tools_src})"
    else
      echo "ERROR: missing doc-tools for darwin-${arch_label} at ${tools_src}/bin/pdftotext" >&2
      exit 1
    fi
  fi

  tar -czf "${OUT}/${pkg}.tar.gz" -C "${stage}" .
  rm -rf "${stage}"

  cp "${bin}" "${OUT}/smr-${arch_label}"
  echo "==> Package: ${OUT}/${pkg}.tar.gz ($(file "${bin}" | sed 's/.*: //'))"
  ls -lh "${OUT}/${pkg}.tar.gz"
}

tauri_bundle_root() {
  local rust_target="${1:-}"
  if [[ -n "$rust_target" && -d "${ROOT}/target/${rust_target}/release/bundle/macos" ]]; then
    echo "${ROOT}/target/${rust_target}/release/bundle"
    return 0
  fi
  if [[ -d "${ROOT}/target/release/bundle/macos" ]]; then
    echo "${ROOT}/target/release/bundle"
    return 0
  fi
  return 1
}

publish_tauri_artifacts() {
  local rust_target="${1:-}"
  local arch_label="$2"
  local bundle_root app_bundle app_name app_bin pkg_app stable_dmg dmg

  bundle_root="$(tauri_bundle_root "$rust_target")" || {
    echo "ERROR: Tauri build did not produce SafeRoute.app for ${arch_label}" >&2
    exit 1
  }

  app_name="SafeRoute.app"
  app_bundle="${bundle_root}/macos/${app_name}"
  if [[ ! -d "$app_bundle" ]]; then
    echo "ERROR: missing ${app_bundle}" >&2
    exit 1
  fi

  app_bin="${app_bundle}/Contents/MacOS/smr-gui"
  if ! file "$app_bin" 2>/dev/null | grep -qE 'Mach-O'; then
    echo "ERROR: ${app_bin} is not a Mach-O binary" >&2
    exit 1
  fi
  if [[ "$arch_label" == "arm64" ]] && ! file "$app_bin" 2>/dev/null | grep -q 'arm64'; then
    echo "ERROR: expected arm64 app binary, got: $(file "$app_bin")" >&2
    exit 1
  fi
  if [[ "$arch_label" == "x86_64" ]] && ! file "$app_bin" 2>/dev/null | grep -qE 'x86_64|386'; then
    echo "ERROR: expected x86_64 app binary, got: $(file "$app_bin")" >&2
    exit 1
  fi

  pkg_app="smr-${VERSION}-darwin-${arch_label}-app"
  rm -f "${OUT}/${pkg_app}.tar.gz"
  tar -czf "${OUT}/${pkg_app}.tar.gz" -C "${bundle_root}/macos" "${app_name}"
  echo "==> Desktop app: ${OUT}/${pkg_app}.tar.gz (${app_name}, $(file "$app_bin" | sed 's/.*: //'))"

  stable_dmg="${OUT}/SafeRoute_${VERSION}_${arch_label}.dmg"
  rm -f "$stable_dmg"
  shopt -s nullglob
  local dmgs=("${bundle_root}/dmg/"*.dmg)
  shopt -u nullglob
  if [[ ${#dmgs[@]} -eq 0 ]]; then
    echo "ERROR: no DMG under ${bundle_root}/dmg/ for ${arch_label}" >&2
    exit 1
  fi
  dmg="${dmgs[0]}"
  cp "$dmg" "$stable_dmg"
  echo "==> Desktop DMG: ${stable_dmg}"
}

build_tauri_desktop() {
  local rust_target="${1:-}"
  local arch_label="$2"
  local -a tauri_args=(--bundles app,dmg)

  link_doc_tools_current "$arch_label"

  if [[ -n "$rust_target" ]]; then
    echo "==> Building desktop app (Tauri cross: ${rust_target})"
    rustup target add "$rust_target" >/dev/null 2>&1 || true
    tauri_args+=(--target "$rust_target")
  else
    echo "==> Building desktop app (Tauri, native ${arch_label})"
  fi

  if ! (cd "$ROOT/gui" && npm run build -- "${tauri_args[@]}"); then
    echo "ERROR: Tauri build failed for ${arch_label}" >&2
    exit 1
  fi

  publish_tauri_artifacts "$rust_target" "$arch_label"
}

# Optional Tauri desktop (native + cross on Apple Silicon)
if [[ "$CLI_ONLY" != true ]] && [[ -f "$ROOT/gui/package.json" ]] && command -v npm >/dev/null 2>&1; then
  echo "==> Sync admin UI assets"
  bash "${ROOT}/scripts/sync-admin-ui.sh"
  if ! (cd "$ROOT/gui" && npm ci --silent); then
    echo "ERROR: npm ci failed in gui/" >&2
    exit 1
  fi

  if [[ "$(smr_native_arch)" == "arm64" ]]; then
    build_tauri_desktop "" "arm64"
    build_tauri_desktop "x86_64-apple-darwin" "x86_64"
  else
    build_tauri_desktop "" "x86_64"
  fi

  link_doc_tools_current "${native_arch}"
elif [[ "$CLI_ONLY" == true ]]; then
  echo "==> Skipping desktop app (--cli-only)"
fi

pack_one "aarch64-apple-darwin" "arm64"
pack_one "x86_64-apple-darwin" "x86_64"

# Default smr symlink for local smoke: native arch
if [[ "$(smr_native_arch)" == "arm64" ]]; then
  cp "${OUT}/smr-arm64" "${OUT}/smr"
else
  cp "${OUT}/smr-x86_64" "${OUT}/smr"
fi

echo ""
echo "==> macOS packages ready: darwin-arm64 + darwin-x86_64"
if [[ "$CLI_ONLY" != true ]] && [[ "$(smr_native_arch)" == "arm64" ]]; then
  echo "    DMGs: SafeRoute_${VERSION}_arm64.dmg, SafeRoute_${VERSION}_x86_64.dmg"
fi

# shellcheck source=dist-layout.sh
source "${ROOT}/scripts/dist-layout.sh"
dist_write_manifest
