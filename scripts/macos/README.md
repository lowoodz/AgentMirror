# macOS release scripts

Canonical workflow: `.cursor/skills/release-cycle/SKILL.md`

## Recommended commands

```bash
chmod +x scripts/release-full.sh scripts/release-cycle.sh scripts/macos/*.sh scripts/clean-*.sh scripts/uninstall.sh
export CARGO_TARGET_DIR="$PWD/target"

# Full validation (mac + Windows artifacts + UTM tests):
./scripts/release-full.sh

# macOS-only cycle:
./scripts/release-cycle.sh

# Package dist/ only:
./scripts/package-all.sh --clean
```

| Script | Purpose |
|--------|---------|
| `../release-full.sh` | **Full pipeline** — clean, package-all, test, install, UTM |
| `release-cycle.sh` | macOS phase orchestrator |
| `preflight.sh` | Toolchain checks |
| `verify-package.sh` | Validate `dist/*.tar.gz` / DMG |
| `install-smoke.sh` | Install from dist + health |
| `common.sh` | Shared helpers |

Entry from repo root: `./scripts/release-cycle.sh` (delegates here on Darwin).

Clean: `./scripts/clean-dist.sh` (dist + UTM guest), `./scripts/uninstall.sh` (local install).
