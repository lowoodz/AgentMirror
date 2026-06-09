#!/usr/bin/env bash
# Stage poppler pdftotext (+ runtime libs) for bundling into SafeRoute packages.
# Build hosts must provide poppler (macOS: brew install poppler; Linux: poppler-utils dev package).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-${ROOT}/resources/doc-tools}"

usage() {
  cat <<'EOF'
Usage: scripts/vendor/stage-doc-tools.sh [OUT_DIR]

Stages platform-specific doc-tools/ (bin/pdftotext + lib/*) for:
  - macOS CLI tar (tools/)
  - macOS .app Resources (via Tauri resources)
  - manual SMR_TOOLS_DIR overrides

Windows hosts should run scripts/windows/stage-doc-tools.ps1 instead.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

case "$(uname -s)" in
  Darwin) ;;
  Linux) ;;
  *)
    echo "stage-doc-tools.sh: use scripts/windows/stage-doc-tools.ps1 on Windows" >&2
    exit 2
    ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
  arm64|aarch64) ARCH_LABEL="arm64" ;;
  x86_64|amd64) ARCH_LABEL="x86_64" ;;
  *)
    echo "Unsupported arch: $ARCH" >&2
    exit 2
    ;;
esac

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
STAGE="${OUT}/${OS}-${ARCH_LABEL}"
BIN="${STAGE}/bin"
LIB="${STAGE}/lib"

rm -rf "$STAGE"
mkdir -p "$BIN" "$LIB"

find_pdftotext() {
  if command -v pdftotext >/dev/null 2>&1; then
    command -v pdftotext
    return 0
  fi
  if [[ "$(uname -s)" == "Darwin" ]] && command -v brew >/dev/null 2>&1; then
    local prefix
    prefix="$(brew --prefix poppler 2>/dev/null || true)"
    if [[ -n "$prefix" && -x "${prefix}/bin/pdftotext" ]]; then
      echo "${prefix}/bin/pdftotext"
      return 0
    fi
  fi
  return 1
}

copy_non_system_dylib() {
  local src="$1"
  [[ -f "$src" ]] || return 0
  case "$src" in
    /usr/lib/*|/System/*|/lib/libSystem*|/lib/libc.so*|/lib/ld-linux*) return 0 ;;
  esac
  local base
  base="$(basename "$src")"
  if [[ ! -f "${LIB}/${base}" ]]; then
    cp -f "$src" "${LIB}/${base}"
    chmod 644 "${LIB}/${base}" || true
  fi
  echo "${LIB}/${base}"
}

bundle_linux_pdftotext() {
  local src="$1"
  cp -f "$src" "${BIN}/pdftotext"
  chmod 755 "${BIN}/pdftotext"

  local needed
  if command -v ldd >/dev/null 2>&1; then
    while IFS= read -r line; do
      local dep="${line#*=>}"
      dep="${dep%% *}"
      dep="${dep// /}"
      [[ -n "$dep" && "$dep" == /* ]] || continue
      copy_non_system_dylib "$dep" >/dev/null || true
    done < <(ldd "$src" 2>/dev/null | grep '=> /' || true)
  fi
}

bundle_macos_pdftotext() {
  local src="$1"
  local poppler_prefix=""
  if command -v brew >/dev/null 2>&1; then
    poppler_prefix="$(brew --prefix poppler 2>/dev/null || true)"
  fi

  cp -f "$src" "${BIN}/pdftotext"
  chmod 755 "${BIN}/pdftotext"

  if [[ -n "$poppler_prefix" && -d "${poppler_prefix}/lib" ]]; then
    cp -f "${poppler_prefix}/lib"/libpoppler*.dylib "${LIB}/" 2>/dev/null || true
    chmod 644 "${LIB}"/libpoppler*.dylib 2>/dev/null || true
  fi

  local queue=()
  for lib in "${LIB}"/*.dylib; do
    [[ -f "$lib" ]] && queue+=("$lib")
  done
  queue+=("${BIN}/pdftotext")

  local guard=0
  while ((${#queue[@]} > 0 && guard < 64)); do
    guard=$((guard + 1))
    local target="${queue[0]}"
    queue=("${queue[@]:1}")
    while IFS= read -r dep; do
      [[ -n "$dep" ]] || continue
      local resolved=""
      case "$dep" in
        /usr/lib/*|/System/*) continue ;;
        @rpath/*)
          local name="${dep#@rpath/}"
          if [[ -n "$poppler_prefix" && -f "${poppler_prefix}/lib/${name}" ]]; then
            resolved="${poppler_prefix}/lib/${name}"
            cp -f "$resolved" "${LIB}/${name}"
            chmod 644 "${LIB}/${name}" 2>/dev/null || true
            queue+=("${LIB}/${name}")
          fi
          ;;
        /*)
          resolved="$dep"
          if [[ -f "$resolved" ]]; then
            local copied
            copied="$(copy_non_system_dylib "$resolved")"
            [[ -n "$copied" ]] && queue+=("$copied")
          fi
          ;;
      esac
    done < <(otool -L "$target" 2>/dev/null | tail -n +2 | awk '{print $1}' || true)
  done

  for lib in "${LIB}"/*.dylib; do
    [[ -f "$lib" ]] || continue
    install_name_tool -id "@loader_path/$(basename "$lib")" "$lib" 2>/dev/null || true
  done

  local poppler_lib=""
  poppler_lib="$(ls "${LIB}"/libpoppler.[0-9]*.dylib 2>/dev/null | head -1 || true)"
  if [[ -n "$poppler_lib" ]]; then
    install_name_tool -change @rpath/libpoppler.159.dylib "@loader_path/../lib/$(basename "$poppler_lib")" "${BIN}/pdftotext" 2>/dev/null || true
    install_name_tool -change @rpath/libpoppler.dylib "@loader_path/../lib/$(basename "$poppler_lib")" "${BIN}/pdftotext" 2>/dev/null || true
  fi
  install_name_tool -add_rpath @loader_path/../lib "${BIN}/pdftotext" 2>/dev/null || true

  # Brew poppler dylibs are often 0400; Tauri resource bundler must read them.
  find "${STAGE}" -type f -exec chmod a+r {} \;
  find "${STAGE}" -type d -exec chmod a+rx {} \;
  chmod 755 "${BIN}/pdftotext"
}

PDFTOTEXT="$(find_pdftotext)" || {
  echo "ERROR: pdftotext not found. macOS: brew install poppler; Linux: install poppler-utils" >&2
  exit 1
}

echo "==> stage-doc-tools: ${STAGE}"
echo "    source pdftotext: ${PDFTOTEXT}"

case "$(uname -s)" in
  Darwin) bundle_macos_pdftotext "$PDFTOTEXT" ;;
  Linux) bundle_linux_pdftotext "$PDFTOTEXT" ;;
esac

if [[ ! -x "${BIN}/pdftotext" ]]; then
  echo "ERROR: staged pdftotext missing" >&2
  exit 1
fi

# Smoke test
if ! "${BIN}/pdftotext" -v >/dev/null 2>&1; then
  echo "WARNING: staged pdftotext -v failed (may still work at runtime with bundled lib path)" >&2
fi

echo "==> staged $(du -sh "${STAGE}" | awk '{print $1}') at ${STAGE}"
ln -sfn "${OS}-${ARCH_LABEL}" "${OUT}/current"
ls -la "${BIN}" "${LIB}" 2>/dev/null | head -20 || true
