#!/usr/bin/env bash
# Stage MSVC runtime DLLs next to bundled Windows tools (pdftotext.exe needs them).
# Poppler-windows binaries link dynamically against VCRUNTIME140 / MSVCP140.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=windows-pe-x64.sh
source "$(cd "$(dirname "$0")" && pwd)/windows-pe-x64.sh"

BIN="${1:-${ROOT}/resources/doc-tools/windows-x64/bin}"
CACHE="${ROOT}/dist/vendor-cache"
VCRT_EXE="${CACHE}/vc_redist.x64.exe"
VCRT_URL="${SMR_VCRT_URL:-https://aka.ms/vs/17/release/vc_redist.x64.exe}"
VCRT_URL_FALLBACK="https://aka.ms/v14/vc_redist.x64.exe"
DLLS=(msvcp140.dll vcruntime140.dll vcruntime140_1.dll)

mkdir -p "$BIN" "$CACHE"

sanitize_windows_doc_tools_bin "$BIN"

# Re-stage when DLLs are missing or wrong architecture (e.g. ARM64 from an ARM VM System32).
for dll in "${DLLS[@]}"; do
  if [[ -f "${BIN}/${dll}" ]] && ! is_windows_pe_x64 "${BIN}/${dll}"; then
    echo "==> stage-vcrt-dlls: removing wrong-arch ${BIN}/${dll} ($(windows_pe_machine "${BIN}/${dll}"))" >&2
    rm -f "${BIN}/${dll}"
  fi
done

vcrt_staged_ok() {
  local dll
  for dll in "${DLLS[@]}"; do
    [[ -f "${BIN}/${dll}" ]] || return 1
    is_windows_pe_x64 "${BIN}/${dll}" || return 1
  done
  return 0
}

VCRT_CACHE="${CACHE}/vcrt-x64"
VENDOR_CRT="${ROOT}/resources/doc-tools/vendor-crt-x64"

copy_vcrt_from_dir() {
  local src_dir="$1"
  local label="$2"
  local dll
  for dll in "${DLLS[@]}"; do
    [[ -f "${src_dir}/${dll}" ]] || return 1
    is_windows_pe_x64 "${src_dir}/${dll}" || return 1
  done
  for dll in "${DLLS[@]}"; do
    cp -f "${src_dir}/${dll}" "${BIN}/${dll}"
    echo "==> staged ${BIN}/${dll} (from ${label})"
  done
  return 0
}

copy_vcrt_from_cache() {
  copy_vcrt_from_dir "$VCRT_CACHE" "$VCRT_CACHE"
}

copy_vcrt_from_vendor() {
  copy_vcrt_from_dir "$VENDOR_CRT" "$VENDOR_CRT"
}

save_vcrt_to_cache() {
  mkdir -p "$VCRT_CACHE" "$VENDOR_CRT"
  local dll
  for dll in "${DLLS[@]}"; do
    cp -f "${BIN}/${dll}" "${VCRT_CACHE}/${dll}"
    cp -f "${BIN}/${dll}" "${VENDOR_CRT}/${dll}"
  done
  echo "==> cached x64 VC++ DLLs at ${VCRT_CACHE} and ${VENDOR_CRT}"
}

if vcrt_staged_ok; then
  echo "==> stage-vcrt-dlls: x64 DLLs already present in ${BIN}"
  exit 0
fi

if copy_vcrt_from_cache; then
  exit 0
fi

if copy_vcrt_from_vendor; then
  save_vcrt_to_cache
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

# On ARM64 Windows, native VC++ DLLs live under System32 (ARM64). x64 copies for
# emulated x64 apps are under SysWOW64 — never copy System32 on ARM guests.
stage_from_vm_x64_dlls() {
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
  vm_scp_to "$VCRT_EXE" "${remote}/vc_redist.x64.exe"
  vm_ssh "powershell -NoProfile -Command \"\
\$d = '${remote}'; \
\$out = Join-Path \$d 'extract'; \
Remove-Item \$out -Recurse -Force -ErrorAction SilentlyContinue; \
New-Item -ItemType Directory -Force -Path \$out | Out-Null; \
\$exe = Join-Path \$d 'vc_redist.x64.exe'; \
\$p = Start-Process -FilePath \$exe -ArgumentList @('/extract:' + \$out, '/quiet', '/norestart') -Wait -PassThru; \
if (\$p.ExitCode -ne 0 -and \$p.ExitCode -ne 1638) { exit \$p.ExitCode }\"" || return 1
  local dll staged=0
  for dll in "${DLLS[@]}"; do
    local found
    found="$(vm_ssh "powershell -NoProfile -Command \"Get-ChildItem -Path '${remote}/extract' -Recurse -Filter '${dll}' -ErrorAction SilentlyContinue | ForEach-Object { \$_.FullName }\"" | tr -d '\r' | head -1)"
    [[ -n "$found" ]] || continue
    vm_scp_from "$found" "${BIN}/${dll}" || return 1
    if require_windows_pe_x64 "${BIN}/${dll}" "$dll"; then
      echo "==> staged ${BIN}/${dll} (from VM vc_redist extract)"
      staged=$((staged + 1))
    else
      rm -f "${BIN}/${dll}"
    fi
  done
  [[ "$staged" -eq ${#DLLS[@]} ]]
}

stage_vcrt_dll() {
  local dll="$1"
  local work="$2"
  local src
  src="$(find_x64_dll_in_tree "$work" "$dll")" || {
    echo "ERROR: no x64 ${dll} found after redist extract" >&2
    return 1
  }
  cp -f "$src" "${BIN}/${dll}"
  require_windows_pe_x64 "${BIN}/${dll}" "$dll"
  echo "==> staged ${BIN}/${dll}"
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
  echo "==> Local extract failed; trying Windows VM vc_redist /extract via SSH" >&2
  if stage_from_vm_x64_dlls; then
    save_vcrt_to_cache
    exit 0
  fi
  echo "ERROR: could not extract VC++ runtime DLLs from ${VCRT_EXE}" >&2
  echo "  Populate ${VENDOR_CRT}/ with x64 msvcp140/vcruntime140*.dll (from vc_redist.x64.exe /extract on Windows)," >&2
  echo "  or ensure UTM Windows VM SSH is up for automatic extract." >&2
  rm -rf "$work"
  exit 1
fi

for dll in "${DLLS[@]}"; do
  stage_vcrt_dll "$dll" "$work"
done

rm -rf "$work"
save_vcrt_to_cache

if ! vcrt_staged_ok; then
  echo "ERROR: VC++ runtime staging verification failed in ${BIN}" >&2
  exit 1
fi
