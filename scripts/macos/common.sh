#!/usr/bin/env bash
# Shared helpers for macOS release scripts.
set -euo pipefail

smr_root() {
  local dir
  dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  echo "$dir"
}

smr_version() {
  grep '^version' "$(smr_root)/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/'
}

smr_set_build_env() {
  local root
  root="$(smr_root)"
  export PATH="${HOME}/.cargo/bin:${PATH}"
  export CARGO_TARGET_DIR="${root}/target"
  export PYTHONUTF8=1
  # Avoid embedding builder paths in release binary strings (panic/backtrace/linker metadata).
  export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${HOME}/=~/ --remap-path-prefix=${root}/=."
}

smr_stop_processes() {
  pkill -f 'target/release/smr' 2>/dev/null || true
  pkill -f 'smr-gui' 2>/dev/null || true
  pkill -f 'SafeRoute.app' 2>/dev/null || true
  pkill -f 'SecureModelRoute.app' 2>/dev/null || true
  lsof -ti :8080 | xargs kill -9 2>/dev/null || true
  lsof -ti :18080 | xargs kill -9 2>/dev/null || true
  sleep "${1:-2}"
}

smr_host_is_apple_silicon() {
  [[ "$(uname -s)" == "Darwin" ]] || return 1
  sysctl -n hw.optional.arm64 2>/dev/null | grep -qx 1
}

smr_native_arch() {
  if smr_host_is_apple_silicon; then echo arm64; else echo x86_64; fi
}

smr_dist_paths() {
  local root version arch dist app_arch dmg candidate
  root="$(smr_root)"
  version="$(smr_version)"
  dist="${root}/dist"
  arch="$(smr_native_arch)"
  if [[ ! -f "${dist}/smr-${version}-darwin-${arch}.tar.gz" ]]; then
    for candidate in arm64 x86_64; do
      if [[ -f "${dist}/smr-${version}-darwin-${candidate}.tar.gz" ]]; then
        arch="$candidate"
        break
      fi
    done
  fi
  app_arch="$arch"
  if [[ ! -f "${dist}/smr-${version}-darwin-${app_arch}-app.tar.gz" ]]; then
    for candidate in arm64 x86_64; do
      if [[ -f "${dist}/smr-${version}-darwin-${candidate}-app.tar.gz" ]]; then
        app_arch="$candidate"
        break
      fi
    done
  fi
  dmg="${dist}/SafeRoute_${version}_${arch}.dmg"
  if [[ ! -f "$dmg" ]]; then
    for candidate in \
      "${dist}/SafeRoute_${version}_aarch64.dmg" \
      "${dist}/SafeRoute_${version}_arm64.dmg" \
      "${dist}/SafeRoute_${version}_x86_64.dmg"; do
      if [[ -f "$candidate" ]]; then
        dmg="$candidate"
        break
      fi
    done
  fi
  if [[ ! -f "$dmg" ]]; then
    for candidate in "${root}/target/release/bundle/dmg/"*.dmg; do
      if [[ -f "$candidate" ]]; then
        dmg="$candidate"
        break
      fi
    done
  fi
  echo "VERSION=${version}"
  echo "DIST=${dist}"
  echo "ARCH=${arch}"
  echo "APP_ARCH=${app_arch}"
  echo "CLI_TAR=${dist}/smr-${version}-darwin-${arch}.tar.gz"
  echo "APP_TAR=${dist}/smr-${version}-darwin-${app_arch}-app.tar.gz"
  echo "DMG=${dmg}"
}

# curl + grep -q on large piped HTML can fail on macOS BSD grep (exit 23); use -c instead.
smr_curl_health_ok() {
  local base="${1:-http://127.0.0.1:8080}"
  curl -sf "${base}/health" 2>/dev/null | grep -q 'LLM-SafeRoute OK'
}

smr_curl_ui_ok() {
  local base="${1:-http://127.0.0.1:8080}"
  local n
  n="$(curl -sf "${base}/ui" 2>/dev/null | grep -c 'LLM-SafeRoute' 2>/dev/null || true)"
  [[ "${n:-0}" -gt 0 ]]
}
