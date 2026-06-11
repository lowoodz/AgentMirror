#!/usr/bin/env python3
"""Compare OpenClaw exec: direct DeepSeek API vs SafeRoute proxy (diagnostic only)."""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from test_common import parse_keys  # noqa: E402

OPENCLAW_CFG = Path.home() / ".openclaw" / "openclaw.json"
BACKUP = OPENCLAW_CFG.with_suffix(".json.direct-probe-backup")
PROMPT = "Use exec once: echo openclaw-ops-ok. Reply with stdout only."
FAIL_PATTERNS = (
    r"LLM request failed",
    r"Unexpected non-whitespace character after JSON",
    r"JSON parse error",
    r"network connection error",
)


def load_cfg() -> dict:
    text = OPENCLAW_CFG.read_text(encoding="utf-8")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return json.loads(re.sub(r",(\s*[}\]])", r"\1", text))


def save_cfg(cfg: dict) -> None:
    OPENCLAW_CFG.write_text(json.dumps(cfg, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def add_direct_provider(cfg: dict, api_key: str) -> None:
    providers = cfg.setdefault("models", {}).setdefault("providers", {})
    providers["deepseek-direct"] = {
        "baseUrl": "https://api.deepseek.com",
        "apiKey": api_key,
        "api": "openai-completions",
        "models": [
            {
                "id": "deepseek-v4-flash",
                "name": "DeepSeek Direct Probe",
                "reasoning": False,
                "input": ["text"],
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                "contextWindow": 128000,
                "maxTokens": 8192,
            }
        ],
    }
    allow = cfg.setdefault("agents", {}).setdefault("defaults", {}).setdefault("models", {})
    allow["deepseek-direct/deepseek-v4-flash"] = {"alias": "DS-Direct-Probe"}
    for mid in ("saferoute/saferoute-high",):
        allow.setdefault(mid, {"alias": "SMR-High"})


def restart_gateway() -> None:
    openclaw = shutil.which("openclaw") or "openclaw"
    subprocess.run([openclaw, "gateway", "restart"], capture_output=True, timeout=90, check=False)
    time.sleep(5)


def set_primary(cfg: dict, model: str) -> None:
    cfg.setdefault("agents", {}).setdefault("defaults", {}).setdefault("model", {})[
        "primary"
    ] = model


def run_agent(session: str) -> dict:
    openclaw = shutil.which("openclaw") or "openclaw"
    proc = subprocess.run(
        [
            openclaw,
            "agent",
            "--session-id",
            session,
            "-m",
            PROMPT,
            "--json",
            "--timeout",
            "120",
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=150,
        check=False,
    )
    stdout = proc.stdout.strip()
    stderr = proc.stderr.strip()
    reply_text = ""
    status = "parse_error"
    try:
        data = json.loads(stdout)
        status = str(data.get("status", ""))
        parts: list[str] = []
        result = data.get("result") or {}
        for item in result.get("payloads") or []:
            if isinstance(item, dict) and isinstance(item.get("text"), str):
                parts.append(item["text"])
        reply_text = "\n".join(parts)
    except json.JSONDecodeError:
        reply_text = stdout[:800]

    combined = f"{reply_text}\n{stderr}"
    failures = [pat for pat in FAIL_PATTERNS if re.search(pat, combined, re.I)]
    exec_ok = "openclaw-ops-ok" in reply_text
    return {
        "exit_code": proc.returncode,
        "agent_status": status,
        "exec_ok": exec_ok,
        "failures": failures,
        "reply_preview": reply_text[:240],
        "stderr_preview": stderr[:240],
    }


def main() -> int:
    if not OPENCLAW_CFG.is_file():
        print(f"FAIL: missing {OPENCLAW_CFG}", file=sys.stderr)
        return 1

    _, ds_key = parse_keys()
    if not BACKUP.is_file():
        shutil.copy2(OPENCLAW_CFG, BACKUP)

    cfg = load_cfg()
    add_direct_provider(cfg, ds_key)

    cases = [
        ("deepseek-direct/deepseek-v4-flash", "smr-probe-ds-direct"),
        ("saferoute/saferoute-high", "smr-probe-saferoute"),
    ]
    results: list[tuple[str, dict]] = []
    for model, session in cases:
        cfg_run = load_cfg()
        add_direct_provider(cfg_run, ds_key)
        set_primary(cfg_run, model)
        save_cfg(cfg_run)
        restart_gateway()
        print(f"\n==> Testing primary={model}")
        results.append((model, run_agent(session)))

    shutil.copy2(BACKUP, OPENCLAW_CFG)
    BACKUP.unlink(missing_ok=True)
    restart_gateway()
    print("==> Restored openclaw.json from probe backup\n")

    print("==> OpenClaw direct DeepSeek vs SafeRoute probe")
    print(f"    prompt: {PROMPT!r}")

    for model, r in results:
        ok = r["exit_code"] == 0 and not r["failures"] and r["exec_ok"]
        label = "PASS" if ok else "FAIL"
        print(f"\n{label}: {model}")
        print(f"    exit={r['exit_code']} status={r['agent_status']!r} exec_ok={r['exec_ok']}")
        if r["failures"]:
            print(f"    patterns: {r['failures']}")
        print(f"    reply: {r['reply_preview']!r}")
        if r["stderr_preview"]:
            print(f"    stderr: {r['stderr_preview']!r}")
        print()

    direct = results[0][1]
    via_smr = results[1][1]
    print("==> Conclusion")
    if direct["exec_ok"] and not direct["failures"]:
        if via_smr["exec_ok"] and not via_smr["failures"]:
            print("Both direct DeepSeek and SafeRoute work — issue may be intermittent/config.")
        else:
            print("Direct DeepSeek OK, SafeRoute FAIL — problem is inside SafeRoute path.")
    elif not direct["exec_ok"] or direct["failures"]:
        print("Direct DeepSeek also FAIL — OpenClaw↔DeepSeek streaming/tooling issue, not SafeRoute-only.")
    else:
        print("Inconclusive — inspect reply previews above.")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
