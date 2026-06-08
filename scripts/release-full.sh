#!/usr/bin/env bash
# Mac host: full release workflow — clean → compile → package (mac + win + UTM NSIS) → verify → test → install → installed → UTM suite.
#
# Use this for end-to-end validation before a GitHub release. For macOS-only or single-phase work,
# prefer ./scripts/release-cycle.sh (see .cursor/skills/release-cycle/SKILL.md).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${ROOT}/target"

RC=(bash "${ROOT}/scripts/macos/release-cycle.sh")
SKIP_CLEAN=false
SKIP_VM=false
PACKAGE_ONLY=false
RC_EXTRA=()

usage() {
  cat <<'EOF'
Usage: ./scripts/release-full.sh [options]

Mac host only. Runs the complete release pipeline:

  clean (dist + UTM guest + local uninstall)
  → preflight → compile → package-all → verify
  → test → install → installed
  → utm-run-all-tests (when UTM guest is running)

Options (forwarded to release-cycle where applicable):
  --cli-only          CLI tar/zip only (no app, DMG, NSIS)
  --skip-clean        Skip clean-dist / clean-vm / uninstall
  --skip-tests        Skip live API tests on macOS host
  --skip-installed    Skip post-install tray / UTM app blackbox
  --skip-vm           Skip utm-run-all-tests (also sets SMR_SKIP_VM_TESTS=1)
  --package-only      Stop after package + verify
  --keep-config       Keep smr.yaml on uninstall
  --log PATH          Log file (default: dist/macos-release-full.log)

Windows native full cycle:
  .\scripts\windows\release-cycle.ps1

See: .cursor/skills/release-cycle/SKILL.md
EOF
}

LOG_PATH="${ROOT}/dist/macos-release-full.log"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cli-only|--skip-tests|--skip-installed|--keep-config)
      RC_EXTRA+=("$1")
      shift
      ;;
    --skip-clean) SKIP_CLEAN=true; shift ;;
    --skip-vm) SKIP_VM=true; export SMR_SKIP_VM_TESTS=1; shift ;;
    --package-only) PACKAGE_ONLY=true; shift ;;
    --log) LOG_PATH="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "release-full.sh is for macOS hosts (use package-all + release-cycle on Windows)." >&2
  exit 2
fi

mkdir -p "$(dirname "$LOG_PATH")"
: > "$LOG_PATH"

log() {
  local line="[$(date '+%Y-%m-%d %H:%M:%S')] $*"
  echo "$line" | tee -a "$LOG_PATH"
}

run() {
  local name="$1"
  shift
  log "==> $name"
  if "$@" >>"$LOG_PATH" 2>&1; then
    log "    OK: $name"
  else
    log "ERROR: $name failed — see ${LOG_PATH}"
    exit 1
  fi
}

CLI_ONLY=false
for a in "${RC_EXTRA[@]}"; do [[ "$a" == "--cli-only" ]] && CLI_ONLY=true; done

log "release-full root=${ROOT} cli_only=${CLI_ONLY} skip_clean=${SKIP_CLEAN} skip_vm=${SKIP_VM}"
log "Log: ${LOG_PATH}"

if [[ "$SKIP_CLEAN" != true ]]; then
  run "Clean dist + VM staging" bash "${ROOT}/scripts/clean-dist.sh"
  run "Clean local install" "${RC[@]}" clean "${RC_EXTRA[@]}"
fi

run "Preflight" "${RC[@]}" preflight "${RC_EXTRA[@]}"
run "Compile (sync UI + verify.sh)" "${RC[@]}" compile "${RC_EXTRA[@]}"

if [[ "$CLI_ONLY" == true ]]; then
  run "Package macOS CLI" bash "${ROOT}/scripts/package-macos.sh" --cli-only
  run "Package Windows CLI zip" bash "${ROOT}/scripts/package-windows.sh"
else
  run "Package all platforms" bash "${ROOT}/scripts/package-all.sh"
fi

run "Verify dist artifacts" "${RC[@]}" verify "${RC_EXTRA[@]}"

if [[ "$PACKAGE_ONLY" == true ]]; then
  log "PACKAGE-ONLY DONE (see dist/LATEST-INSTALLERS.txt)"
  exit 0
fi

run "Live tests (host)" "${RC[@]}" test "${RC_EXTRA[@]}"
run "Install from dist" "${RC[@]}" install "${RC_EXTRA[@]}"
run "Installed-app tests" "${RC[@]}" installed "${RC_EXTRA[@]}"

if [[ "$SKIP_VM" != true ]]; then
  # shellcheck source=vm/vm-ssh.sh
  source "${ROOT}/scripts/vm/vm-ssh.sh"
  if vm_ssh_require 2>/dev/null; then
    if [[ "$CLI_ONLY" == true ]]; then
      log "SKIP utm-run-all-tests (--cli-only)"
    elif ls "${ROOT}"/dist/smr-*-windows-x86_64.zip >/dev/null 2>&1; then
      run "UTM full suite (functional + NSIS + blackbox)" bash "${ROOT}/scripts/vm/utm-run-all-tests.sh"
    else
      log "SKIP utm-run-all-tests: no Windows CLI zip in dist/"
    fi
  else
    log "SKIP utm-run-all-tests: SSH to $VM_SSH unavailable"
  fi
fi

log "RELEASE FULL PASSED"
echo "Log: ${LOG_PATH}"
echo "Artifacts: ${ROOT}/dist/LATEST-INSTALLERS.txt"
