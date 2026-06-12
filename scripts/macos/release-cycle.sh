#!/usr/bin/env bash
# End-to-end macOS release cycle: clean → compile → package → test → install → post-install test.
# Optional artifacts: CLI always; app tar + DMG when --with-app / --with-dmg (default on).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=macos/common.sh
source "${ROOT}/scripts/macos/common.sh"
# shellcheck source=load_test_env.sh
source "${ROOT}/scripts/load_test_env.sh"

PHASE="all"

SKIP_CLEAN=false
SKIP_TESTS=false
SKIP_INSTALLED=false
KEEP_CONFIG=false
WITH_APP=true
WITH_DMG=true
LOG_PATH="${ROOT}/dist/macos-release-cycle.log"

usage() {
  cat <<'EOF'
Usage: release-cycle.sh [phase] [options]

Phases: all | preflight | clean | compile | package | verify | test | install | installed | openclaw

Artifact options (app / DMG are optional; CLI tar is always built and verified):
  --with-app       Require app tar; build Tauri app in package phase (default)
  --without-app    CLI-only package; skip app tar in verify/install
  --no-app         Alias for --without-app
  --with-dmg       Require DMG in verify (default on arm64 host)
  --without-dmg    Do not require DMG
  --no-dmg         Alias for --without-dmg
  --cli-only       Shorthand: --without-app --without-dmg

Other:
  --skip-clean     Skip uninstall in clean phase
  --skip-tests     Skip live API tests
  --skip-installed Skip post-install tests
  --keep-config    Keep config on uninstall
  --log PATH       Log file (default: dist/macos-release-cycle.log)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    all|preflight|clean|compile|package|verify|test|install|installed|openclaw)
      PHASE="$1"
      shift
      ;;
    --with-app) WITH_APP=true; shift ;;
    --without-app|--no-app) WITH_APP=false; shift ;;
    --with-dmg) WITH_DMG=true; shift ;;
    --without-dmg|--no-dmg) WITH_DMG=false; shift ;;
    --cli-only) WITH_APP=false; WITH_DMG=false; shift ;;
    --skip-clean) SKIP_CLEAN=true; shift ;;
    --skip-tests) SKIP_TESTS=true; shift ;;
    --skip-installed) SKIP_INSTALLED=true; shift ;;
    --keep-config) KEEP_CONFIG=true; shift ;;
    --log) LOG_PATH="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

mkdir -p "$(dirname "$LOG_PATH")"
: > "$LOG_PATH"

log() {
  local line="[$(date '+%Y-%m-%d %H:%M:%S')] $*"
  echo "$line" | tee -a "$LOG_PATH"
}

run_step() {
  local name="$1"
  shift
  log "==> $name"
  if "$@"; then
    log "    OK: $name"
  else
    log "ERROR: $name failed (see ${LOG_PATH})"
    exit 1
  fi
}

phase_preflight() {
  run_step "Preflight" bash "${ROOT}/scripts/macos/preflight.sh" --require-node --require-python
}

phase_clean() {
  if [[ "$SKIP_CLEAN" == true ]]; then
    log "Skipping clean (--skip-clean)"
    return 0
  fi
  run_step "Stop processes" smr_stop_processes
  if [[ "$KEEP_CONFIG" != true ]]; then
    run_step "Backup user config" bash "${ROOT}/scripts/backup-user-config.sh" || true
  fi
  local args=(bash "${ROOT}/scripts/uninstall.sh" --quiet)
  [[ "$KEEP_CONFIG" == true ]] && args+=(--keep-config)
  run_step "Uninstall previous install" "${args[@]}"
}

phase_compile() {
  smr_set_build_env
  run_step "Sync admin UI" bash "${ROOT}/scripts/sync-admin-ui.sh"
  run_step "Unit + smoke (verify.sh)" bash "${ROOT}/scripts/verify.sh"
}

phase_package() {
  local pkg_args=()
  [[ "$WITH_APP" == false ]] && pkg_args+=(--cli-only)
  run_step "Build packages (package-macos.sh)" bash "${ROOT}/scripts/package-macos.sh" "${pkg_args[@]}"
}

phase_verify() {
  local args=(bash "${ROOT}/scripts/macos/verify-package.sh")
  [[ "$WITH_APP" == true ]] && args+=(--require-app)
  if [[ "$WITH_DMG" == true ]] && [[ "$(smr_native_arch)" == "arm64" ]] && [[ "$WITH_APP" == true ]]; then
    args+=(--require-dmg)
  fi
  run_step "Verify dist artifacts (app=${WITH_APP}, dmg=${WITH_DMG})" "${args[@]}"
}

phase_test() {
  if [[ "$SKIP_TESTS" == true ]]; then
    log "Skipping live tests (--skip-tests)"
    return 0
  fi
  if ! has_test_keys; then
    log "SKIP live tests: set config/test.env from config/test.env.example"
    return 0
  fi
  run_step "Install functional" python3 "${ROOT}/scripts/install_functional_test.py"
  run_step "Blackbox" python3 "${ROOT}/scripts/blackbox_test.py"
  run_step "Stress" python3 "${ROOT}/scripts/live_test.py"
}

phase_install() {
  run_step "Clean before install" smr_stop_processes
  local args=(bash "${ROOT}/scripts/uninstall.sh" --quiet)
  [[ "$KEEP_CONFIG" == true ]] && args+=(--keep-config)
  run_step "Uninstall for fresh install" "${args[@]}"
  local smoke_args=()
  [[ "$WITH_APP" == false ]] && smoke_args+=(--cli-only)
  run_step "Install smoke from dist" bash "${ROOT}/scripts/macos/install-smoke.sh" "${smoke_args[@]}"
  if [[ "$KEEP_CONFIG" != true ]] && [[ -e "${ROOT}/config/user-config-backup/latest" ]]; then
    run_step "Restore user config" bash "${ROOT}/scripts/restore-user-config.sh"
  fi
}

phase_installed() {
  if [[ "$SKIP_INSTALLED" == true ]]; then
    log "Skipping installed-app tests (--skip-installed)"
    return 0
  fi
  if [[ "$WITH_APP" == false ]]; then
    log "SKIP installed-app tests: --without-app / --cli-only (no tray GUI package)"
    return 0
  fi
  if ! has_test_keys; then
    log "SKIP installed-app tests: missing config/test.env"
    return 0
  fi
  run_step "Installed-app tests" bash "${ROOT}/scripts/run_installed_app_tests.sh"
}

phase_openclaw() {
  if [[ "$SKIP_TESTS" == true ]]; then
    log "Skipping OpenClaw matrix (--skip-tests)"
    return 0
  fi
  if ! has_test_keys; then
    log "SKIP OpenClaw matrix: set config/test.env from config/test.env.example"
    return 0
  fi
  if ! has_openclaw; then
    log "SKIP OpenClaw matrix: openclaw not in PATH"
    return 0
  fi
  run_step "OpenClaw strict matrix (12 cases)" bash "${ROOT}/scripts/run_openclaw_matrix.sh" \
    --log "${ROOT}/dist/openclaw-matrix-macos-release-full.log"
}

log "macOS release cycle (phase=${PHASE}, app=${WITH_APP}, dmg=${WITH_DMG}) root=${ROOT}"
log "Log: ${LOG_PATH}"

case "$PHASE" in
  all)
    phase_preflight
    phase_clean
    phase_compile
    phase_package
    phase_verify
    phase_test
    phase_install
    phase_installed
    phase_openclaw
    ;;
  preflight) phase_preflight ;;
  clean) phase_clean ;;
  compile) phase_compile ;;
  package) phase_package ;;
  verify) phase_verify ;;
  test) phase_test ;;
  install) phase_install ;;
  installed) phase_installed ;;
  openclaw) phase_openclaw ;;
  *)
    echo "Unknown phase: $PHASE" >&2
    usage >&2
    exit 2
    ;;
esac

log "RELEASE CYCLE PASSED"
exit 0
