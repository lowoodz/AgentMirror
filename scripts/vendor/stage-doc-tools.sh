#!/usr/bin/env bash
# Stage poppler pdftotext (+ runtime libs) for bundling into SafeRoute packages.
# Build hosts must provide poppler (macOS: brew install poppler; Linux: poppler-utils dev package).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-${ROOT}/resources/doc-tools}"
FORCE_ARCH="${2:-}"

usage() {
  cat <<'EOF'
Usage: scripts/vendor/stage-doc-tools.sh [OUT_DIR] [ARCH_LABEL]

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

if [[ -n "$FORCE_ARCH" ]]; then
  ARCH_LABEL="$FORCE_ARCH"
else
  ARCH="$(uname -m)"
  case "$ARCH" in
    arm64|aarch64) ARCH_LABEL="arm64" ;;
    x86_64|amd64) ARCH_LABEL="x86_64" ;;
    *)
      echo "Unsupported arch: $ARCH" >&2
      exit 2
      ;;
  esac
fi
case "$ARCH_LABEL" in
  arm64|x86_64) ;;
  *)
    echo "Unsupported ARCH_LABEL: $ARCH_LABEL (use arm64 or x86_64)" >&2
    exit 2
    ;;
esac

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
host_is_apple_silicon() {
  [[ "$(uname -s)" == "Darwin" ]] || return 1
  sysctl -n hw.optional.arm64 2>/dev/null | grep -qx 1
}
STAGE="${OUT}/${OS}-${ARCH_LABEL}"
BIN="${STAGE}/bin"
LIB="${STAGE}/lib"

rm -rf "$STAGE"
mkdir -p "$BIN" "$LIB"

pdftotext_matches_arch() {
  local bin="$1"
  local label="$2"
  [[ -x "$bin" ]] || return 1
  if ! command -v file >/dev/null 2>&1; then
    return 0
  fi
  local info
  info="$(file "$bin" 2>/dev/null || true)"
  case "$label" in
    arm64) [[ "$info" == *arm64* ]] ;;
    x86_64) [[ "$info" == *x86_64* ]] ;;
    *) return 1 ;;
  esac
}

find_pdftotext() {
  local candidates=()
  if [[ "$(uname -s)" == "Darwin" ]]; then
    if [[ "$ARCH_LABEL" == "arm64" ]]; then
      candidates=(/opt/homebrew/bin/pdftotext /usr/local/bin/pdftotext)
    else
      candidates=(/usr/local/bin/pdftotext /opt/homebrew/bin/pdftotext)
    fi
    if [[ "$ARCH_LABEL" == "arm64" && -x /opt/homebrew/bin/brew ]]; then
      prefix="$(/opt/homebrew/bin/brew --prefix poppler 2>/dev/null || true)"
      [[ -n "$prefix" && -x "${prefix}/bin/pdftotext" ]] && candidates=("${prefix}/bin/pdftotext" "${candidates[@]}")
    fi
    if command -v brew >/dev/null 2>&1; then
      prefix="$(brew --prefix poppler 2>/dev/null || true)"
      [[ -n "$prefix" && -x "${prefix}/bin/pdftotext" ]] && candidates=("${prefix}/bin/pdftotext" "${candidates[@]}")
    fi
  elif command -v pdftotext >/dev/null 2>&1; then
    candidates=("$(command -v pdftotext)")
  fi

  local p
  for p in "${candidates[@]}"; do
    if pdftotext_matches_arch "$p" "$ARCH_LABEL"; then
      echo "$p"
      return 0
    fi
  done
  # Apple Silicon host with Intel-only Homebrew: reuse x86_64 pdftotext for darwin-arm64
  # bundles (runs via Rosetta). Prefer native arm64: brew install poppler under /opt/homebrew.
  if [[ "$(uname -s)" == "Darwin" && "$ARCH_LABEL" == "arm64" && -x /usr/local/bin/pdftotext ]] \
    && host_is_apple_silicon \
    && file /usr/local/bin/pdftotext 2>/dev/null | grep -q x86_64; then
    echo "WARNING: darwin-arm64 uses x86_64 pdftotext (Rosetta); install /opt/homebrew poppler for native arm64." >&2
    echo /usr/local/bin/pdftotext
    return 0
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
  if [[ "$src" == */bin/pdftotext ]]; then
    poppler_prefix="$(cd "$(dirname "$src")/.." && pwd)"
  elif command -v brew >/dev/null 2>&1; then
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

  # Core poppler only (libpoppler.NNN.dylib), not libpoppler-cpp / libpoppler-glib.
  local core_poppler=""
  for candidate in "${LIB}"/libpoppler.[0-9]*.dylib; do
    [[ -f "$candidate" ]] || continue
    core_poppler="$candidate"
    break
  done

  rewrite_poppler_rpath() {
    local target="$1"
    local rel="${2:-@loader_path/../lib}"
    [[ -n "$core_poppler" ]] || return 0
    local base
    base="$(basename "$core_poppler")"
    while IFS= read -r dep; do
      [[ "$dep" == @rpath/libpoppler* ]] || continue
      install_name_tool -change "$dep" "${rel}/${base}" "$target" 2>/dev/null || true
    done < <(otool -L "$target" 2>/dev/null | tail -n +2 | awk '{print $1}' || true)
  }

  for lib in "${LIB}"/*.dylib; do
    [[ -f "$lib" ]] || continue
    rewrite_poppler_rpath "$lib" "@loader_path"
  done
  rewrite_poppler_rpath "${BIN}/pdftotext"
  install_name_tool -add_rpath @loader_path/../lib "${BIN}/pdftotext" 2>/dev/null || true

  # Brew poppler dylibs are often 0400; Tauri resource bundler must read them.
  find "${STAGE}" -type f -exec chmod a+r {} \;
  find "${STAGE}" -type d -exec chmod a+rx {} \;
  chmod 755 "${BIN}/pdftotext"
}

PDFTOTEXT="$(find_pdftotext)" || {
  echo "ERROR: pdftotext not found for darwin-${ARCH_LABEL}." >&2
  if [[ "$(uname -s)" == "Darwin" ]]; then
    echo "       Install poppler for this arch (e.g. brew install poppler; arm64 uses /opt/homebrew, x86_64 uses /usr/local)." >&2
  else
    echo "       Linux: install poppler-utils" >&2
  fi
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

# Smoke test: on arm64 hosts, run x86_64 staged binaries via Rosetta.
run_staged_pdftotext() {
  if [[ "$(uname -s)" == "Darwin" ]] && host_is_apple_silicon \
    && command -v arch >/dev/null 2>&1 \
    && file "${BIN}/pdftotext" 2>/dev/null | grep -q x86_64; then
    arch -x86_64 "$@"
  else
    "$@"
  fi
}
if ! run_staged_pdftotext "${BIN}/pdftotext" -v >/dev/null 2>&1; then
  echo "WARNING: staged pdftotext -v failed (may still work at runtime with bundled lib path)" >&2
fi

echo "==> staged $(du -sh "${STAGE}" | awk '{print $1}') at ${STAGE}"
if [[ -z "$FORCE_ARCH" ]]; then
  ln -sfn "${OS}-${ARCH_LABEL}" "${OUT}/current"
fi
ls -la "${BIN}" "${LIB}" 2>/dev/null | head -20 || true
