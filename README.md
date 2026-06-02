# SecureModelRoute

**SecureModelRoute** is a local security proxy for LLM clients. Point your IDE or agent at `http://127.0.0.1:8080/v1` instead of a cloud API directly. The proxy adds **model routing with automatic fallback**, **data-loss prevention (DLP)**, **operation safety**, and **protected-path rules**—with a built-in Web admin UI and an optional system-tray desktop app.

**中文文档:** [README.zh-CN.md](README.zh-CN.md)

## Features

| Area | What it does |
|------|----------------|
| **Model routing** | Multiple fallback groups (`high` / `medium` / `low`). Endpoints are tried in order on upstream errors, malformed JSON, or missing first stream token. OpenAI ↔ Anthropic request/response conversion. |
| **DLP — content** | Full-text or fragment rules (`min_fragment_len`, `min_fragment_ratio`). Secrets are replaced with random tokens while preserving case where possible. Optional built-in credential presets (`sk-`, `AKIA`, `ghp_`, …). |
| **DLP — files** | Disk-backed index (SQLite signatures + Bloom filter + byte verification) for large corpora. Streaming chunked indexing; `notify` rebuilds on file changes. |
| **SessionGuard** | Activates when **tool_call** / **tool_result** text references protected paths/files. Redacts matching content for the next **N** requests (`trigger_window`). |
| **Operation security** | Inspects tool-related fields on **request and response** (`observe` or `enforce`). Rules by `command_exec`, `api_call`, `network_access` + keywords. |
| **Path protection** | `deny_delete` / `deny_modify` / `deny_access` on paths; directories cover descendants. |
| **Web UI** | Configure routing, DLP, path rules, and ops at `/ui`. Saves YAML and hot-reloads while keeping SessionGuard state. |
| **Desktop app (optional)** | Tauri tray app embeds the proxy; closing the window hides to tray/menu bar without stopping the service. |
| **Audit** | Structured request audit in SQLite; live events via API. |

Global master switch: `pipeline.security_enabled` (also in the Web UI header). When off, DLP and operation security are disabled.

## Quick start

### Build and install (from source)

```bash
chmod +x scripts/install.sh
./scripts/install.sh           # CLI → ~/.local/bin
./scripts/install.sh --gui     # CLI + tray desktop app
./scripts/install.sh --all     # CLI + GUI + login autostart (tray-only)

securemodelroute               # start proxy and open admin UI
```

**Windows (PowerShell):**

```powershell
.\install.ps1 -All             # CLI + tray GUI + login shortcut
securemodelroute
```

### Release archives

Extract a platform tarball from `dist/` and run `./install.sh` (macOS/Linux) or `.\install.ps1` (Windows). See packaging scripts under `scripts/` if you build releases yourself.

### Point your client at the proxy

Default listen address: `127.0.0.1:8080` (see `server.listen` in config).

| URL | Purpose |
|-----|---------|
| `http://127.0.0.1:8080/v1` | OpenAI-compatible API |
| `http://127.0.0.1:8080/v1/messages` | Anthropic Messages API |
| `http://127.0.0.1:8080/ui` | Web admin |
| `http://127.0.0.1:8080/health` | Health check |

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080/v1", api_key="dummy")
```

Optional headers:

- `X-SMR-Fallback-Group` — e.g. `high`, `medium`, `low`
- `X-SMR-Session-Id` — ties SessionGuard and audit to a session (auto-generated if omitted)

Anthropic SDK: `base_url="http://127.0.0.1:8080"`, path `/v1/messages`.

### Config locations

| Platform | Typical path |
|----------|----------------|
| macOS / Linux (install script) | `~/.local/etc/securemodelroute/smr.yaml` |
| macOS / Linux (`smr` directly) | `~/.config/securemodelroute/smr.yaml` |
| Windows | `%APPDATA%\securemodelroute\smr.yaml` |

Override with `SMR_CONFIG=/path/to/smr.yaml`. Store upstream API keys in environment variables (`api_key_env`), not in committed files.

## Configuration overview

Full example: [`config/smr.example.yaml`](config/smr.example.yaml).

```yaml
server:
  listen: "127.0.0.1:8080"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce   # observe | enforce
  builtin_credential_presets: true

fallback_groups:
  high:
    - id: openai-primary
      base_url: "https://api.openai.com/v1"
      model: "gpt-4o-mini"
      api_key_env: OPENAI_API_KEY
      protocol: openai              # openai | anthropic (optional; inferred from URL)
      timeout_secs: 120
    - id: anthropic-fallback
      base_url: "https://api.anthropic.com/v1"
      model: "claude-sonnet-4-20250514"
      protocol: anthropic
      api_key_env: ANTHROPIC_API_KEY

content_rules:
  - id: example-secret
    enabled: true
    match_mode: full
    category: secret
    value: "replace-with-placeholder"

file_rules:
  - id: corp-docs
    path: /data/docs
    enabled: true
    recursive: true
    trigger_window: 5
    match_mode: fragment
    min_fragment_len: 32
    formats: [txt, md, json, yaml, rs, py]   # extend as needed
```

**`fallback_groups`** — Named lists of `ModelEndpoint`. The default group is `server.default_fallback_group`. Streaming locks to the current endpoint after the first content token (no further fallback).

**`content_rules`** — In-memory patterns for request/response JSON fields. Use for secrets, phrases, and **extensionless** sensitive strings that file indexing cannot match by suffix alone.

**`file_rules`** — Directories or files to index and protect. Important notes:

- **`formats`** — File extensions **without** a leading dot (e.g. `txt`, `md`). Add any suffix your corpus uses; only matching extensions are indexed.
- **Extensionless files** — Not selected by `formats`. Add equivalent sensitive text via **`content_rules`** instead.
- **`trigger_window`** — After SessionGuard triggers on a tool mentioning a protected file, redaction applies for this many subsequent requests.
- **`index`** — Tunables for chunk size, Bloom size, workers, haystack limits, ripgrep prefilter, etc. (see example in legacy doc or `config/smr.example.yaml`).

**`operation_rules`** / **`path_protection_rules`** — Operation keywords and path-level deny levels.

```bash
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
export SMR_CONFIG=/path/to/smr.yaml
```

## File DLP index (basics)

Large text corpora are indexed on disk instead of loading everything into RAM.

**Index root:** `~/.config/securemodelroute/file-index/{rule_id}/` (platform config dir + `file-index`).

| Artifact | Role |
|----------|------|
| `current.json` | Active generation pointer |
| `gen/{n}/index.db` | Signature → file offset/length |
| `gen/{n}/bloom.bin` | In-memory Bloom prefilter |
| `gen/{n}/files.json` | Per-file mtime/size for incremental rebuild |
| `gen/{n}/manifest.json` | Build stats |
| `gen/{n}/literals.json` | Samples for optional ripgrep prefilter |

**Runtime flow:**

1. Background indexing for each enabled `file_rules` entry. DLP uses the index when `/api/status` reports `file_index_ready: true`.
2. **tool_call** / **tool_result** mentioning a protected **file** under a rule activates SessionGuard for that session.
3. For the next `trigger_window` requests, haystack fields are scanned: Bloom → SQLite candidates → read source bytes → redact on match.

SessionGuard stores rule metadata only—not full corpus text in memory.

## Web admin UI

Open **`http://127.0.0.1:8080/ui`** (port from `server.listen`).

| Tab | Purpose |
|-----|---------|
| Overview | Proxy URL, default group, DLP/ops status, file index readiness |
| Model routing | Three fallback groups; drag to reorder; save applies hot reload |
| DLP | File zones (paths) + content tags (secrets/phrases) |
| Path protection | Path + deny level table |
| Operation rules | Behavior type + keyword patterns |
| Logs | SQLite request audit + live events |
| Advanced YAML | Full config edit, save, reload from disk |

**Internationalization:** UI strings are available in **English** and **中文**; switch language from the header control.

**Admin API (selected):**

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/status` | Listen address, security flags, index readiness, proxy URL |
| GET/PUT | `/api/config` | Read/write YAML; PUT saves and hot-reloads |
| GET | `/api/events?limit=50` | Recent security/DLP events |
| GET | `/api/audits?limit=50` | Request audit rows |
| PUT | `/api/reload` | Reload config from disk (keeps SessionGuard) |

## Traffic body snapshots (debug)

To persist JSON request/response bodies for troubleshooting redaction and routing:

```yaml
logging:
  level: info
  redact_content: true
  save_traffic_bodies: true      # default: false
  traffic_max_body_bytes: 20971520  # 20 MiB per snapshot file
```

When `save_traffic_bodies` is enabled, proxied JSON bodies are written to `{config_dir}/traffic/` (up to `traffic_max_body_bytes`, hard cap 20 MiB). The admin UI shows a preview and links to the full file—use only on trusted machines and disable in production.

## Development and testing

```bash
cargo test
cargo clippy -- -D warnings
./scripts/verify.sh
```

For live and black-box tests against real upstreams, copy the example key file (gitignored):

```bash
cp test_model_api_key.example.txt test_model_api_key.txt
# Edit test_model_api_key.txt with your keys — never commit it
```

```bash
python3 scripts/install_functional_test.py
python3 scripts/blackbox_test.py
./scripts/run_all_tests.sh          # macOS/Linux
.\scripts\run_all_tests.ps1         # Windows
```

Implementation checklist: [TODO.md](TODO.md). Previous Chinese README snapshot: [docs/README.legacy.zh-CN.md](docs/README.legacy.zh-CN.md).

## License

MIT
