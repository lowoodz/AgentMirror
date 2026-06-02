#!/usr/bin/env python3
"""Functional smoke tests against locally installed SecureModelRoute (default :8080)."""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_common import http, latest_audit, parse_keys  # noqa: E402

BASE = "http://127.0.0.1:8080"
CONTENT_SECRET = "LOCAL-INSTALL-TEST-SECRET"
FILE_SECRET = "LOCAL-INSTALL-FILE-SECRET-XYZ"


def check(name: str, ok: bool, detail: str) -> bool:
    mark = "PASS" if ok else "FAIL"
    print(f"[{mark}] {name}: {detail}")
    return ok


def main() -> int:
    results: list[bool] = []

    code, text, _ = http("GET", f"{BASE}/health")
    results.append(check("health", code == 200 and "OK" in text, f"status={code}"))

    code, text, _ = http("GET", f"{BASE}/api/status")
    status = json.loads(text) if code == 200 else {}
    results.append(
        check(
            "status_api",
            code == 200 and status.get("file_index_ready") is True,
            f"security={status.get('security_enabled')}, index={status.get('file_index_ready')}",
        )
    )

    code, ui, _ = http("GET", f"{BASE}/ui")
    results.append(check("web_ui", code == 200 and "SafeRoute" in ui, f"bytes={len(ui)}"))

    code, text, ms = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body={
            "model": "glm-4-flash",
            "messages": [{"role": "user", "content": "Reply exactly: install-ok"}],
            "max_tokens": 16,
        },
    )
    reply = ""
    if code == 200:
        reply = json.loads(text)["choices"][0]["message"]["content"]
    results.append(check("chat_glm", code == 200 and len(reply) > 0, f"{ms:.0f}ms reply={reply[:30]!r}"))

    code, raw, ms = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body={
            "model": "glm-4-flash",
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
            "model": "glm-4-flash",
            "messages": [{"role": "user", "content": f"My secret is {CONTENT_SECRET}"}],
            "max_tokens": 12,
        },
        headers={"X-SMR-Session-Id": "local-install-dlp"},
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
    results.append(
        check(
            "fallback",
            code == 200 and len(chain) >= 2,
            f"{ms:.0f}ms chain={chain}",
        )
    )

    code, text, ms = http(
        "POST",
        f"{BASE}/v1/messages",
        body={
            "model": "glm-4-flash",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "Say hi"}],
        },
        headers={"X-SMR-Fallback-Group": "glm-anthropic"},
    )
    results.append(check("anthropic_api", code == 200 and "content" in text, f"{ms:.0f}ms status={code}"))

    code, audits, _ = http("GET", f"{BASE}/api/audits?limit=3")
    n = len(json.loads(audits).get("audits", [])) if code == 200 else 0
    results.append(check("audit_log", code == 200 and n > 0, f"records={n}"))

    passed = sum(results)
    total = len(results)
    print(f"\n合计: {passed}/{total} 通过")
    return 0 if passed == total else 1


if __name__ == "__main__":
    raise SystemExit(main())
