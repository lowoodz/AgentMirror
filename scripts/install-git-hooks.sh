#!/usr/bin/env bash
# Point this repo at .githooks/ (pre-commit hygiene checks).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

chmod +x "${ROOT}/scripts/check-commit-hygiene.sh" "${ROOT}/.githooks/pre-commit"

git config core.hooksPath .githooks

echo "Installed git hooks: core.hooksPath=.githooks"
echo "Pre-commit runs: ./scripts/check-commit-hygiene.sh --staged"
echo "Manual check:    ./scripts/check-commit-hygiene.sh --staged|--all"
