#!/usr/bin/env bash
# Canonical dist/ paths for release artifacts, staging, and logs.
# Source from package / verify / test scripts — do not hard-code versioned names elsewhere.
set -euo pipefail

dist_layout_root() {
  cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd
}

dist_layout_version() {
  grep '^version' "$(dist_layout_root)/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/'
}

# Print KEY=value lines (eval "$(dist_layout_paths)")
dist_layout_paths() {
  local root version dist arch app_arch
  root="$(dist_layout_root)"
  version="$(dist_layout_version)"
  dist="${root}/dist"
  arch="$(uname -m 2>/dev/null || echo unknown)"
  if [[ "$arch" == "arm64" ]]; then arch="arm64"; else arch="x86_64"; fi
  app_arch="$arch"

  cat <<EOF
ROOT=${root}
VERSION=${version}
DIST=${dist}
ARCH=${arch}
APP_ARCH=${app_arch}
CLI_DARWIN_ARM64=${dist}/smr-${version}-darwin-arm64.tar.gz
CLI_DARWIN_X86_64=${dist}/smr-${version}-darwin-x86_64.tar.gz
APP_DARWIN_ARM64=${dist}/smr-${version}-darwin-arm64-app.tar.gz
APP_DARWIN_X86_64=${dist}/smr-${version}-darwin-x86_64-app.tar.gz
DMG_ARM64=${dist}/SafeRoute_${version}_arm64.dmg
CLI_WINDOWS_ZIP=${dist}/smr-${version}-windows-x86_64.zip
APP_WINDOWS_ZIP=${dist}/smr-${version}-windows-x86_64-app.zip
NSIS_SETUP=${dist}/SafeRoute_${version}_x64-setup.exe
CLI_WINDOWS_EXE=${dist}/smr.exe
WIN_DESKTOP_DIR=${dist}/windows-desktop
WIN_DESKTOP_EXE=${dist}/windows-desktop/SafeRoute.exe
MANIFEST=${dist}/LATEST-INSTALLERS.txt
LOG_MACOS_RELEASE=${dist}/macos-release-cycle.log
LOG_MACOS_INSTALL=${dist}/macos-install-smoke.log
LOG_WINDOWS_RELEASE=${dist}/windows-release-cycle.log
LOG_WINDOWS_DESKTOP_BUILD=${dist}/windows-desktop-build.log
LOG_WINDOWS_NSIS_TEST=${dist}/windows-nsis-install-test.log
LOG_WINDOWS_UTM_APP=${dist}/windows-utm-installed-app-test.log
LOG_WINDOWS_UTM_FUNC=${dist}/windows-utm-test.log
LOG_WINDOWS_UTM_PY=${dist}/windows-utm-python-test.log
LOG_WINDOWS_INSTALL_SMOKE=${dist}/windows-install-smoke.log
TEST_RUNS_DIR=${dist}/test-runs
EOF
}

dist_write_manifest() {
  eval "$(dist_layout_paths)"
  mkdir -p "$DIST" "$WIN_DESKTOP_DIR"
  cat > "$MANIFEST" <<EOF
LLM-SafeRoute ${VERSION} — release artifacts (fixed paths under dist/)

Ship (upload / hand to users):
  macOS CLI:     smr-${VERSION}-darwin-arm64.tar.gz
                 smr-${VERSION}-darwin-x86_64.tar.gz
  macOS app:     smr-${VERSION}-darwin-arm64-app.tar.gz
  macOS DMG:     SafeRoute_${VERSION}_arm64.dmg
  Windows CLI:   smr-${VERSION}-windows-x86_64.zip
  Windows app:   smr-${VERSION}-windows-x86_64-app.zip
  Windows NSIS:  SafeRoute_${VERSION}_x64-setup.exe   (Tauri NSIS only)

UTM / install-test staging (not for end users):
  dist/windows-desktop/SafeRoute.exe
  dist/smr.exe

Build / test logs (fixed names, overwritten each run):
  macos-release-cycle.log, windows-desktop-build.log, windows-nsis-install-test.log, …

Deprecated — removed from repo:
  IExpress installer scripts (use Tauri NSIS via package-windows-gui.sh / package.ps1)
EOF
  echo "Wrote ${MANIFEST}"
}

dist_clean() {
  eval "$(dist_layout_paths)"
  local version="$VERSION"
  mkdir -p "$DIST" "$WIN_DESKTOP_DIR" "$TEST_RUNS_DIR"

  echo "==> dist clean (keep v${version} release artifacts only)"

  # Old release versions
  find "$DIST" -maxdepth 1 \( \
    -name 'smr-[0-9]*-darwin-*.tar.gz' -o \
    -name 'smr-[0-9]*-windows-*.zip' -o \
    -name 'SafeRoute_[0-9]*_*.dmg' -o \
    -name 'SafeRoute_[0-9]*_*-setup.exe' \
    \) ! -name "*${version}*" -print -delete 2>/dev/null || true

  # Duplicate DMG naming (keep _arm64 only)
  rm -f "$DIST/SafeRoute_${version}_aarch64.dmg" 2>/dev/null || true

  # Legacy IExpress
  rm -f \
    "$DIST"/SafeRoute-*-x64-Setup.exe \
    "$DIST"/smr-*-windows-x86_64-full.zip 2>/dev/null || true
  rm -f "$WIN_DESKTOP_DIR"/SafeRoute-[0-9]*-x64-Setup.exe 2>/dev/null || true
  find "$WIN_DESKTOP_DIR" -maxdepth 1 -type f ! -name 'SafeRoute.exe' \
    ! -name "SafeRoute_${version}_x64-setup.exe" -print -delete 2>/dev/null || true
  find "$WIN_DESKTOP_DIR" -maxdepth 1 -type f -name '*setup.sed*' -o -name '*Wrote SED*' \
    -print -delete 2>/dev/null || true
  find "$WIN_DESKTOP_DIR" -maxdepth 1 -type f -empty -delete 2>/dev/null || true

  # Intermediate build inputs (recreated on demand)
  rm -f "$DIST"/smr-windows-build-src.zip "$DIST"/smr-windows-build-src.tar.gz 2>/dev/null || true
  rm -f "$DIST"/smr-arm64 "$DIST"/smr-x86_64 2>/dev/null || true

  # CLI zip staging copies at dist root (inside .zip already)
  rm -f \
    "$DIST"/README.md "$DIST"/smr.example.yaml \
    "$DIST"/install.ps1 "$DIST"/install.sh \
    "$DIST"/uninstall.ps1 "$DIST"/verify.ps1 "$DIST"/verify.sh 2>/dev/null || true

  # Ad-hoc / historical logs and probes
  find "$DIST" -maxdepth 1 -type f -name '*.log' -print -delete 2>/dev/null || true
  find "$DIST" -maxdepth 1 -type l -name '*.log' -delete 2>/dev/null || true
  rm -f \
    "$DIST"/.DS_Store "$DIST"/.smr-gui-probe.txt \
    "$DIST"/find-smr.txt "$DIST"/windows-probe.txt \
    "$DIST"/id_ed25519.pub 2>/dev/null || true
  rm -rf "$DIST/ci-windows" "$DIST/color-check" "$DIST/test-runs" 2>/dev/null || true
  mkdir -p "$TEST_RUNS_DIR"

  dist_write_manifest
  echo "==> dist/ after clean:"
  ls -lh "$DIST" 2>/dev/null | head -25
  ls -lh "$WIN_DESKTOP_DIR" 2>/dev/null || true
}
