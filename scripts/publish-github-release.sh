#!/usr/bin/env bash
# Publish AgentMirror release to GitHub: push master, tag, upload dist artifacts.
# Scans installers for personal paths/secrets before upload. Never ships config/smr.yaml.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep '^version = ' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
TAG="v${VERSION}"

ARTIFACTS=(
  "dist/smr-${VERSION}-darwin-arm64.tar.gz"
  "dist/smr-${VERSION}-darwin-x86_64.tar.gz"
  "dist/smr-${VERSION}-darwin-arm64-app.tar.gz"
  "dist/smr-${VERSION}-darwin-x86_64-app.tar.gz"
  "dist/AgentMirror_${VERSION}_arm64.dmg"
  "dist/AgentMirror_${VERSION}_x86_64.dmg"
  "dist/smr-${VERSION}-windows-x86_64.zip"
  "dist/smr-${VERSION}-windows-x86_64-app.zip"
  "dist/AgentMirror_${VERSION}_x64-setup.exe"
)

PATTERNS='/Users/[a-z]|C:\\Users\\[^P]|sk-[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{20,}'

echo "==> Scanning ${#ARTIFACTS[@]} artifacts for personal info / secrets"
for f in "${ARTIFACTS[@]}"; do
  if [[ ! -f "$f" ]]; then
    echo "ERROR: missing $f — run ./scripts/release-full.sh first" >&2
    exit 1
  fi
  if strings "$f" 2>/dev/null | rg -i "$PATTERNS" | head -1 | grep -q .; then
    echo "ERROR: $f contains sensitive strings" >&2
    strings "$f" | rg -i "$PATTERNS" | head -5 >&2
    exit 1
  fi
done
echo "    artifact scan OK"

# Bundled config must be example only (no live smr.yaml)
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
tar -xzf "dist/smr-${VERSION}-darwin-arm64.tar.gz" -C "$tmpdir"
if [[ -f "$tmpdir/smr.yaml" && ! -f "$tmpdir/smr.example.yaml" ]]; then
  echo "ERROR: tarball ships smr.yaml instead of smr.example.yaml" >&2
  exit 1
fi
if [[ -f "$tmpdir/config/smr.yaml" ]]; then
  echo "ERROR: tarball contains config/smr.yaml (live config)" >&2
  exit 1
fi
echo "    bundled config OK (smr.example.yaml only)"

echo "==> Push master"
git push origin master

echo "==> Tag ${TAG} at HEAD"
git tag -f "$TAG"
git push origin "$TAG" --force

NOTES="$(cat <<EOF
## AgentMirror ${VERSION}

### Highlights
- **AgentMirror** desktop app (macOS DMG + Windows NSIS) with tray GUI and \`smr\` CLI
- **Agent reflection**: reconstruct goals, decisions, and actions from LLM proxy traffic; causal graphs, reflection reports, daily digests
- **DLP**: span-level redaction, reversible tool-arg tokens, file index (PDF sidecar), SessionGuard
- **Routing**: OpenAI ↔ Anthropic auto-detect; high/medium/low fallback groups; OpenClaw 12/12 security matrix (Mac + Windows)

### Install
See [README](https://github.com/lowoodz/AgentMirror#readme). First run copies \`smr.example.yaml\` — configure upstream \`api_key_env\` locally; no keys are embedded in installers.

### Verified
Full \`release-full.sh\` passed (Mac + Windows VM): unit tests, live API blackbox, OpenClaw matrix 12/12.
EOF
)"

if gh release view "$TAG" >/dev/null 2>&1; then
  echo "==> Update existing release ${TAG}"
  gh release edit "$TAG" --title "AgentMirror ${VERSION}" --notes "$NOTES"
else
  echo "==> Create release ${TAG}"
  gh release create "$TAG" --title "AgentMirror ${VERSION}" --notes "$NOTES"
fi

echo "==> Upload artifacts"
gh release upload "$TAG" "${ARTIFACTS[@]}" --clobber

echo "==> Done: $(gh release view "$TAG" --json url -q .url)"
