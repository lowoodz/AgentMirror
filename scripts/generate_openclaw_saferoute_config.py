#!/usr/bin/env python3
"""Write minimal OpenClaw config pointing at local SafeRoute (matrix / VM E2E)."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path


def openclaw_config_path(explicit: Path | None = None) -> Path:
    if explicit:
        return explicit
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA", "").strip()
        if appdata:
            return Path(appdata) / ".openclaw" / "openclaw.json"
    return Path.home() / ".openclaw" / "openclaw.json"


def saferoute_model(model_id: str, name: str) -> dict:
    return {
        "id": model_id,
        "name": name,
        "reasoning": False,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 128000,
        "maxTokens": 8192,
    }


def render_config(base_url: str = "http://127.0.0.1:8080/v1") -> dict:
    tiers = [
        ("saferoute-high", "SafeRoute High"),
        ("saferoute-medium", "SafeRoute Medium"),
        ("saferoute-lite", "SafeRoute Lite"),
    ]
    models = [saferoute_model(mid, name) for mid, name in tiers]
    allow = {f"saferoute/{mid}": {"alias": name.split()[-1]} for mid, name in tiers}
    return {
        "models": {
            "mode": "merge",
            "providers": {
                "saferoute": {
                    "baseUrl": base_url.rstrip("/"),
                    "apiKey": "dummy",
                    "api": "openai-completions",
                    "models": models,
                }
            },
        },
        "agents": {
            "defaults": {
                "model": {"primary": "saferoute/saferoute-high"},
                "models": allow,
            }
        },
        "gateway": {
            "mode": "local",
            "port": 18789,
            "bind": "loopback",
            "auth": {"mode": "token", "token": "smr-matrix-openclaw-token"},
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, help="openclaw.json path")
    parser.add_argument(
        "--base-url",
        default="http://127.0.0.1:8080/v1",
        help="SafeRoute OpenAI base URL",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Overwrite existing openclaw.json",
    )
    args = parser.parse_args()

    out = openclaw_config_path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.is_file() and not args.force:
        print(f"exists: {out}")
        return 0

    out.write_text(
        json.dumps(render_config(args.base_url), indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
