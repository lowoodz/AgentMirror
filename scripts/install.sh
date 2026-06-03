#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

PREFIX="${SMR_INSTALL_PREFIX:-${HOME}/.local}"
BINDIR="${PREFIX}/bin"
CONFDIR="${PREFIX}/etc/securemodelroute"
INSTALL_SERVICE=false
INSTALL_GUI=false
INSTALL_ALL=false

for arg in "$@"; do
  case "$arg" in
    --service) INSTALL_SERVICE=true ;;
    --gui) INSTALL_GUI=true ;;
    --all) INSTALL_ALL=true ;;
  esac
done

if [[ "$INSTALL_ALL" == true ]]; then
  INSTALL_GUI=true
fi

echo "==> Building release..."
cargo build --release

DESKTOP_APP=""
if [[ "$INSTALL_GUI" == true || "$INSTALL_ALL" == true ]]; then
  if command -v npm >/dev/null && [[ -f "$ROOT/gui/package.json" ]]; then
    echo "==> Building desktop app (tray GUI, embeds server)"
    (cd "$ROOT/gui/src-tauri" && bash create_icon.sh)
    (cd "$ROOT/gui" && npm install --silent && CARGO_TARGET_DIR="${ROOT}/target" npm run build)
  APP_BUNDLE=""
  for name in SafeRoute.app SecureModelRoute.app; do
    if [[ -d "$ROOT/target/release/bundle/macos/${name}" ]]; then
      APP_BUNDLE="$ROOT/target/release/bundle/macos/${name}"
      break
    fi
  done
  if [[ -n "$APP_BUNDLE" ]] && [[ "$(uname -s)" == "Darwin" ]]; then
    APP_NAME="$(basename "$APP_BUNDLE")"
    DEST="${HOME}/Applications/${APP_NAME}"
    rm -rf "$DEST" "${HOME}/Applications/SafeRoute.app" "${HOME}/Applications/SecureModelRoute.app"
    cp -R "$APP_BUNDLE" "$DEST"
    DESKTOP_APP="$DEST"
    echo "    Installed desktop app: ${DEST}"
  else
    echo "Warning: SafeRoute.app not found after build" >&2
  fi
  else
    echo "Warning: npm or gui/ missing; extract *-darwin-*-app.tar.gz manually" >&2
  fi
elif command -v npm >/dev/null && [[ "${SMR_BUILD_GUI:-0}" == "1" ]]; then
  echo "==> Optional: build desktop GUI (SMR_BUILD_GUI=1)"
  (cd "$ROOT/gui/src-tauri" && bash create_icon.sh)
  (cd "$ROOT/gui" && npm install --silent && CARGO_TARGET_DIR="${ROOT}/target" npm run build)
  APP_BUNDLE=""
  for name in SafeRoute.app SecureModelRoute.app; do
    if [[ -d "$ROOT/target/release/bundle/macos/${name}" ]]; then
      APP_BUNDLE="$ROOT/target/release/bundle/macos/${name}"
      break
    fi
  done
  if [[ -n "$APP_BUNDLE" ]] && [[ "$(uname -s)" == "Darwin" ]]; then
    APP_NAME="$(basename "$APP_BUNDLE")"
    DEST="${HOME}/Applications/${APP_NAME}"
    rm -rf "$DEST" "${HOME}/Applications/SafeRoute.app" "${HOME}/Applications/SecureModelRoute.app"
    cp -R "$APP_BUNDLE" "$DEST"
    echo "    Installed desktop app: ${DEST}"
  fi
fi

echo "==> Installing to ${PREFIX}"
mkdir -p "${BINDIR}" "${CONFDIR}"
install -m 755 "${ROOT}/target/release/smr" "${BINDIR}/smr"

if [[ ! -f "${CONFDIR}/smr.yaml" ]]; then
  install -m 644 "${ROOT}/config/smr.example.yaml" "${CONFDIR}/smr.yaml"
  echo "    Created ${CONFDIR}/smr.yaml"
fi

LAUNCHER="${BINDIR}/securemodelroute"
cat > "${LAUNCHER}" << EOF
#!/usr/bin/env bash
exec "${BINDIR}/smr" --config "${CONFDIR}/smr.yaml" --open "\$@"
EOF
chmod +x "${LAUNCHER}"

# Headless LaunchAgent only when GUI is not installed (GUI embeds the server in the tray app).
if [[ "$INSTALL_SERVICE" == true && "$INSTALL_GUI" != true && "$(uname -s)" == "Darwin" ]]; then
  PLIST="${HOME}/Library/LaunchAgents/com.securemodelroute.smr.plist"
  mkdir -p "${HOME}/Library/LaunchAgents"
  cat > "${PLIST}" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.securemodelroute.smr</string>
  <key>ProgramArguments</key>
  <array>
    <string>${BINDIR}/smr</string>
    <string>--config</string>
    <string>${CONFDIR}/smr.yaml</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${CONFDIR}/smr.log</string>
  <key>StandardErrorPath</key><string>${CONFDIR}/smr.err.log</string>
</dict>
</plist>
EOF
  launchctl unload "${PLIST}" 2>/dev/null || true
  launchctl load "${PLIST}"
  echo "    LaunchAgent installed: ${PLIST}"
fi

if [[ -n "$DESKTOP_APP" && ("$INSTALL_ALL" == true || "$INSTALL_GUI" == true) && "$(uname -s)" == "Darwin" ]]; then
  # GUI embeds the HTTP server — disable conflicting headless LaunchAgent if present.
  HEADLESS_PLIST="${HOME}/Library/LaunchAgents/com.securemodelroute.smr.plist"
  if [[ -f "$HEADLESS_PLIST" ]]; then
    launchctl unload "$HEADLESS_PLIST" 2>/dev/null || true
    rm -f "$HEADLESS_PLIST"
    echo "    Removed headless LaunchAgent (conflicts with tray GUI on :8080)"
  fi
  pkill -f "${BINDIR}/smr --config" 2>/dev/null || true
  sleep 1

  PLIST="${HOME}/Library/LaunchAgents/com.securemodelroute.gui.plist"
  mkdir -p "${HOME}/Library/LaunchAgents"
  cat > "${PLIST}" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.securemodelroute.gui</string>
  <key>ProgramArguments</key>
  <array>
    <string>${DESKTOP_APP}/Contents/MacOS/smr-gui</string>
    <string>--background</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
</dict>
</plist>
EOF
  launchctl unload "${PLIST}" 2>/dev/null || true
  launchctl load "${PLIST}"
  echo "    Logon startup: ${PLIST} (--background, menu bar tray only)"
  open -a "$DESKTOP_APP" --args --background
  echo "    Tray app started"
fi

echo ""
echo "Installed:"
echo "  binary:   ${BINDIR}/smr"
echo "  launcher: ${LAUNCHER}"
echo "  config:   ${CONFDIR}/smr.yaml"
echo "  web UI:   http://127.0.0.1:8080/ui"
if [[ "$INSTALL_ALL" == true ]]; then
  echo "  mode:     full (CLI + tray GUI; close window to hide in menu bar)"
elif [[ "$INSTALL_GUI" == true ]]; then
  echo "  mode:     tray GUI (close window to hide in menu bar)"
fi
echo ""
echo "Run:  securemodelroute"
echo "Or:   smr --config ${CONFDIR}/smr.yaml --open"
echo ""
echo "Options:"
echo "  ./scripts/install.sh --all      # CLI + tray GUI + login autostart"
echo "  ./scripts/install.sh --gui      # tray GUI only (with CLI build)"
echo "  ./scripts/install.sh --service  # headless LaunchAgent only"
if [[ ":${PATH}:" != *":${BINDIR}:"* ]]; then
  echo "Add to PATH:  export PATH=\"${BINDIR}:\$PATH\""
fi
