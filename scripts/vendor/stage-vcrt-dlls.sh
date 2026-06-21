#!/usr/bin/env bash
# Stage MSVC runtime DLLs next to bundled Windows tools (pdftotext.exe needs them).
# Poppler-windows binaries link dynamically against VCRUNTIME140 / MSVCP140.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${1:-${ROOT}/resources/doc-tools/windows-x64/bin}"
CACHE="${ROOT}/dist/vendor-cache"
VCRT_EXE="${CACHE}/vc_redist.x64.exe"
VCRT_URL="${SMR_VCRT_URL:-https://aka.ms/vs/17/release/vc_redist.x64.exe}"
VCRT_URL_FALLBACK="https://aka.ms/v14/vc_redist.x64.exe"
DLLS=(msvcp140.dll vcruntime140.dll vcruntime140_1.dll)

mkdir -p "$BIN" "$CACHE"

missing=()
for dll in "${DLLS[@]}"; do
  if [[ ! -f "${BIN}/${dll}" ]]; then
    missing+=("$dll")
  fi
done
if [[ ${#missing[@]} -eq 0 ]]; then
  echo "==> stage-vcrt-dlls: already present in ${BIN}"
  exit 0
fi

find7z() {
  command -v 7z >/dev/null 2>&1 && { echo 7z; return; }
  command -v 7zz >/dev/null 2>&1 && { echo 7zz; return; }
  command -v /opt/homebrew/bin/7zz >/dev/null 2>&1 && { echo /opt/homebrew/bin/7zz; return; }
  command -v /usr/local/bin/7zz >/dev/null 2>&1 && { echo /usr/local/bin/7zz; return; }
  return 1
}

ensure_7z() {
  if seven="$(find7z)"; then
    echo "$seven"
    return 0
  fi
  if command -v brew >/dev/null 2>&1; then
    echo "==> Installing p7zip (required to bundle VC++ runtime DLLs)" >&2
    brew install p7zip >&2
  fi
  find7z
}

extract_with_7z() {
  local seven="$1"
  local work="$2"
  rm -rf "$work"
  mkdir -p "$work"
  (cd "$work" && "$seven" x -y "$VCRT_EXE") >/dev/null
  local cab
  cab="$(find "$work" -maxdepth 1 -name 'vc_redist*.cab' -print -quit)"
  [[ -n "$cab" ]] || { echo "vc_redist.cab not found inside ${VCRT_EXE}" >&2; return 1; }
  (cd "$work" && "$seven" x -y "$(basename "$cab")") >/dev/null
}

extract_with_innoextract() {
  local work="$1"
  rm -rf "$work"
  mkdir -p "$work"
  (cd "$work" && innoextract -e -d . "$VCRT_EXE") >/dev/null
}

ensure_innoextract() {
  if command -v innoextract >/dev/null 2>&1; then
    echo innoextract
    return 0
  fi
  if command -v brew >/dev/null 2>&1; then
    echo "==> Installing innoextract (extract VC++ redist on macOS)" >&2
    brew install innoextract >&2 || true
  fi
  command -v innoextract >/dev/null 2>&1 && echo innoextract && return 0
  return 1
}

stage_from_vm_system32() {
  local root script remote
  root="$(cd "$(dirname "$0")/../.." && pwd)"
  script="${root}/scripts/vm/vm-ssh.sh"
  [[ -f "$script" ]] || return 1
  # shellcheck source=vm/vm-ssh.sh
  source "$script"
  vm_ssh_init
  vm_ssh_require 2>/dev/null || return 1
  remote="${GUEST_STAGING}/vcrt-stage"
  vm_ssh_mkdir "$remote"
  vm_ssh "powershell -NoProfile -Command \"\$d='${remote}'; New-Item -ItemType Directory -Force -Path \$d | Out-Null; Copy-Item 'C:\\Windows\\System32\\msvcp140.dll' -Destination \$d -Force; Copy-Item 'C:\\Windows\\System32\\vcruntime140.dll' -Destination \$d -Force; Copy-Item 'C:\\Windows\\System32\\vcruntime140_1.dll' -Destination \$d -Force\"" || return 1
  local dll
  for dll in "${DLLS[@]}"; do
    vm_scp_from "${remote}/${dll}" "${BIN}/${dll}" || return 1
    echo "==> staged ${BIN}/${dll} (from VM System32)"
  done
  return 0
}

if [[ ! -f "$VCRT_EXE" ]]; then
  echo "==> Download Microsoft VC++ 2015-2022 redist (x64)"
  if ! curl -fsSL -o "$VCRT_EXE" "$VCRT_URL"; then
    curl -fsSL -o "$VCRT_EXE" "$VCRT_URL_FALLBACK"
  fi
fi
if ! file "$VCRT_EXE" 2>/dev/null | grep -q 'PE32'; then
  echo "ERROR: ${VCRT_EXE} is not a Windows PE (download may have redirected to HTML)." >&2
  echo "  Set SMR_VCRT_URL to a direct vc_redist.x64.exe URL." >&2
  rm -f "$VCRT_EXE"
  exit 1
fi

work="${CACHE}/vcrt-extract-$$"
extracted=0
if tool="$(ensure_innoextract 2>/dev/null)"; then
  echo "==> Extract VC++ runtime DLLs with ${tool}"
  if extract_with_innoextract "$work"; then
    extracted=1
  fi
fi
if [[ "$extracted" -eq 0 ]] && seven="$(ensure_7z 2>/dev/null || true)"; then
  echo "==> Extract VC++ runtime DLLs with ${seven} (legacy cab layout)"
  if extract_with_7z "$seven" "$work"; then
    extracted=1
  fi
fi
if [[ "$extracted" -eq 0 ]] && [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* || -n "${OS:-}" ]]; then
  echo "==> Extract VC++ runtime DLLs via /extract (Windows host)"
  rm -rf "$work"
  mkdir -p "$work"
  "${VCRT_EXE}" /extract:"${work}" /q
  extracted=1
fi
if [[ "$extracted" -eq 0 ]]; then
  echo "==> Local extract failed; trying Windows VM System32 via SSH" >&2
  if stage_from_vm_system32; then
    rm -rf "$work"
    exit 0
  fi
  echo "ERROR: could not extract VC++ runtime DLLs from ${VCRT_EXE}" >&2
  echo "  macOS: brew install innoextract   or   ensure UTM VM SSH is up" >&2
  rm -rf "$work"
  exit 1
fi

for dll in "${DLLS[@]}"; do
  src="$(find "$work" -iname "$dll" -print -quit)"
  if [[ -z "$src" || ! -f "$src" ]]; then
    echo "ERROR: ${dll} not found after redist extract" >&2
    rm -rf "$work"
    exit 1
  fi
  cp -f "$src" "${BIN}/${dll}"
  echo "==> staged ${BIN}/${dll}"
done

rm -rf "$work"
