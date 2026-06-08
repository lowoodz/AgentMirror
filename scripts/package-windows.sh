#!/usr/bin/env bash
# Build Windows x86_64 release package from macOS/Linux (cross-compile).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

TARGET="${SMR_WINDOWS_TARGET:-x86_64-pc-windows-gnu}"

linker=""
if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
  linker="mingw"
elif command -v zig >/dev/null 2>&1; then
  linker="zig"
elif [[ -x /tmp/zig-macos-aarch64-0.14.0/zig ]]; then
  export PATH="/tmp/zig-macos-aarch64-0.14.0:${PATH}"
  linker="zig"
fi

if [[ -z "${linker}" ]]; then
  echo "Error: no Windows cross linker found."
  echo "  macOS: brew install mingw-w64"
  echo "  or install Zig: https://ziglang.org/download/"
  exit 1
fi

if [[ "${linker}" == "zig" ]]; then
  echo "==> Using cargo-zigbuild for Windows cross compile"
else
  mkdir -p "${ROOT}/.cargo"
  cat > "${ROOT}/.cargo/config.toml" <<EOF
[target.${TARGET}]
linker = "x86_64-w64-mingw32-gcc"
EOF
  echo "==> Using mingw-w64 as Windows cross linker"
fi

echo "==> Adding Rust target ${TARGET}"
rustup target add "${TARGET}"

echo "==> Building SafeRoute for ${TARGET} (release)"
if [[ "${linker}" == "zig" ]]; then
  cargo zigbuild --release --target "${TARGET}" -p smr-cli
else
  cargo build --release --target "${TARGET}" -p smr-cli
fi

BIN="${ROOT}/target/${TARGET}/release/smr.exe"
OUT="${ROOT}/dist"
mkdir -p "${OUT}"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
PKG="smr-${VERSION}-windows-x86_64"

cp "${BIN}" "${OUT}/smr.exe"
cp config/smr.example.yaml "${OUT}/smr.example.yaml"
cp README.md "${OUT}/README.md"
cp scripts/install.ps1 "${OUT}/install.ps1"
cp scripts/uninstall.ps1 "${OUT}/uninstall.ps1"
cp scripts/verify.ps1 "${OUT}/verify.ps1"

ZIP="${OUT}/${PKG}.zip"
rm -f "${ZIP}"
(
  cd "${OUT}"
  zip -q "${PKG}.zip" smr.exe smr.example.yaml README.md install.ps1 uninstall.ps1 verify.ps1
)

echo "==> Package: ${ZIP}"
echo "==> Binary:  ${OUT}/smr.exe"
ls -lh "${ZIP}" "${OUT}/smr.exe"

# Optional Tauri desktop (built on Windows VM via windows-user SSH)
if [[ "${SMR_BUILD_WINDOWS_GUI:-0}" == "1" ]]; then
  # shellcheck source=vm/vm-ssh.sh
  source "${ROOT}/scripts/vm/vm-ssh.sh"
  vm_ssh_init
  if ssh "${VM_SSH_OPTS[@]}" "$VM_SSH" "echo ok" >/dev/null 2>&1; then
    echo ""
    bash "${ROOT}/scripts/vm/package-windows-gui.sh" || echo "Warning: Windows desktop app build failed (see dist/windows-desktop-build.log)"
  else
    echo "Warning: SMR_BUILD_WINDOWS_GUI=1 but SSH to $VM_SSH unavailable; skip app zip"
    echo "  On Windows host: .\\scripts\\package.ps1"
  fi
fi
