#!/usr/bin/env bash
# Helpers: verify Windows PE binaries staged for windows-x64 bundles are AMD64 (not ARM64/x86).
set -euo pipefail

# Machine field at PE+4: 0x8664 = AMD64, 0x014c = i386, 0xAA64 = ARM64
windows_pe_machine() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "missing"
    return 0
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$path" <<'PY'
import struct, sys
path = sys.argv[1]
try:
    with open(path, "rb") as f:
        b = f.read()
except OSError:
    print("missing")
    raise SystemExit(0)
if len(b) < 64 or b[:2] != b"MZ":
    print("invalid")
    raise SystemExit(0)
pe = struct.unpack_from("<I", b, 0x3C)[0]
if pe + 6 > len(b):
    print("invalid")
    raise SystemExit(0)
machine = struct.unpack_from("<H", b, pe + 4)[0]
print({0x8664: "x64", 0x014C: "x86", 0xAA64: "arm64"}.get(machine, f"0x{machine:04x}"))
PY
    return 0
  fi
  local info
  info="$(file -b "$path" 2>/dev/null || true)"
  if [[ "$info" =~ x86-64|x86_64|PE32\+.*64-bit ]]; then
    echo "x64"
  elif [[ "$info" =~ ARM64|AArch64|aarch64 ]]; then
    echo "arm64"
  elif [[ "$info" =~ 80386|i386|PE32[^+] ]]; then
    echo "x86"
  elif [[ "$info" =~ PE32|executable ]]; then
    echo "pe-other"
  else
    echo "invalid"
  fi
}

is_windows_pe_x64() {
  [[ "$(windows_pe_machine "$1")" == "x64" ]]
}

require_windows_pe_x64() {
  local path="$1"
  local label="${2:-$(basename "$path")}"
  if [[ ! -f "$path" ]]; then
    echo "ERROR: missing ${label} (${path})" >&2
    return 1
  fi
  local machine
  machine="$(windows_pe_machine "$path")"
  if [[ "$machine" != "x64" ]]; then
    echo "ERROR: ${label} is ${machine}, expected x64 for windows-x64 bundle (${path})" >&2
    return 1
  fi
  return 0
}

# Drop extensionless pdftotext (macOS/Linux artifact) from Windows bin dirs.
sanitize_windows_doc_tools_bin() {
  local bin="${1:?bin directory}"
  if [[ -f "${bin}/pdftotext" && ! "${bin}/pdftotext" =~ \.exe$ ]]; then
    echo "==> removing non-Windows pdftotext from ${bin}" >&2
    rm -f "${bin}/pdftotext"
  fi
}

find_x64_dll_in_tree() {
  local root="$1"
  local name="$2"
  local candidate
  while IFS= read -r candidate; do
    if is_windows_pe_x64 "$candidate"; then
      echo "$candidate"
      return 0
    fi
  done < <(find "$root" -iname "$name" -type f 2>/dev/null)
  return 1
}

verify_windows_doc_tools_bin() {
  local bin="${1:?bin directory}"
  sanitize_windows_doc_tools_bin "$bin"
  require_windows_pe_x64 "${bin}/pdftotext.exe" "pdftotext.exe"
  local dll
  for dll in msvcp140.dll vcruntime140.dll vcruntime140_1.dll; do
    require_windows_pe_x64 "${bin}/${dll}" "$dll"
  done
  if [[ -f "${bin}/poppler.dll" ]]; then
    require_windows_pe_x64 "${bin}/poppler.dll" "poppler.dll"
  fi
  echo "==> verify windows-x64 doc-tools: ${bin} ok"
}
