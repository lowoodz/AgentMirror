#!/usr/bin/env python3
"""Ensure OpenClaw saferoute provider models use reasoning=false for openai-completions."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path


def openclaw_config_path() -> Path:
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA", "").strip()
        if appdata:
            return Path(appdata) / ".openclaw" / "openclaw.json"
    return Path.home() / ".openclaw" / "openclaw.json"


def load_json5(path: Path) -> dict:
    text = path.read_text(encoding="utf-8")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        # OpenClaw allows JSON5-style trailing commas; strip them lightly.
        cleaned = text
        import re

        cleaned = re.sub(r",(\s*[}\]])", r"\1", cleaned)
        return json.loads(cleaned)


def restart_gateway() -> None:
    openclaw = shutil.which("openclaw") or shutil.which("openclaw.cmd")
    if not openclaw:
        print("WARN: openclaw not in PATH; restart gateway manually", file=sys.stderr)
        return
    try:
        proc = subprocess.run(
            [openclaw, "gateway", "restart"],
            capture_output=True,
            text=True,
            timeout=90,
            check=False,
        )
        if proc.returncode != 0:
            print(
                f"WARN: openclaw gateway restart exit {proc.returncode}: "
                f"{(proc.stderr or proc.stdout or '')[:200]}",
                file=sys.stderr,
            )
        else:
            print("==> openclaw gateway restarted")
            time.sleep(4)
    except (OSError, subprocess.TimeoutExpired) as exc:
        print(f"WARN: openclaw gateway restart failed: {exc}", file=sys.stderr)


def patch_reasoning(cfg: dict) -> int:
    providers = cfg.get("models", {}).get("providers", {})
    saferoute = providers.get("saferoute", {})
    models = saferoute.get("models", [])
    changed = 0
    for model in models:
        if not isinstance(model, dict):
            continue
        if model.get("reasoning") is not False:
            model["reasoning"] = False
            changed += 1
    return changed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, help="openclaw.json path")
    parser.add_argument("--restore", type=Path, help="Restore from backup")
    args = parser.parse_args()

    if args.restore:
        if not args.restore.is_file():
            print(f"Missing backup {args.restore}", file=sys.stderr)
            return 1
        cfg_path = args.config or openclaw_config_path()
        shutil.copy2(args.restore, cfg_path)
        print(f"Restored {cfg_path} from {args.restore}")
        return 0

    cfg_path = args.config or openclaw_config_path()
    if not cfg_path.is_file():
        print(f"WARN: missing OpenClaw config {cfg_path}", file=sys.stderr)
        return 0

    backup = cfg_path.with_suffix(".json.matrix-backup")
    if not backup.is_file():
        shutil.copy2(cfg_path, backup)

    cfg = load_json5(cfg_path)
    changed = patch_reasoning(cfg)
    cfg_path.write_text(json.dumps(cfg, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"Patched {cfg_path}: reasoning=false on {changed} saferoute model(s)")
    restart_gateway()
    print(backup)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
