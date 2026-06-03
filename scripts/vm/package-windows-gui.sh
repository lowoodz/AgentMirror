#!/usr/bin/env bash
# Build Windows Tauri desktop on UTM guest; outputs to dist/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VM_ID="${SMR_UTM_VM:-Windows}"
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
BUILD_PS1="${ROOT}/scripts/vm/build-windows-desktop.ps1"
SRC_ZIP_GUEST="C:/Users/Public/smr-build-src.zip"
BUILD_PS1_GUEST="C:/Users/Public/build-windows-desktop.ps1"
TARGET_GUEST="C:/Users/Public/smr-gui-target.txt"
LOG_GUEST="C:/Users/Public/smr-desktop-build.log"
DIST="${ROOT}/dist"
VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
mkdir -p "$DIST"

if [[ ! -x "$UTMCTL" ]]; then
  echo "UTM not found" >&2
  exit 1
fi

# Probe guest Rust host triple.
PROBE_GUEST="C:/Users/Public/smr-gui-probe.txt"
"$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "rustc -vV 1>C:\\Users\\Public\\smr-gui-probe.txt 2>&1" 2>/dev/null || true
"$UTMCTL" file pull "$VM_ID" "$PROBE_GUEST" > "${DIST}/.smr-gui-probe.txt" 2>/dev/null || true
RUST_HOST="$(grep '^host:' "${DIST}/.smr-gui-probe.txt" 2>/dev/null | awk '{print $2}' || true)"

# Default: on ARM UTM VM build x86_64 desktop (GNU CLI + MSVC GUI parity).
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
if [[ -f "${ROOT}/gui/src-tauri/create_icon.sh" ]]; then
  (cd "${ROOT}/gui/src-tauri" && bash create_icon.sh) 2>/dev/null || true
fi
SRC_TAR="${DIST}/smr-windows-build-src.tar.gz"
TMP_DIR="$(mktemp -d)"
tar -czf "$SRC_TAR" -C "$ROOT" \
  --exclude=./dist --exclude=target --exclude=node_modules --exclude=.git \
  Cargo.toml Cargo.lock crates gui config README.md

SRC_ZIP="${DIST}/smr-windows-build-src.zip"
rm -f "$SRC_ZIP"
(
  cd "$TMP_DIR"
  tar -xzf "$SRC_TAR"
  zip -rq "$SRC_ZIP" .
)
rm -rf "$TMP_DIR"

echo "==> Upload source ($(du -h "$SRC_ZIP" | awk '{print $1}')) + build script"
cat "$SRC_ZIP" | "$UTMCTL" file push "$VM_ID" "$SRC_ZIP_GUEST"
cat "$BUILD_PS1" | "$UTMCTL" file push "$VM_ID" "$BUILD_PS1_GUEST"
echo -n "$GUI_TARGET" | "$UTMCTL" file push "$VM_ID" "$TARGET_GUEST"

echo "==> Clear previous build log on guest"
"$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "del /f C:\\Users\\Public\\smr-desktop-build.log 2>nul" 2>/dev/null || true
rm -f "${DIST}/windows-desktop-build.log"

echo "==> Build desktop app on guest (target: ${GUI_TARGET}, may take 15-30 min first time)..."
LAUNCH_PS1='C:/Users/Public/launch-desktop-build.ps1'
cat <<'PS1' | "$UTMCTL" file push "$VM_ID" "$LAUNCH_PS1"
$ps = if (Test-Path "$env:WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe") {
  "$env:WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
} else {
  "powershell.exe"
}
Start-Process -FilePath $ps -ArgumentList @(
  '-NoProfile','-ExecutionPolicy','Bypass','-File','C:/Users/Public/build-windows-desktop.ps1'
) -WindowStyle Hidden
PS1
"$UTMCTL" exec "$VM_ID" --cmd powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:/Users/Public/launch-desktop-build.ps1" 2>/dev/null || true

DEADLINE=$((SECONDS + 5400))
BUILD_STARTED=0
while (( SECONDS < DEADLINE )); do
  sleep 30
  "$UTMCTL" file pull "$VM_ID" "$LOG_GUEST" > "${DIST}/windows-desktop-build.log" 2>/dev/null || true
  if grep -q "==> Windows desktop (Tauri) build" "${DIST}/windows-desktop-build.log" 2>/dev/null; then
    BUILD_STARTED=1
  fi
  if grep -q "DESKTOP_BUILD_OK" "${DIST}/windows-desktop-build.log" 2>/dev/null; then
    break
  fi
  if [[ "$BUILD_STARTED" -eq 1 ]] && grep -E '^\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] ERROR:' "${DIST}/windows-desktop-build.log" 2>/dev/null | grep -q .; then
    cat "${DIST}/windows-desktop-build.log" >&2
    exit 1
  fi
  if [[ "$BUILD_STARTED" -eq 0 && "$SECONDS" -gt 600 ]]; then
    echo "Build did not start within 10 min (no log on guest). Check UTM guest agent." >&2
    tail -20 "${DIST}/windows-desktop-build.log" 2>/dev/null || true
    exit 1
  fi
  echo "... desktop build running (${SECONDS}s)"
done

"$UTMCTL" file pull "$VM_ID" "$LOG_GUEST" > "${DIST}/windows-desktop-build.log" 2>/dev/null || true

mkdir -p "${DIST}/windows-desktop"
for _pull in 1 2 3 4 5 6; do
  for guest_exe in "C:/Users/Public/smr-desktop-out/SecureModelRoute.exe" "C:/Users/Public/smr-desktop-out/SafeRoute.exe" "C:/Users/Public/smr-desktop-out/smr-gui.exe"; do
    "$UTMCTL" file pull "$VM_ID" "$guest_exe" > "${DIST}/windows-desktop/SecureModelRoute.exe" 2>/dev/null || true
    if [[ -s "${DIST}/windows-desktop/SecureModelRoute.exe" ]]; then
      break 2
    fi
  done
  sleep 10
done
SETUP=$("$UTMCTL" exec "$VM_ID" --cmd cmd.exe /c "dir /b C:\\Users\\Public\\smr-desktop-out\\*-setup.exe 2>nul" 2>/dev/null | head -1 || true)
if [[ -n "$SETUP" ]]; then
  "$UTMCTL" file pull "$VM_ID" "C:/Users/Public/smr-desktop-out/${SETUP}" > "${DIST}/windows-desktop/${SETUP}" 2>/dev/null || true
fi

if [[ ! -s "${DIST}/windows-desktop/SecureModelRoute.exe" ]]; then
  echo "Desktop build did not produce SecureModelRoute.exe" >&2
  tail -30 "${DIST}/windows-desktop-build.log" >&2 || true
  exit 1
fi

rm -f "$APP_ZIP"
(
  cd "${DIST}/windows-desktop"
  zip -q "$APP_ZIP" SecureModelRoute.exe
  [[ -n "$SETUP" && -f "$SETUP" ]] && zip -q "$APP_ZIP" "$SETUP"
)

echo "==> Desktop app package: $APP_ZIP"
ls -lh "$APP_ZIP" "${DIST}/windows-desktop/SecureModelRoute.exe"

# Symlink x86_64 name when building native x64 only; ARM64 is a separate artifact.
if [[ "$APP_SUFFIX" == "x86_64" ]]; then
  echo "    (x86_64 release desktop — matches smr-*-windows-x86_64.zip CLI)"
fi
