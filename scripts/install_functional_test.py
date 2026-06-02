#!/usr/bin/env python3
"""Self-contained install smoke test (starts smr, same coverage as Windows UTM functional checks).

Full file DLP edge cases (scoped sibling, directory-only, parent/child rules) live in
scripts/blackbox_test.py (27 scenarios).
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_common import (  # noqa: E402
    KEYS_FILE,
    http,
    latest_audit,
    parse_keys,
    start_smr,
    stop_smr,
    wait_ready,
)

PORT = int(os.environ.get("SMR_INSTALL_TEST_PORT", "18082"))
BASE = f"http://127.0.0.1:{PORT}"
CONTENT_SECRET = "LOCAL-INSTALL-TEST-SECRET"
FILE_PROBE_SECRET = "LOCAL-INSTALL-FILE-SECRET-XYZ"


def check(name: str, ok: bool, detail: str) -> bool:
    mark = "PASS" if ok else "FAIL"
    print(f"[{mark}] {name}: {detail}")
    return ok


def build_config(glm: str, ds: str, secrets: Path, vault: Path) -> str:
    secrets_s = str(secrets).replace("\\", "/")
    vault_s = str(vault).replace("\\", "/")
    return f"""server:
  listen: "127.0.0.1:{PORT}"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce
  builtin_credential_presets: true

logging:
  level: info
  redact_content: true

fallback_groups:
  high:
    - id: glm-primary
      base_url: "https://open.bigmodel.cn/api/coding/paas/v4"
      model: "glm-4-flash"
      api_key: "{glm}"
      protocol: openai
      timeout_secs: 90
    - id: deepseek-fallback
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds}"
      protocol: openai
      timeout_secs: 90
  fallback-test:
    - id: dead-endpoint
      base_url: "http://127.0.0.1:9"
      model: "fake-model"
      api_key: "dead"
      timeout_secs: 3
    - id: deepseek-rescue
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds}"
      protocol: openai
      timeout_secs: 90
  glm-anthropic:
    - id: ds-anthropic
      base_url: "https://api.deepseek.com/anthropic"
      model: "deepseek-chat"
      api_key: "{ds}"
      protocol: anthropic
      timeout_secs: 90

content_rules:
  - id: install-test-secret
    enabled: true
    match_mode: full
    category: secret
    value: "{CONTENT_SECRET}"

file_rules:
  - id: install-secrets
    enabled: true
    path: "{secrets_s}"
    recursive: true
    trigger_window: 5
    match_mode: full
    formats: ["txt"]

path_protection_rules:
  - id: install-protected-secrets
    enabled: true
    path: "{secrets_s}"
    level: deny_access
  - id: install-protected-vault
    enabled: true
    path: "{vault_s}"
    level: deny_access

operation_rules:
  - id: block-rm-rf
    enabled: true
    operation: command_exec
    object:
      pattern: "rm -rf"
      is_regex: false
"""


def main() -> int:
    if not KEYS_FILE.exists():
        print(f"Missing {KEYS_FILE}", file=sys.stderr)
        return 1

    glm, ds = parse_keys()
    secrets = Path(tempfile.mkdtemp(prefix="smr-install-secrets-"))
    vault = secrets / "vault"
    vault.mkdir()
    (secrets / "probe.txt").write_text("LOCAL-INSTALL-FILE-SECRET-XYZ", encoding="utf-8")
    (secrets / "project.txt").write_text("project-data", encoding="utf-8")
    (vault / "secret.txt").write_text("vault-secret-data", encoding="utf-8")

    proc = None
    cfg_file: Path | None = None
    results: list[bool] = []

    try:
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(build_config(glm, ds, secrets, vault))
            cfg_file = Path(f.name)

        print(f"==> Install functional test @ {BASE}")
        proc = start_smr(cfg_file)
        time.sleep(1.0)
        results.append(check("service_ready", wait_ready(BASE), "ready"))

        code, text, _ = http("GET", f"{BASE}/health")
        results.append(check("health", code == 200 and "OK" in text, f"value={text.strip()!r}"))

        code, text, _ = http("GET", f"{BASE}/api/status")
        status = json.loads(text) if code == 200 else {}
        results.append(
            check(
                "status_api",
                code == 200 and status.get("file_index_ready") is True,
                f"security={status.get('security_enabled')}",
            )
        )

        code, ui, _ = http("GET", f"{BASE}/ui")
        results.append(check("web_ui", code == 200 and "SafeRoute" in ui, f"bytes={len(ui)}"))

        code, text, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [{"role": "user", "content": "Reply exactly: install-ok"}],
                "max_tokens": 16,
            },
        )
        reply = json.loads(text)["choices"][0]["message"]["content"] if code == 200 else ""
        results.append(check("chat_route", code == 200 and len(reply) > 0, f"{ms:.0f}ms reply={reply[:30]!r}"))

        code, raw, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [{"role": "user", "content": "Count 1 2 3 briefly."}],
                "max_tokens": 24,
                "stream": True,
            },
            stream=True,
        )
        chunks = sum(1 for line in raw.splitlines() if line.startswith("data: ") and "[DONE]" not in line)
        results.append(check("streaming", code == 200 and chunks >= 1, f"{ms:.0f}ms chunks={chunks}"))

        code, _, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [{"role": "user", "content": f"My secret is {CONTENT_SECRET}"}],
                "max_tokens": 12,
            },
            headers={"X-SMR-Session-Id": "install-func-dlp"},
        )
        audit = latest_audit(BASE)
        dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
        results.append(check("content_dlp", code == 200 and dlp > 0, f"{ms:.0f}ms dlp={dlp}"))

        code, _, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [{"role": "user", "content": "Say ok"}],
                "max_tokens": 8,
            },
            headers={"X-SMR-Fallback-Group": "fallback-test"},
        )
        audit = latest_audit(BASE)
        chain = audit.get("fallback_chain", []) if audit else []
        results.append(check("fallback", code == 200 and len(chain) >= 2, f"{ms:.0f}ms chain={chain}"))

        code, text, ms = http(
            "POST",
            f"{BASE}/v1/messages",
            body={
                "model": "deepseek-chat",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "Say hi"}],
            },
            headers={"X-SMR-Fallback-Group": "glm-anthropic"},
        )
        results.append(check("anthropic_api", code == 200 and "content" in text, f"{ms:.0f}ms"))

        code, audits, _ = http("GET", f"{BASE}/api/audits?limit=3")
        n = len(json.loads(audits).get("audits", [])) if code == 200 else 0
        results.append(check("audit_log", code == 200 and n > 0, f"records={n}"))

        probe_path = str(secrets / "probe.txt").replace("\\", "/")
        http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [
                    {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "id": "call_probe",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": json.dumps({"path": probe_path}),
                                },
                            }
                        ],
                    }
                ],
                "max_tokens": 8,
            },
            headers={"X-SMR-Session-Id": "install-func-file-session"},
        )
        code, _, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [
                    {
                        "role": "user",
                        "content": f"file probe secret: {FILE_PROBE_SECRET}",
                    }
                ],
                "max_tokens": 16,
            },
            headers={"X-SMR-Session-Id": "install-func-file-session"},
        )
        audit = latest_audit(BASE)
        dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
        results.append(
            check("file_session_dlp", code == 200 and dlp > 0, f"{ms:.0f}ms dlp={dlp}")
        )

        path_str = str(secrets / "project.txt").replace("\\", "/")
        http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [
                    {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": json.dumps({"path": path_str}),
                                },
                            }
                        ],
                    }
                ],
                "max_tokens": 8,
            },
        )
        audit = latest_audit(BASE)
        blocks = int(audit.get("safety_blocks", 0)) if audit else 0
        results.append(check("path_protection", blocks > 0, f"blocks={blocks} path={path_str}"))

        passed = sum(results)
        total = len(results)
        print(f"\nSUMMARY: {passed}/{total} PASSED")
        return 0 if passed == total else 1
    finally:
        stop_smr(proc)
        if cfg_file and cfg_file.exists():
            cfg_file.unlink(missing_ok=True)
        for p in secrets.rglob("*"):
            if p.is_file():
                p.unlink()
        for p in sorted(secrets.rglob("*"), reverse=True):
            if p.is_dir():
                p.rmdir()
        secrets.rmdir()


if __name__ == "__main__":
    raise SystemExit(main())
