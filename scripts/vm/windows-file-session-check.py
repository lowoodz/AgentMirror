#!/usr/bin/env python3
"""File-session DLP smoke against an already-running smr on :8080 (Windows UTM guest)."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

_staging = os.environ.get("SMR_GUEST_STAGING", "").strip()
if not _staging:
    _staging = str(Path(os.environ.get("USERPROFILE", "")) / "smr-staging")
sys.path.insert(0, _staging)

from test_common import dlp_after_chat, http, warm_file_index  # noqa: E402

BASE = "http://127.0.0.1:8080"
SECRETS = Path(_staging) / "smr-secrets"
FILE_SECRET = "UNIQUEINSTALLFILESECRETXYZ998877" + ("X" * 33)


def main() -> int:
    probe = SECRETS / "probe.txt"
    probe.write_text(FILE_SECRET, encoding="utf-8")
    if not warm_file_index(BASE):
        print("FAIL file_index_not_ready", file=sys.stderr)
        return 1

    probe_path = str(probe).replace("\\", "/")
    session = "win-install-file-session"

    code_trigger, _, ms_trigger = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body={
            "model": "deepseek-chat",
            "messages": [
                {"role": "user", "content": "Read the probe file"},
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
                },
            ],
            "max_tokens": 8,
        },
        headers={"X-SMR-Session-Id": session},
    )

    result: dict[str, object] = {}

    def do_chat() -> None:
        code, text, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body={
                "model": "deepseek-chat",
                "messages": [
                    {"role": "user", "content": f"file probe secret: {FILE_SECRET}"},
                ],
                "max_tokens": 16,
            },
            headers={"X-SMR-Session-Id": session},
        )
        result.update(code=code, ms=ms, text=text)

    dlp = dlp_after_chat(BASE, session, do_chat)
    code = int(result.get("code", 0))
    ms = float(result.get("ms", 0.0))
    ok = code == 200 and code_trigger == 200 and dlp > 0
    print(
        f"file_session_dlp ok={ok} trigger={code_trigger} code={code} "
        f"ms={ms:.0f} dlp={dlp} trigger_ms={ms_trigger:.0f}"
    )
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
