# Release cycle reference

## Scenarios

### A — Mac developer, ship everything

```bash
export CARGO_TARGET_DIR="$PWD/target"
./scripts/release-full.sh
```

Requires: UTM Windows guest running for NSIS + VM tests. Produces all `dist/LATEST-INSTALLERS.txt` artifacts.

### B — Mac developer, macOS only

```bash
./scripts/release-cycle.sh
```

Skips Windows cross-package unless you run `package-windows.sh` separately.

### C — Mac developer, package only (no install/tests)

```bash
./scripts/package-all.sh --clean
# or CLI only:
./scripts/package-all.sh --clean --cli-only
```

### D — Windows machine, full validation

```powershell
$env:CARGO_TARGET_DIR = "$PWD\target"
.\scripts\windows\release-cycle.ps1
```

### E — Incremental phase (debug one step)

```bash
./scripts/release-cycle.sh compile
./scripts/release-cycle.sh package --with-app --without-dmg
./scripts/release-cycle.sh verify --with-app --without-dmg
./scripts/release-cycle.sh install
```

```powershell
.\scripts\windows\release-cycle.ps1 -Phase package -CliOnly
.\scripts\windows\release-cycle.ps1 -Phase verify -WithoutApp -WithSetup
```

## Phase order (both platforms)

```
preflight → clean → compile → package → verify → test → install → installed
```

| Phase | Needs API keys | Notes |
|-------|----------------|-------|
| preflight | No | Fails early on missing cargo/node/nsis |
| clean | No | Stop smr/SafeRoute; uninstall |
| compile | No | Sync admin UI; cargo test + health |
| package | No | Writes versioned artifacts to `dist/` |
| verify | No | Fails if expected artifacts missing |
| test | Yes (skip if missing) | functional + blackbox + stress |
| install | No | Uninstall again; install from dist; health |
| installed | Yes for blackbox | macOS tray; UTM app blackbox from Mac |

**Mac `release-full.sh` adds after `installed`:** `vm/utm-run-all-tests.sh` when UTM guest is up.

## Optional artifacts

### macOS (`scripts/macos/release-cycle.sh`)

| Flag | Default (full) | CLI only |
|------|----------------|----------|
| `--with-app` | on | `--without-app` / `--cli-only` |
| `--with-dmg` | on (arm64) | `--without-dmg` / `--cli-only` |

### Windows (`scripts/windows/release-cycle.ps1`)

| Flag | Default (full) | CLI only |
|------|----------------|----------|
| `-WithApp` | on | `-WithoutApp` / `-CliOnly` |
| `-WithSetup` | on | `-WithoutSetup` / `-CliOnly` |

## Environment

| Variable | Value | Notes |
|----------|-------|-------|
| `CARGO_TARGET_DIR` | `{repo}/target` | **Required** |
| `CI` | `true` | Windows Tauri / GHA |
| `PYTHONUTF8` | `1` | Python tests on Windows |
| `SMR_INSTALL_PREFIX` | temp path | Isolated install-smoke |
| `SMR_SKIP_VM_TESTS` | `1` | Skip UTM on Mac |
| `SMR_WINDOWS_USER` | `windows-user` | **Required** — enforced by `vm-ssh.sh` |
| `SMR_WINDOWS_HOST` | `windows-vm` | SSH Host alias |
| `config/test.env` | gitignored | API keys |

## Install locations

### macOS

| Component | Path |
|-----------|------|
| CLI | `~/.local/bin/smr` |
| Config | `~/.local/etc/securemodelroute/smr.yaml` |
| GUI | `~/Applications/SafeRoute.app` |
| App data | `~/Library/Application Support/securemodelroute/` |

### Windows

| Component | Path |
|-----------|------|
| CLI (zip) | `%USERPROFILE%\.local\bin\smr.exe` |
| Config | `%USERPROFILE%\.local\etc\securemodelroute\smr.yaml` |
| NSIS GUI | `%LOCALAPPDATA%\Programs\com.securemodelroute.desktop\` |

### UTM guest staging (not for end users)

| Path | Purpose |
|------|---------|
| `C:\Users\Public\smr-build*` | Tauri/Rust build tree |
| `C:\Users\Public\smr-build-target-cache` | Rust cache (multi-GB; `clean-vm.sh` removes) |
| `C:\Users\Public\smr-desktop-out` | NSIS + exe output before pull to Mac |
| `C:\Users\Public\smr-*-stage` | Install test staging |

## Complete script inventory

### Orchestrators

| Script | Host |
|--------|------|
| `release-full.sh` | Mac — full validation |
| `release-cycle.sh` | Mac entry |
| `macos/release-cycle.sh` | Mac phases |
| `windows/release-cycle.ps1` | Windows phases |
| `package-all.sh` | Mac — all dist artifacts |
| `run_full_tests.sh` | Mac — dev tests + UTM |
| `run_installed_app_tests.sh` | Mac — post-install |

### Package

| Script | Output |
|--------|--------|
| `package-macos.sh` | darwin tarballs + app + DMG |
| `package-windows.sh` | `smr-{v}-windows-x86_64.zip`, `smr.exe` |
| `vm/package-windows-gui.sh` | NSIS setup + `windows-desktop/SafeRoute.exe` |
| `package.ps1` | Windows native CLI + app + NSIS |
| `package.sh` | Non-Darwin fallback |

### Clean

| Script | Scope |
|--------|-------|
| `clean-dist.sh` | `dist/` + calls `clean-vm.sh` |
| `clean-vm.sh` | UTM `C:\Users\Public\smr-*` |
| `vm/clean-vm-artifacts.ps1` | Guest-side deletion |
| `uninstall.sh` / `uninstall.ps1` | Installed app on host |

### Verify / test / install

| Script | Phase |
|--------|-------|
| `verify.sh` / `verify.ps1` | compile |
| `macos/verify-package.sh` | verify |
| `windows/verify-package.ps1` | verify |
| `install_functional_test.py` | test |
| `blackbox_test.py` | test / installed |
| `live_test.py` | test |
| `run_all_tests.sh` / `.ps1` | test |
| `macos/install-smoke.sh` | install |
| `vm/windows-install-smoke.ps1` | install |
| `vm/utm-run-test.sh` | UTM functional |
| `vm/utm-run-all-tests.sh` | UTM full suite |
| `vm/utm-run-nsis-install-test.sh` | UTM NSIS |
| `vm/utm-run-app-blackbox.sh` | UTM installed GUI |

### Layout / UI

| Script | Role |
|--------|------|
| `dist-layout.sh` | Paths + `LATEST-INSTALLERS.txt` |
| `sync-admin-ui.sh` | Admin UI → Tauri dist |

## Lessons learned (do not repeat)

| Mistake | Correct approach |
|---------|------------------|
| Ran IExpress `package-windows-setup.sh` | **Removed.** NSIS via `package-windows-gui.sh` / `package.ps1` only |
| Reused old `*-setup.exe` when NSIS build failed | `build-windows-desktop.ps1` fails hard; `clean-dist.sh` first |
| GUI showed old admin UI (semver matched) | `/health` `ui=` digest; rebuild smr-cli after UI change |
| Timestamped logs filled `dist/` | Fixed log names; `clean-dist.sh`; verbose logs in `test-runs/` |
| Wrong DMG name `*_aarch64.dmg` | Canonical: `SafeRoute_{v}_arm64.dmg` |
| NSIS test as SYSTEM/utmctl | Use windows-user SSH + `windows-nsis-install-test.ps1` |
| 2.6GB VM disk from Rust cache | `./scripts/clean-vm.sh` after build cycles |
| SSH `Connection reset` | ControlMaster via `vm-ssh.sh`; manual fix in UTM console only |
| Forgot `CARGO_TARGET_DIR` | Always `$PWD/target` |
| Committed API keys | Only `config/test.env` (gitignored) |

## Deprecated

| Old | Use instead |
|-----|-------------|
| `package-windows-setup.sh` | removed |
| `windows-release` skill | `release-cycle` skill |
| `-Phase install-smoke` | `-Phase install` |
| `-Phase full-tests` | `-Phase test` |
| `dist/SafeRoute-*-x64-Setup.exe` | `dist/SafeRoute_{v}_x64-setup.exe` |

## CI → release upload

```bash
gh release upload vX.Y.Z \
  dist/smr-X.Y.Z-darwin-*.tar.gz \
  dist/SafeRoute_X.Y.Z_arm64.dmg \
  dist/smr-X.Y.Z-windows-x86_64.zip \
  dist/SafeRoute_X.Y.Z_x64-setup.exe \
  --clobber
```

Paths must match `dist/LATEST-INSTALLERS.txt`.
