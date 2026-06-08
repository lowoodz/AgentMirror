#!/usr/bin/env bash
# Build Windows Tauri desktop on UTM guest via windows-user SSH only; outputs to dist/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=vm-ssh.sh
source "${ROOT}/scripts/vm/vm-ssh.sh"

BUILD_PS1="${ROOT}/scripts/vm/build-windows-desktop.ps1"
DIST="${ROOT}/dist"
VERSION="$(grep '^version' "${ROOT}/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
mkdir -p "$DIST"

vm_ssh_init
vm_ssh_require
STAGING="${GUEST_STAGING}"
SRC_ZIP_GUEST="${STAGING}/smr-build-src.zip"
BUILD_PS1_GUEST="${STAGING}/build-windows-desktop.ps1"
TARGET_GUEST="${STAGING}/smr-gui-target.txt"
LOG_GUEST="${STAGING}/smr-desktop-build.log"
PROBE_GUEST="${STAGING}/smr-gui-probe.txt"
OUT_GUEST="${STAGING}/smr-desktop-out"

echo "==> Windows desktop build on $VM_SSH (staging: ${STAGING})"
trap vm_ssh_close EXIT

# Probe guest Rust host triple (windows-user profile; default x86_64 if probe fails).
vm_ssh "powershell -NoProfile -Command \"\$s='${STAGING}'; \$p=Join-Path \$env:USERPROFILE '.cargo/bin/rustc.exe'; \$out=Join-Path \$s 'smr-gui-probe.txt'; if (Test-Path \$p) { & \$p -vV | Set-Content \$out -Encoding utf8 } else { 'host: unknown' | Set-Content \$out -Encoding utf8 }\"" || true
vm_scp_from "$PROBE_GUEST" "${DIST}/.smr-gui-probe.txt" 2>/dev/null || true
RUST_HOST="$(grep '^host:' "${DIST}/.smr-gui-probe.txt" 2>/dev/null | awk '{print $2}' || true)"

export SMR_WINDOWS_GUI_TARGET="${SMR_WINDOWS_GUI_TARGET:-x86_64-pc-windows-msvc}"
GUI_TARGET="$SMR_WINDOWS_GUI_TARGET"
APP_SUFFIX="x86_64"
if [[ -n "$RUST_HOST" && "$RUST_HOST" == aarch64-* ]]; then
  if [[ "${SMR_FORCE_GUI_CROSS:-0}" == "1" || "${SMR_WINDOWS_GUI_TARGET:-}" == "x86_64-pc-windows-msvc" ]]; then
    echo "==> ARM guest: cross-building x86_64 desktop"
    GUI_TARGET="x86_64-pc-windows-msvc"
    APP_SUFFIX="x86_64"
  else
    echo "==> ARM guest: native ARM64 desktop build"
    GUI_TARGET="aarch64-pc-windows-msvc"
    APP_SUFFIX="arm64"
  fi
elif [[ -n "$RUST_HOST" && "$RUST_HOST" != x86_64-* ]]; then
  echo "Unsupported guest host: ${RUST_HOST}" >&2
  exit 1
fi

APP_ZIP="${DIST}/smr-${VERSION}-windows-${APP_SUFFIX}-app.zip"

echo "==> Pack minimal source for Windows GUI build"
bash "${ROOT}/scripts/sync-admin-ui.sh"
if [[ -f "${ROOT}/gui/src-tauri/create_icon.sh" ]]; then
  (cd "${ROOT}/gui/src-tauri" && bash create_icon.sh) 2>/dev/null || true
fi
SRC_TAR="${DIST}/smr-windows-build-src.tar.gz"
TMP_DIR="$(mktemp -d)"
tar -czf "$SRC_TAR" -C "$ROOT" \
  --exclude=./dist --exclude=target --exclude=node_modules --exclude=.git \
  Cargo.toml Cargo.lock crates gui config scripts README.md

SRC_ZIP="${DIST}/smr-windows-build-src.zip"
rm -f "$SRC_ZIP"
(
  cd "$TMP_DIR"
  tar -xzf "$SRC_TAR"
  zip -rq "$SRC_ZIP" .
)
rm -rf "$TMP_DIR"

echo "==> Upload source ($(du -h "$SRC_ZIP" | awk '{print $1}')) + build script to ${STAGING}"
vm_scp_to "$SRC_ZIP" "$SRC_ZIP_GUEST"
vm_scp_to "$BUILD_PS1" "$BUILD_PS1_GUEST"
echo -n "$GUI_TARGET" > "${DIST}/.smr-gui-target.txt"
vm_scp_to "${DIST}/.smr-gui-target.txt" "$TARGET_GUEST"

echo "==> Clear previous build log on guest"
LOG_WIN="${LOG_GUEST//\//\\}"
vm_ssh "cmd.exe /c \"del /q ${LOG_WIN} 2>nul & exit /b 0\"" 2>/dev/null || true
rm -f "${DIST}/windows-desktop-build.log"
BUILD_LOG_MARKER="==> Windows desktop (Tauri) build"

echo "==> Build desktop app as windows-user (target: ${GUI_TARGET}, may take 15-30 min first time)..."
vm_ssh_bg "cmd.exe /c \"set SMR_GUEST_STAGING=${STAGING}&& set SMR_WINDOWS_USER=${VM_USER}&& powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${BUILD_PS1_GUEST}\""

DEADLINE=$((SECONDS + 5400))
BUILD_STARTED=0
while (( SECONDS < DEADLINE )); do
  sleep 30
  vm_scp_from "$LOG_GUEST" "${DIST}/windows-desktop-build.log" 2>/dev/null || true
  if grep -q "==> Windows desktop (Tauri) build" "${DIST}/windows-desktop-build.log" 2>/dev/null; then
    BUILD_STARTED=1
  fi
  if [[ "$BUILD_STARTED" -eq 1 ]] && grep -q "DESKTOP_BUILD_OK" "${DIST}/windows-desktop-build.log" 2>/dev/null; then
    break
  fi
  if [[ "$BUILD_STARTED" -eq 1 ]]; then
    if awk -v marker="$BUILD_LOG_MARKER" '
      $0 ~ marker { seen=1; err=0; next }
      seen && /^\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] ERROR:/ { err=1 }
      END { exit err ? 0 : 1 }
    ' "${DIST}/windows-desktop-build.log" 2>/dev/null; then
      cat "${DIST}/windows-desktop-build.log" >&2
      exit 1
    fi
  fi
  if [[ "$BUILD_STARTED" -eq 0 && "$SECONDS" -gt 900 ]]; then
    echo "Build did not start within 15 min (no log under ${STAGING})." >&2
    tail -20 "${DIST}/windows-desktop-build.log" 2>/dev/null || true
    exit 1
  fi
  echo "... desktop build running (${SECONDS}s)"
done

wait "${VM_SSH_BG_PID:-0}" 2>/dev/null || true

vm_scp_from "$LOG_GUEST" "${DIST}/windows-desktop-build.log" 2>/dev/null || true

mkdir -p "${DIST}/windows-desktop"
STABLE_SETUP="SafeRoute_${VERSION}_x64-setup.exe"
for _pull in 1 2 3 4 5 6; do
  vm_scp_from "${OUT_GUEST}/SafeRoute.exe" "${DIST}/windows-desktop/SafeRoute.exe" 2>/dev/null || \
    vm_scp_from "${OUT_GUEST}/smr-gui.exe" "${DIST}/windows-desktop/SafeRoute.exe" 2>/dev/null || true
  [[ -s "${DIST}/windows-desktop/SafeRoute.exe" ]] && break
  sleep 10
done

SETUP=""
for _pull in 1 2 3 4 5 6; do
  SETUP="$(vm_ssh "powershell -NoProfile -Command \"Get-ChildItem '${OUT_GUEST}/*-setup.exe' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name -First 1\"" 2>/dev/null | tr -d '\r' | head -1 || true)"
  if [[ -n "$SETUP" ]]; then
    vm_scp_from "${OUT_GUEST}/${SETUP}" "${DIST}/windows-desktop/${SETUP}" 2>/dev/null || true
    [[ -f "${DIST}/windows-desktop/${SETUP}" ]] && cp "${DIST}/windows-desktop/${SETUP}" "${DIST}/${STABLE_SETUP}" && break
  fi
  vm_scp_from "${OUT_GUEST}/${STABLE_SETUP}" "${DIST}/${STABLE_SETUP}" 2>/dev/null || true
  [[ -f "${DIST}/${STABLE_SETUP}" ]] && break
  sleep 10
done

if [[ ! -s "${DIST}/windows-desktop/SafeRoute.exe" ]]; then
  echo "Desktop build did not produce SafeRoute.exe" >&2
  tail -30 "${DIST}/windows-desktop-build.log" >&2 || true
  exit 1
fi

if [[ ! -f "${DIST}/${STABLE_SETUP}" ]]; then
  echo "ERROR: NSIS setup.exe not produced (expected ${STABLE_SETUP})" >&2
  grep -iE 'nsis|makensis|ERROR' "${DIST}/windows-desktop-build.log" 2>/dev/null | tail -20 >&2 || true
  exit 1
fi

rm -f "$APP_ZIP"
(
  cd "${DIST}/windows-desktop"
  zip -q "$APP_ZIP" SafeRoute.exe
  [[ -n "$SETUP" && -f "$SETUP" ]] && zip -q "$APP_ZIP" "$SETUP"
  cp "${DIST}/${STABLE_SETUP}" "${DIST}/windows-desktop/${STABLE_SETUP}"
  zip -q "$APP_ZIP" "${STABLE_SETUP}"
)

echo "==> Desktop app package: $APP_ZIP"
ls -lh "$APP_ZIP" "${DIST}/windows-desktop/SafeRoute.exe" "${DIST}/${STABLE_SETUP}" 2>/dev/null || ls -lh "$APP_ZIP" "${DIST}/windows-desktop/SafeRoute.exe"

if [[ "$APP_SUFFIX" == "x86_64" ]]; then
  echo "    (x86_64 release desktop — matches smr-*-windows-x86_64.zip CLI)"
fi

# shellcheck source=../dist-layout.sh
source "${ROOT}/scripts/dist-layout.sh"
dist_write_manifest
