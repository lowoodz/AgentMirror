# Windows release — reference

> **Unified workflow:** optional CLI / app / NSIS artifacts are documented in [release-cycle/reference.md](../release-cycle/reference.md). Use `-CliOnly`, `-WithApp`, `-WithoutSetup`, etc. on `scripts/windows/release-cycle.ps1`.

## Environment variables

| Variable | Required value | Why |
|----------|----------------|-----|
| `CARGO_TARGET_DIR` | `{repo}/target` | Avoids stale binaries from sandbox or other workspaces |
| `CI` | `true` in GHA / release builds | Tauri non-interactive mode; use `--ci` flag |
| `PYTHONUTF8` | `1` | Python tests on Windows |
| `SMR_INSTALL_PREFIX` | Optional test prefix | Isolated install-smoke without touching `~/.local` |
| `SMR_NSIS_TEST_USER` | VM interactive username | NSIS currentUser install under UTM |

Do **not** set or commit: `SMR_GLM_API_KEY`, `SMR_DEEPSEEK_API_KEY` in repo files — only in gitignored `config/test.env`.

## Tauri / NSIS

- Command: `npm run tauri -- build --bundles nsis --ci`
- Invalid: `--silent` (npm `--silent` is fine for `npm ci` only)
- NSIS output: `target/release/bundle/nsis/*-setup.exe`
- Stable name copied to dist: `SafeRoute_{version}_x64-setup.exe`
- Portable exe search order: `SafeRoute.exe`, `smr-gui.exe`

## Install paths

| Component | Path |
|-----------|------|
| CLI (zip install) | `%USERPROFILE%\.local\bin\smr.exe` |
| Config | `%USERPROFILE%\.local\etc\securemodelroute\smr.yaml` |
| NSIS GUI | `%LOCALAPPDATA%\Programs\com.securemodelroute.desktop\SafeRoute.exe` |
| Legacy GUI | `%LOCALAPPDATA%\Programs\SafeRoute\` |

Uninstall: `scripts/uninstall.ps1` (NSIS `/S` + remove portable artifacts).

## Test matrix

| Layer | Script | Needs API keys |
|-------|--------|----------------|
| Unit + health | `scripts/verify.ps1` | No |
| Package check | `scripts/windows/verify-package.ps1` (`-RequireSetup`, `-RequireAppZip`) | No |
| Install smoke | `scripts/vm/windows-install-smoke.ps1` (`-CliOnly` for CLI-only) | No |
| Functional | `scripts/install_functional_test.py` | Yes |
| Blackbox | `scripts/blackbox_test.py` | Yes |
| Full suite | `scripts/run_all_tests.ps1` | Yes (skips live if missing) |

## GitHub Actions → Release

1. Push triggers `.github/workflows/windows-nsis.yml` when packaging paths change.
2. Job runs `scripts/package.ps1` with `CI=true`.
3. Artifacts: `windows-nsis-{sha}`.
4. Upload to release:

```bash
gh run download <run-id> --repo lowoodz/LLM-SafeRoute
gh release upload vX.Y.Z dist/SafeRoute_X.Y.Z_x64-setup.exe dist/smr-X.Y.Z-windows-x86_64.zip --clobber
```

## UTM / remote testing

- **CLI zip test**: `scripts/vm/utm-run-install-smoke.sh` or `windows-install-smoke.ps1`
- **NSIS test**: `scripts/vm/utm-run-nsis-install-test.sh` — uses interactive task; do not run NSIS `/S` as SYSTEM
- **Full suite**: `scripts/vm/utm-run-all-tests.sh` or SSH `scripts/windows_vm_test.sh`

## Naming

- Product / GUI exe: **SafeRoute**
- GitHub repo: **LLM-SafeRoute**
- Crate binary: `smr.exe`; GUI crate: `smr-gui`; Tauri productName: `SafeRoute`
