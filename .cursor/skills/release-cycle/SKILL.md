---
name: release-cycle
description: >-
  Build, package, test, clean-install, and post-install-test LLM-SafeRoute on
  macOS and Windows. Use when compiling releases, running full verification,
  packaging DMG/NSIS, uninstalling old installs, GitHub release uploads, or
  debugging platform-specific build/test failures. Always read this skill before
  touching scripts/package*, release-cycle*, or dist/ artifacts.
---

# LLM-SafeRoute Release Cycle

**Pipeline:** `preflight → clean → compile → package → verify → test → install → installed`

Agents and humans must reuse the scripts below — do not ad-hoc `cargo build`, IExpress, or timestamped logs in `dist/`.

## Which command?

| Goal | Command |
|------|---------|
| **Mac: full validation before release** | `./scripts/release-full.sh` |
| **Mac: macOS-only cycle** | `./scripts/release-cycle.sh` |
| **Mac: build all dist artifacts** | `./scripts/package-all.sh` (`--clean` recommended) |
| **Windows native full cycle** | `.\scripts\windows\release-cycle.ps1` |
| **Clean stale dist + UTM guest** | `./scripts/clean-dist.sh` |
| **Single phase** | `./scripts/release-cycle.sh verify` or `-Phase verify` |

Always set build output:

```bash
export CARGO_TARGET_DIR="$PWD/target"
```

```powershell
$env:CARGO_TARGET_DIR = "$PWD\target"
```

## Phase map

| Phase | Purpose | macOS script | Windows script |
|-------|---------|--------------|----------------|
| **preflight** | Toolchain, `CARGO_TARGET_DIR` | `macos/preflight.sh` | `windows/preflight.ps1` |
| **clean** | Kill :8080, uninstall old | `uninstall.sh` | `uninstall.ps1` |
| **compile** | Sync UI + unit/smoke tests | `sync-admin-ui.sh` + `verify.sh` | copy UI + `verify.ps1` |
| **package** | Build `dist/` artifacts | `package-macos.sh` / `package-all.sh` | `package.ps1` |
| **verify** | Check dist files exist | `macos/verify-package.sh` | `windows/verify-package.ps1` |
| **test** | Live API functional/blackbox/stress | Python tests in `release-cycle` | `run_all_tests.ps1` |
| **install** | Fresh install from dist + health | `macos/install-smoke.sh` | `vm/windows-install-smoke.ps1` |
| **installed** | Tray + attach blackbox | `run_installed_app_tests.sh` | UTM blackbox from Mac |

### Mac + UTM (Windows artifacts)

When UTM guest is running on Apple Silicon Mac:

| Step | Script |
|------|--------|
| Windows NSIS + GUI exe | `vm/package-windows-gui.sh` (called by `package-all.sh`) |
| Windows CLI zip | `package-windows.sh` |
| VM functional + NSIS + blackbox | `vm/utm-run-all-tests.sh` (called by `release-full.sh`) |
| VM installed-app blackbox | `vm/utm-run-app-blackbox.sh` (via `run_installed_app_tests.sh`) |
| Clean VM staging (~GB cache) | `clean-vm.sh` (via `clean-dist.sh`) |

See `scripts/vm/WINDOWS_VM.md` for VM SSH setup.

## One-command full runs

### macOS host — complete (recommended before GitHub release)

```bash
chmod +x scripts/release-full.sh scripts/release-cycle.sh scripts/macos/*.sh scripts/clean-*.sh
export CARGO_TARGET_DIR="$PWD/target"
./scripts/release-full.sh
```

Options: `--cli-only`, `--skip-clean`, `--skip-tests`, `--skip-vm`, `--package-only`, `--keep-config`

Log: `dist/macos-release-full.log`

### macOS host — local platform only

```bash
./scripts/release-cycle.sh
```

Log: `dist/macos-release-cycle.log`

### Windows x86_64 native

```powershell
Set-ExecutionPolicy Bypass -Scope Process -Force
$env:CARGO_TARGET_DIR = "$PWD\target"
.\scripts\windows\release-cycle.ps1
```

Log: `dist/windows-release-cycle.log`

## Optional artifacts

CLI is **always** built and verified. App + installer are optional (default **on** in full cycle).

| Artifact | macOS path | Windows path |
|----------|------------|--------------|
| CLI | `dist/smr-{v}-darwin-{arch}.tar.gz` | `dist/smr-{v}-windows-x86_64.zip` |
| App | `dist/smr-{v}-darwin-{arch}-app.tar.gz` | `dist/smr-{v}-windows-x86_64-app.zip` |
| Installer | `dist/SafeRoute_{v}_{arch}.dmg` | `dist/SafeRoute_{v}_x64-setup.exe` |
| UTM staging | — | `dist/windows-desktop/SafeRoute.exe`, `dist/smr.exe` |

**macOS flags:** `--with-app` / `--without-app`, `--with-dmg` / `--without-dmg`, `--cli-only`

**Windows flags:** `-WithApp` / `-WithoutApp`, `-WithSetup` / `-WithoutSetup`, `-CliOnly`

When CLI-only: skip tray/GUI installed tests automatically.

## Expected `dist/` layout

Canonical paths: `scripts/dist-layout.sh` → regenerated **`dist/LATEST-INSTALLERS.txt`** after every package/clean.

**Fixed logs** (overwrite each run — never create `package-all-YYYYMMDD.log` in `dist/` root):

```
dist/macos-release-cycle.log
dist/macos-release-full.log
dist/macos-install-smoke.log
dist/windows-release-cycle.log
dist/windows-desktop-build.log
dist/windows-nsis-install-test.log
dist/test-runs/          # detailed per-run logs OK here
```

**Clean:**

```bash
./scripts/clean-dist.sh    # dist/ + VM guest (VM SSH)
./scripts/clean-vm.sh      # VM only
```

## Agent checklist (always — avoid repeated mistakes)

1. **`CARGO_TARGET_DIR=$PWD/target`** — never a stale sandbox path.
2. **`sync-admin-ui.sh`** before any package (compile phase does this).
3. **`clean-dist.sh`** before package when artifacts may be stale.
4. **`uninstall.sh` / `uninstall.ps1`** before install tests — port :8080 must be free.
5. **NSIS only** — `package-windows-gui.sh` or `package.ps1`. IExpress scripts **removed**; never produce `SafeRoute-*-x64-Setup.exe`.
6. **NSIS build must fail** if Tauri fails — `build-windows-desktop.ps1` clears `bundle/nsis`; do not reuse old `*-setup.exe`.
7. **Stale GUI server** — same semver can hide old admin UI on :8080. `/health` includes `ui=` digest; GUI refuses mismatch. Rebuild CLI after UI edits.
8. **Tauri on Windows:** `tauri build --bundles nsis --ci` — never `--silent`.
9. **NSIS silent install in VM** — run via VM SSH (`windows-nsis-install-test.ps1`), not SYSTEM/utmctl.
10. **`config/test.env`** for live tests — gitignored; never commit API keys.
11. **Log to fixed names** under `dist/` — use `dist/test-runs/` for verbose timestamped logs.

## Common failures

| Symptom | Fix |
|---------|-----|
| Old UI after install | Rebuild smr + repackage; check `/health` `ui=` digest |
| Wrong Windows installer (IExpress) | Use NSIS only; run `clean-dist.sh` |
| NSIS missing after VM build | Read `dist/windows-desktop-build.log`; fix compile; do not copy old setup |
| `unexpected argument '--silent'` | Use `--ci` not `--silent` |
| Port 8080 in use | Run clean / uninstall phase |
| Live tests skipped | Create `config/test.env` from example |
| VM full of build cache | `./scripts/clean-vm.sh` |
| SSH `Connection reset` | Use ControlMaster (`vm-ssh.sh`); fix SSH manually in UTM console |

## GitHub release

1. Bump version in `Cargo.toml`, `gui/package.json`, `gui/src-tauri/tauri.conf.json`.
2. Mac: `./scripts/release-full.sh` (or `release-cycle.sh` + `package-all.sh --clean`).
3. Windows artifact: GHA `.github/workflows/windows-nsis.yml` or native `release-cycle.ps1`.
4. Upload from `dist/LATEST-INSTALLERS.txt` list only.
5. Do **not** upload `config/smr.yaml`, `config/test.env`, or personal paths.

## Script index

| Path | Role |
|------|------|
| **`release-full.sh`** | **Mac: full pipeline (all platforms + UTM)** |
| `release-cycle.sh` | Cross-platform entry → macOS orchestrator |
| `macos/release-cycle.sh` | macOS phase orchestrator |
| `windows/release-cycle.ps1` | Windows phase orchestrator |
| `package-all.sh` | Mac: mac tarballs + win zip + UTM NSIS (`--clean`, `--cli-only`) |
| `package-macos.sh` | macOS CLI + app + DMG |
| `package-windows.sh` | Cross-compile Windows CLI zip |
| `vm/package-windows-gui.sh` | UTM: Tauri NSIS + `SafeRoute.exe` |
| `package.ps1` | Windows native package |
| `dist-layout.sh` | Canonical paths + manifest writer |
| `clean-dist.sh` | Clean dist + invoke clean-vm |
| `clean-vm.sh` | UTM guest staging cleanup |
| `sync-admin-ui.sh` | `assets/index.html` → `gui/dist/` |
| `verify.sh` / `verify.ps1` | Compile-phase tests |
| `run_all_tests.sh` / `.ps1` | Dev test matrix (no install) |
| `run_full_tests.sh` | Host live tests + optional UTM |
| `run_installed_app_tests.sh` | Post-install tray + blackbox |
| `uninstall.sh` / `uninstall.ps1` | Clean uninstall |

Platform details, install paths, CI upload: [reference.md](reference.md)

Rule for automation: `.cursor/rules/release-cycle.mdc`
