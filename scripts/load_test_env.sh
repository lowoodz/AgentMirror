#!/usr/bin/env bash
# Load config/test.env into the current shell (no-op if missing).
# Usage: source scripts/load_test_env.sh
set -a
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${SMR_TEST_ENV:-${ROOT}/config/test.env}"
if [[ -f "${ENV_FILE}" ]]; then
  # shellcheck disable=SC1090
  source "${ENV_FILE}"
fi
set +a

has_test_keys() {
  if [[ -n "${SMR_GLM_API_KEY:-}" && -n "${SMR_DEEPSEEK_API_KEY:-}" ]]; then
    return 0
  fi
  local keys_file="${SMR_KEYS_FILE:-${ROOT}/test_model_api_key.txt}"
  [[ -f "${keys_file}" ]]
}

# Print path to a keys file (materialize from env when needed).
resolve_keys_file() {
  local keys_file="${SMR_KEYS_FILE:-${ROOT}/test_model_api_key.txt}"
  if [[ -f "${keys_file}" ]]; then
    echo "${keys_file}"
    return 0
  fi
  if [[ -n "${SMR_GLM_API_KEY:-}" && -n "${SMR_DEEPSEEK_API_KEY:-}" ]]; then
    mkdir -p "${ROOT}/dist"
    keys_file="${ROOT}/dist/.test-keys-from-env.txt"
    cat > "${keys_file}" <<EOF
1、GLM
api-key：${SMR_GLM_API_KEY}

2、Deepseek
api-key：${SMR_DEEPSEEK_API_KEY}
EOF
    echo "${keys_file}"
    return 0
  fi
  return 1
}
