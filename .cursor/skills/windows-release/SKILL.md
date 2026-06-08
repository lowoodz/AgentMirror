---
name: windows-release
description: >-
  Deprecated alias — use the release-cycle skill instead. Windows NSIS/Tauri
  packaging is documented in .cursor/skills/release-cycle/.
---

# Windows Release (deprecated)

Use **[release-cycle](../release-cycle/SKILL.md)** — the single source of truth for macOS + Windows.

| Host | Full validation |
|------|-----------------|
| macOS | `./scripts/release-full.sh` |
| Windows | `.\scripts\windows\release-cycle.ps1` |

Windows-only quick entry:

```powershell
Set-ExecutionPolicy Bypass -Scope Process -Force
$env:CARGO_TARGET_DIR = "$PWD\target"
.\scripts\windows\release-cycle.ps1              # full: CLI + app + NSIS
.\scripts\windows\release-cycle.ps1 -CliOnly     # CLI zip only
```

Artifact flags mirror macOS `--with-app`, `--without-dmg`, `--cli-only` — see [release-cycle/SKILL.md](../release-cycle/SKILL.md).
