# Windows release scripts (package / install / test)

Run these on **Windows x86_64** (native or GitHub Actions). Unified macOS + Windows skill: `.cursor/skills/release-cycle/SKILL.md`.

## One command (recommended)

```powershell
Set-ExecutionPolicy Bypass -Scope Process -Force
cd C:\path\to\LLM-SafeRoute
$env:CARGO_TARGET_DIR = "$PWD\target"
.\scripts\windows\release-cycle.ps1
```

Default **full** cycle: CLI zip + app zip + NSIS setup + install smoke (CLI + GUI) + live tests.

**CLI-only** (no Tauri / NSIS / app zip):

```powershell
.\scripts\windows\release-cycle.ps1 -CliOnly
```

Phases: preflight → uninstall old → compile → package → verify → test → install → installed.

Single phase:

```powershell
.\scripts\windows\release-cycle.ps1 -Phase package
.\scripts\windows\release-cycle.ps1 -Phase verify -WithoutApp -WithSetup
.\scripts\windows\release-cycle.ps1 -Phase install -CliOnly
```

## Options

| Flag | Purpose |
|------|---------|
| `-WithApp` / `-WithoutApp` | Require or skip app zip; GUI install smoke when app on |
| `-WithSetup` / `-WithoutSetup` | Require or skip NSIS setup in verify |
| `-CliOnly` | Shorthand: `-WithoutApp -WithoutSetup` |
| `-SkipClean` | Skip uninstall / process kill |
| `-SkipTests` | Skip live API tests (blackbox/stress) |
| `-SkipInstalled` | Skip post-install tray tests (also auto-skipped with `-CliOnly`) |
| `-KeepConfigOnClean` | Keep `~/.local/etc/securemodelroute` on uninstall |
| `-InstallPrefix` | Custom prefix for install-smoke |

Legacy aliases: `-Phase install-smoke` → `install`, `-Phase full-tests` → `test`.

Logs: `dist/windows-release-cycle.log`, `dist/windows-install-smoke.log`.

## Individual scripts

| Script | Purpose |
|--------|---------|
| `preflight.ps1` | cargo, npm, NSIS, Python, `CARGO_TARGET_DIR` |
| `verify-package.ps1` | Check CLI zip; optional `-RequireSetup` / `-RequireAppZip` |
| `release-cycle.ps1` | Orchestrator |
| `common.ps1` | Shared helpers (sourced by others) |
| `ensure-nsis-tools.ps1` | Locate/install `makensis.exe` |
| `prepare-nsis-bundle.ps1` | Stage CLI into Tauri NSIS bundle |

Lower-level (also used by CI):

| Script | Purpose |
|--------|---------|
| `../package.ps1` | Build CLI + Tauri NSIS + zip (`-CliOnly` skips app/NSIS) |
| `../install.ps1` | Install CLI / run NSIS setup |
| `../uninstall.ps1` | NSIS silent uninstall + remove portable install |
| `../verify.ps1` | Unit tests + health smoke |
| `../run_all_tests.ps1` | verify + functional + blackbox + stress |
| `../vm/windows-install-smoke.ps1` | Install from dist zip (`-CliOnly` for CLI-only) |

## macOS host → Windows VM

See [../vm/WINDOWS_VM.md](../vm/WINDOWS_VM.md). NSIS install tests must run in an **interactive user session** (not SYSTEM); use `scripts/vm/utm-run-nsis-install-test.sh`.

## Agent skill

Automation / release docs: `.cursor/skills/release-cycle/SKILL.md` (macOS + Windows optional artifacts). Legacy alias: `windows-release`.
