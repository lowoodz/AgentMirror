#!/usr/bin/env bash
# Install from dist packages, launch tray GUI, run black-box tests (macOS host + optional Windows UTM).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.cargo/bin:${PATH}"
LOG_DIR="${ROOT}/dist/test-runs"
mkdir -p "$LOG_DIR"
STAMP="$(date +%Y%m%d-%H%M%S)"
SUMMARY="${LOG_DIR}/installed-app-${STAMP}.log"
failures=0

if [[ ! -f test_model_api_key.txt ]]; then
  echo "Missing test_model_api_key.txt — copy from test_model_api_key.example.txt (gitignored)" >&2
  exit 1
fi

run_step() {
  local name="$1"
  shift
  echo ""
  echo "################################################################"
  echo "# ${name}"
  echo "################################################################"
  set +e
  "$@" 2>&1 | tee "${LOG_DIR}/${STAMP}-${name// /-}.log"
  local rc=${PIPESTATUS[0]}
  set -e
  if [[ "$rc" -eq 0 ]]; then
    echo ">>> ${name}: PASSED" | tee -a "$SUMMARY"
  else
    echo ">>> ${name}: FAILED" | tee -a "$SUMMARY"
    failures=$((failures + 1))
  fi
}

mac_installed_app_test() {
  local version arch app_tar cli_tar
  version="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
  if file "${ROOT}/target/release/bundle/macos/SecureModelRoute.app/Contents/MacOS/smr-gui" 2>/dev/null | grep -q arm64; then
    arch="arm64"
  elif [[ -f "${ROOT}/dist/smr-${version}-darwin-arm64-app.tar.gz" ]]; then
    arch="arm64"
  else
    arch="x86_64"
  fi
  app_tar="${ROOT}/dist/smr-${version}-darwin-${arch}-app.tar.gz"
  cli_tar="${ROOT}/dist/smr-${version}-darwin-${arch}.tar.gz"
  [[ -f "$app_tar" ]] || { echo "Missing $app_tar — run ./scripts/package-macos.sh" >&2; return 1; }
  [[ -f "$cli_tar" ]] || { echo "Missing $cli_tar" >&2; return 1; }

  local test_root test_home secrets_dir config_path app_bundle gui_bin stage
  test_root="$(mktemp -d "${TMPDIR:-/tmp}/smr-installed-test.XXXXXX")"
  test_home="${test_root}/home"
  secrets_dir="${test_root}/secrets"
  mkdir -p "$test_home" "$secrets_dir"
  echo "probe-secret-data" > "${secrets_dir}/project.txt"

  config_path="${test_home}/Library/Application Support/securemodelroute/smr.yaml"
  python3 "${ROOT}/scripts/generate_test_config.py" "$config_path" "$secrets_dir"

  stage="$(mktemp -d)"
  tar -xzf "$cli_tar" -C "$stage"
  mkdir -p "${test_home}/.local/bin" "${test_home}/.local/etc/securemodelroute"
  cp "${stage}/smr" "${test_home}/.local/bin/smr"
  chmod +x "${test_home}/.local/bin/smr"
  rm -rf "$stage"

  tar -xzf "$app_tar" -C "$test_root"
  app_bundle="${test_root}/SecureModelRoute.app"
  gui_bin="${app_bundle}/Contents/MacOS/smr-gui"
  [[ -x "$gui_bin" ]] || { echo "Missing $gui_bin" >&2; return 1; }

  echo "==> Stop conflicting listeners on :8080"
  lsof -ti :8080 | xargs kill -9 2>/dev/null || true
  sleep 1

  echo "==> Launch tray GUI (isolated HOME, SMR_CONFIG=$config_path)"
  HOME="$test_home" SMR_CONFIG="$config_path" "$gui_bin" &
  local gui_pid=$!
  cleanup_mac() {
    kill "$gui_pid" 2>/dev/null || true
    wait "$gui_pid" 2>/dev/null || true
    lsof -ti :8080 | xargs kill -9 2>/dev/null || true
    rm -rf "$test_root"
  }
  trap cleanup_mac EXIT

  echo "==> Wait for server"
  local ok=0
  for _ in $(seq 1 90); do
    if curl -sf "http://127.0.0.1:8080/health" 2>/dev/null | grep -q OK; then
      ok=1
      break
    fi
    if ! kill -0 "$gui_pid" 2>/dev/null; then
      echo "GUI process exited early" >&2
      return 1
    fi
    sleep 1
  done
  [[ "$ok" -eq 1 ]] || { echo "GUI server not ready on :8080" >&2; return 1; }

  echo "==> User smoke: admin UI"
  curl -sf "http://127.0.0.1:8080/ui" | grep -q SafeRoute

  echo "==> Tray: close main window, service should stay up"
  osascript -e "tell application \"System Events\"
    repeat with p in (every process whose unix id is ${gui_pid})
      try
        if (count of windows of p) > 0 then
          click button 1 of window 1 of p
        end if
      end try
    end repeat
  end tell" 2>/dev/null || true
  sleep 2
  if ! curl -sf "http://127.0.0.1:8080/health" 2>/dev/null | grep -q OK; then
    echo "Service stopped after window close (tray regression)" >&2
    return 1
  fi
  if ! kill -0 "$gui_pid" 2>/dev/null; then
    echo "GUI process quit after window close (expected hide-to-tray)" >&2
    return 1
  fi
  echo "Tray hide OK — process alive, server still listening"

  echo "==> Black-box attach @ :8080 (27 scenarios)"
  SMR_ATTACH=1 SMR_BASE=http://127.0.0.1:8080 python3 scripts/blackbox_test.py
}

echo "Installed-app test run: $(date)" | tee "$SUMMARY"

if [[ "$(uname -s)" == "Darwin" ]]; then
  run_step "macOS-installed-app" mac_installed_app_test
else
  echo ">>> SKIP macOS: not on Darwin" | tee -a "$SUMMARY"
fi

UTMCTL="/Applications/UTM.app/Contents/MacOS/utmctl"
if [[ "${SMR_SKIP_VM_TESTS:-0}" != "1" ]] && [[ -x "$UTMCTL" ]] && "$UTMCTL" list 2>/dev/null | grep -q started; then
  if [[ -s "${ROOT}/dist/windows-desktop/SecureModelRoute.exe" ]]; then
    run_step "windows-utm-installed-app" bash "${ROOT}/scripts/vm/utm-run-app-blackbox.sh"
  else
    echo ">>> SKIP Windows app test: missing dist/windows-desktop/SecureModelRoute.exe" | tee -a "$SUMMARY"
  fi
else
  echo ">>> SKIP Windows UTM app test" | tee -a "$SUMMARY"
fi

echo ""
if [[ "$failures" -eq 0 ]]; then
  echo "ALL INSTALLED-APP TESTS PASSED"
  echo "Logs: ${LOG_DIR}/${STAMP}-*.log"
  exit 0
fi
echo "${failures} stage(s) FAILED — see ${LOG_DIR}/${STAMP}-*.log"
exit 1
