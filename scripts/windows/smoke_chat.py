#!/usr/bin/env python3
"""Minimal SafeRoute model smoke test (Windows or any OS).

Requires SafeRoute already running with upstream keys in smr.yaml.

  python scripts\\windows\\smoke_chat.py

Optional env:
  SMR_BASE=http://127.0.0.1:8080
  SMR_MODEL=glm-4-flash
  SMR_PROMPT=Reply with exactly: smr-ok
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request

BASE = os.environ.get("SMR_BASE", "http://127.0.0.1:8080").rstrip("/")
MODEL = os.environ.get("SMR_MODEL", "glm-4-flash")
PROMPT = os.environ.get("SMR_PROMPT", "Reply with exactly: smr-ok")


def http_get(url: str, timeout: float = 10.0) -> tuple[int, str]:
    req = urllib.request.Request(url, method="GET")
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.status, resp.read().decode("utf-8", errors="replace")


def http_post_json(url: str, body: dict, timeout: float = 90.0) -> tuple[int, dict]:
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Content-Type": "application/json",
            "Authorization": "Bearer dummy",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.status, json.loads(resp.read().decode("utf-8", errors="replace"))


def main() -> int:
    health_url = f"{BASE}/health"
    print(f"==> GET {health_url}")
    try:
        code, text = http_get(health_url)
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        print(f"FAIL: SafeRoute not reachable ({exc})", file=sys.stderr)
        print("Start SafeRoute (tray) and check SMR_BASE / listen port.", file=sys.stderr)
        return 1
    if code != 200 or "OK" not in text:
        print(f"FAIL: health status={code} body={text!r}", file=sys.stderr)
        return 1
    print(f"OK: {text.strip()}")

    chat_url = f"{BASE}/v1/chat/completions"
    payload = {
        "model": MODEL,
        "messages": [{"role": "user", "content": PROMPT}],
        "max_tokens": 32,
    }
    print(f"==> POST {chat_url} model={MODEL!r}")
    try:
        code, out = http_post_json(chat_url, payload)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        print(f"FAIL: HTTP {exc.code} {detail[:500]}", file=sys.stderr)
        return 1
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        print(f"FAIL: chat request error ({exc})", file=sys.stderr)
        return 1

    if code != 200:
        print(f"FAIL: status={code} body={out}", file=sys.stderr)
        return 1

    reply = (out.get("choices") or [{}])[0].get("message", {}).get("content", "")
    print(f"OK: reply={reply!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
