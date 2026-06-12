#!/usr/bin/env bash
# Scan staged (or all tracked) files for secrets, personal paths, and portability issues.
# Used by .githooks/pre-commit and manually: ./scripts/check-commit-hygiene.sh [--staged|--all]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MODE=staged
while [[ $# -gt 0 ]]; do
  case "$1" in
    --staged) MODE=staged; shift ;;
    --all) MODE=all; shift ;;
    -h|--help)
      echo "Usage: $0 [--staged|--all]"
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

failures=0

log_fail() {
  echo "COMMIT CHECK FAIL: $*" >&2
  failures=$((failures + 1))
}

# Never commit gitignored secret/config blobs (even if force-added).
FORBIDDEN_PATHS=(
  config/test.env
  config/smr.yaml
  config/user-config-backup/
  config/local-hygiene.env
  test_model_api_key.txt
  dist/.test-keys-from-env.txt
)

# Optional comma-separated personal identifiers (copy config/local-hygiene.env.example).
HYGIENE_BLOCK_IDENTIFIERS=()
LOCAL_HYGIENE="${ROOT}/config/local-hygiene.env"
if [[ -f "$LOCAL_HYGIENE" ]]; then
  # shellcheck disable=SC1090
  source "$LOCAL_HYGIENE"
  if [[ -n "${SMR_HYGIENE_BLOCK_IDENTIFIERS:-}" ]]; then
    IFS=',' read -ra _blocks <<< "$SMR_HYGIENE_BLOCK_IDENTIFIERS"
    for _b in "${_blocks[@]}"; do
      _b="${_b#"${_b%%[![:space:]]*}"}"
      _b="${_b%"${_b##*[![:space:]]}"}"
      [[ -n "$_b" ]] && HYGIENE_BLOCK_IDENTIFIERS+=("$_b")
    done
  fi
fi

should_skip_file() {
  local f="$1"
  case "$f" in
    target/*|dist/*|node_modules/*|gui/node_modules/*|test-data/*) return 0 ;;
    *.png|*.jpg|*.jpeg|*.gif|*.ico|*.icns|*.pdf|*.zip|*.tar.gz|*.dmg|*.exe|*.woff*|*.ttf) return 0 ;;
    *.lock|Cargo.lock) return 0 ;;
  esac
  return 1
}

is_placeholder_path() {
  local line="$1"
  [[ "$line" =~ windows-user|your-windows|your-user|Public/|/tmp/|example\.|placeholder|changeme|\$\{|\*\* ]] && return 0
  [[ "$line" == *'<'* && "$line" == *'>'* ]] && return 0
  [[ "$line" =~ SMR_WINDOWS_USER|SMR_GUEST_STAGING|USERPROFILE|LOCALAPPDATA ]] && return 0
  return 1
}

is_allowed_secret_line() {
  local line="$1"
  case "$line" in
    *dummy*|*example*|*placeholder*|*changeme*|*your_*|*your-*|*api_key_env*|*OPENAI_API_KEY*|*ANTHROPIC_API_KEY*) return 0 ;;
  esac
  if [[ "$line" =~ SMR_[A-Z_]+=[[:space:]]*$ ]]; then return 0; fi
  if [[ "$line" =~ api[_-]?key.*=.*dummy ]]; then return 0; fi
  return 1
}

is_test_fixture_line() {
  local f="$1"
  local line="$2"
  case "$f" in
    scripts/blackbox_test.py|scripts/live_test.py|scripts/transparency_pass_through_test.py)
      [[ "$line" == *PRESET_* || "$line" == *CONTENT_SECRET* || "$line" == *FILE_SECRET* ]] && return 0
      ;;
    crates/*/src/*.rs)
      [[ "$line" == *sk-abc* || "$line" == *AKIA1234* || "$line" == *ghp_abc* ]] && return 0
      ;;
  esac
  return 1
}

is_ui_form_line() {
  local f="$1"
  local line="$2"
  case "$f" in
    crates/smr-core/assets/index.html|gui/dist/index.html)
      [[ "$line" == *draftApiKey* || "$line" == *routingDraft.api_key* || "$line" == *placeholder=*sk* ]] && return 0
      ;;
  esac
  return 1
}

is_doc_example_line() {
  local line="$1"
  [[ "$line" == *Example* || "$line" == *example* || "$line" == *192.168.1.100* || "$line" == *your-windows* ]] && return 0
  return 1
}

is_env_export_line() {
  local line="$1"
  [[ "$line" == *'export SMR_'*'="$('* || "$line" == *'SMR_'*'="$('* ]] && return 0
  return 1
}

skip_inline_secret_scan() {
  case "$1" in
    crates/smr-core/assets/index.html|gui/dist/index.html) return 0 ;;
  esac
  return 1
}

collect_files() {
  if [[ "$MODE" == "staged" ]]; then
    git diff --cached --name-only --diff-filter=ACMR
  else
    git ls-files
  fi
}

check_forbidden_paths() {
  local f
  for f in "${FORBIDDEN_PATHS[@]}"; do
    if git diff --cached --name-only --diff-filter=ACMR | grep -Fxq "$f" 2>/dev/null; then
      log_fail "forbidden file staged: $f (must stay gitignored)"
    fi
  done
}

scan_file() {
  local f="$1"
  local line_num=0
  local line

  should_skip_file "$f" && return 0
  [[ -f "$f" ]] || return 0

  while IFS= read -r line || [[ -n "$line" ]]; do
    line_num=$((line_num + 1))

    # --- Secrets / API keys ---
    if ! skip_inline_secret_scan "$f" && ! is_allowed_secret_line "$line" && ! is_test_fixture_line "$f" "$line" && ! is_ui_form_line "$f" "$line" && ! is_env_export_line "$line"; then
      if [[ "$line" =~ sk-[a-zA-Z0-9]{20,} ]]; then
        log_fail "$f:$line_num: possible OpenAI-style API key (sk-...)"
      fi
      if [[ "$line" =~ ghp_[a-zA-Z0-9]{20,} ]]; then
        log_fail "$f:$line_num: possible GitHub token (ghp_...)"
      fi
      if [[ "$line" =~ AKIA[0-9A-Z]{16} ]]; then
        log_fail "$f:$line_num: possible AWS access key (AKIA...)"
      fi
      if [[ "$line" =~ api[_-]?key.*[:=].*[a-zA-Z0-9._-]{16,} ]]; then
        log_fail "$f:$line_num: inline api-key value (use api_key_env or gitignored test.env)"
      fi
      if [[ "$line" =~ SMR_(GLM|DEEPSEEK)_API_KEY=[^[:space:]#]+ ]]; then
        log_fail "$f:$line_num: SMR_*_API_KEY with value (belongs in config/test.env only)"
      fi
      if [[ "$line" =~ Bearer[[:space:]]+[a-zA-Z0-9._-]{20,} ]]; then
        log_fail "$f:$line_num: Bearer token literal"
      fi
    fi

    # --- Personal / machine-specific paths ---
    case "$f" in
      scripts/check-commit-hygiene.sh) ;;
      *)
    if [[ "$line" =~ /Users/[a-zA-Z0-9._-]+/ ]] && ! is_placeholder_path "$line"; then
      if [[ ! "$line" =~ /Users/(Shared|Public|windows-user|your-|example) ]]; then
        log_fail "$f:$line_num: macOS home path (/Users/...) — use env vars or placeholders"
      fi
    fi
    if [[ "$line" =~ [Cc]:[/\\]Users[/\\][a-zA-Z0-9._-]+ ]] && ! is_placeholder_path "$line"; then
      if [[ ! "$line" =~ [Uu]sers[/\\](Public|windows-user|your-|Windows-user|Default) ]]; then
        log_fail "$f:$line_num: Windows user path (C:/Users/...) — use SMR_GUEST_STAGING / placeholders"
      fi
    fi
    if [[ "$line" == *"@192.168."* || "$line" == *"@10."* ]]; then
      log_fail "$f:$line_num: personal SSH host or LAN address — use config/test.env + ~/.ssh/config"
    fi
    if [[ "$line" =~ HostName[[:space:]]+(192\.168\.|10\.|172\.(1[6-9]|2[0-9]|3[0-1])\.)[0-9.]+ ]]; then
      if ! is_doc_example_line "$line"; then
        log_fail "$f:$line_num: LAN IP in HostName — keep in ~/.ssh/config, not the repo"
      fi
    fi
      ;;
    esac

    # --- Optional personal identifiers (config/local-hygiene.env, gitignored) ---
    if [[ ${#HYGIENE_BLOCK_IDENTIFIERS[@]} -gt 0 ]]; then
      case "$f" in
        scripts/check-commit-hygiene.sh|config/local-hygiene.env.example) ;;
        scripts/*|crates/*|gui/src*|config/*.yaml|.cursor/rules/*)
          for _id in "${HYGIENE_BLOCK_IDENTIFIERS[@]}"; do
            if [[ "$line" == *"$_id"* ]]; then
              log_fail "$f:$line_num: blocked personal identifier \"$_id\" — use config/test.env or placeholders"
              break
            fi
          done
          ;;
      esac
    fi
  done <"$f"
}

main() {
  echo "==> Commit hygiene check ($MODE)"
  if [[ "$MODE" == "staged" ]]; then
    if ! git rev-parse --git-dir >/dev/null 2>&1; then
      echo "Not a git repository" >&2
      exit 2
    fi
    if [[ -z "$(git diff --cached --name-only)" ]]; then
      echo "Nothing staged — skip"
      exit 0
    fi
    check_forbidden_paths
  fi

  local f
  while IFS= read -r f; do
    [[ -n "$f" ]] || continue
    scan_file "$f"
  done < <(collect_files)

  if [[ "$failures" -gt 0 ]]; then
    echo "" >&2
    echo "Commit blocked: $failures issue(s). Fix or unstage, then retry." >&2
    echo "Manual run: ./scripts/check-commit-hygiene.sh --staged" >&2
    exit 1
  fi

  echo "Commit hygiene OK ($MODE)"
}

main
