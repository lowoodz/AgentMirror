#!/usr/bin/env bash
# Poll /api/status until file_index_ready (and not rebuilding) or timeout.
set -euo pipefail

BASE_URL="${1:-http://127.0.0.1:8080}"
TIMEOUT_SECS="${2:-600}"
INTERVAL="${3:-2}"

deadline=$((SECONDS + TIMEOUT_SECS))

while (( SECONDS < deadline )); do
  if status="$(curl -sf "${BASE_URL}/api/status" 2>/dev/null)"; then
    ready="$(python3 -c "import json,sys; s=json.loads(sys.argv[1]); print('1' if s.get('file_index_ready') and not s.get('file_index_rebuilding') else '0')" "$status" 2>/dev/null || echo 0)"
    if [[ "$ready" == "1" ]]; then
      echo "file_index_ready at ${BASE_URL}"
      exit 0
    fi
    rebuilding="$(python3 -c "import json,sys; print('1' if json.loads(sys.argv[1]).get('file_index_rebuilding') else '0')" "$status" 2>/dev/null || echo 0)"
    if [[ "$rebuilding" == "1" ]]; then
      echo "… file index rebuilding"
    else
      echo "… waiting for file index"
    fi
  else
    echo "… waiting for ${BASE_URL}"
  fi
  sleep "$INTERVAL"
done

echo "Timed out after ${TIMEOUT_SECS}s waiting for file_index_ready at ${BASE_URL}" >&2
exit 1
