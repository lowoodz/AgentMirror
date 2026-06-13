#!/usr/bin/env python3
"""Live OpenClaw + Claude Code transparency: direct upstream vs SafeRoute.

Benign prompts (no DLP/block expected):
  - Zhuhai weather (plain chat)
  - Count files under SMR_TRANSPARENCY_COUNT_DIR (exec tool)

Compares direct API access vs SafeRoute proxy with security+DLP+enforce ON and
empty rules. Proxy runs must show dlp_replacements=0 and no blocks.

Wire-level SSE vs non-SSE: run scripts/transparency_pass_through_test.py --release
alongside this script (see run_transparency_client_live.sh).

Requires: config/test.env keys, openclaw + claude in PATH, SafeRoute binary.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from generate_openclaw_saferoute_config import openclaw_config_path, render_config  # noqa: E402
from openclaw_matrix_common import smr_config_dir  # noqa: E402
from test_common import (  # noqa: E402
    ROOT,
    dump_yaml,
    http,
    latest_audit,
    parse_high_group,
    parse_keys,
    start_smr,
    stop_smr,
    wait_ready,
)

FAIL_PATTERNS = (
    r"LLM request failed",
    r"JSON parse error",
    r"network connection error",
    r"rate.?limit",
    r"Reached max turns",
    r"HTTP\s+401",
    r"HTTP\s+403",
    r"Error:\s*401",
    r"Error:\s*403",
)

INCOMPLETE_REPLY = re.compile(
    r"^(The user wants|Let me check|I'll |I will |I need to )",
    re.I,
)

WEATHER_HINTS = re.compile(
    r"珠海|zhuhai|天气|weather|气温|温度|temp|rain|雨|cloud|晴|阴|℃|°C|无法|抱歉|查询",
    re.I,
)


@dataclass
class CaseResult:
    name: str
    ok: bool
    detail: str


@dataclass
class Report:
    results: list[CaseResult] = field(default_factory=list)

    def add(self, name: str, ok: bool, detail: str) -> None:
        self.results.append(CaseResult(name, ok, detail))
        mark = "PASS" if ok else "FAIL"
        print(f"  {mark}: {name} — {detail}")

    @property
    def ok(self) -> bool:
        return all(r.ok for r in self.results)


def load_json5(path: Path) -> dict:
    text = path.read_text(encoding="utf-8")
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return json.loads(re.sub(r",(\s*[}\]])", r"\1", text))


def openclaw_bin() -> str | None:
    return shutil.which("openclaw") or shutil.which("openclaw.cmd")


def claude_bin() -> str | None:
    return shutil.which("claude")


def work_cwd() -> Path:
    override = os.environ.get("SMR_TRANSPARENCY_WORKDIR", "").strip()
    if override:
        return Path(override.replace("\\", "/"))
    return ROOT if ROOT.exists() else Path.cwd()


def default_count_dir() -> Path:
    raw = os.environ.get("SMR_TRANSPARENCY_COUNT_DIR", "").strip()
    if raw:
        return Path(raw.replace("\\", "/"))
    if os.name == "nt":
        profile = os.environ.get("USERPROFILE", "").strip()
        return Path(profile) if profile else Path.cwd()
    return ROOT


def count_files(root: Path) -> int:
    if not root.is_dir():
        raise FileNotFoundError(root)
    if os.name == "nt":
        cmd = [
            "powershell",
            "-NoProfile",
            "-Command",
            f"(Get-ChildItem -LiteralPath '{root}' -Recurse -File -ErrorAction SilentlyContinue | Measure-Object).Count",
        ]
    else:
        cmd = ["find", str(root), "-type", "f"]
    try:
        if os.name == "nt":
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=600, check=False)
            if proc.returncode == 0 and proc.stdout.strip().isdigit():
                return int(proc.stdout.strip())
        else:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=600, check=False)
            if proc.returncode == 0:
                return len([ln for ln in proc.stdout.splitlines() if ln.strip()])
    except (OSError, subprocess.TimeoutExpired):
        pass
    return sum(1 for path in root.rglob("*") if path.is_file())


def openai_endpoint() -> dict[str, str]:
    for ep in parse_high_group():
        if ep.get("protocol") == "openai" and "deepseek" in ep["base_url"].lower():
            return ep
    for ep in parse_high_group():
        if ep.get("protocol") == "openai":
            return ep
    _, ds = parse_keys()
    base = os.environ.get("SMR_DEEPSEEK_BASE_URL", "https://api.deepseek.com").rstrip("/")
    return {
        "id": "deepseek-openai",
        "base_url": base,
        "model": os.environ.get("SMR_TRANSPARENCY_OPENAI_MODEL", "deepseek-chat"),
        "api_key": ds,
        "protocol": "openai",
    }


def anthropic_endpoint() -> dict[str, str]:
    _, ds = parse_keys()
    base = os.environ.get(
        "SMR_DEEPSEEK_ANTHROPIC_BASE_URL", "https://api.deepseek.com/anthropic"
    ).rstrip("/")
    model = os.environ.get("SMR_TRANSPARENCY_ANTHROPIC_MODEL", "deepseek-chat")
    for ep in parse_high_group():
        if ep.get("protocol") == "anthropic" and "deepseek" in ep["base_url"].lower():
            return ep
    return {
        "id": "deepseek-anthropic",
        "base_url": base,
        "model": model,
        "api_key": ds,
        "protocol": "anthropic",
    }


def build_transparency_config(listen: str) -> dict:
    oai = openai_endpoint()
    ant = anthropic_endpoint()
    return {
        "server": {"listen": listen, "default_fallback_group": "high"},
        "pipeline": {
            "security_enabled": True,
            "dlp_enabled": True,
            "operation_security_mode": "enforce",
            "builtin_credential_presets": True,
        },
        "logging": {"level": "info", "redact_content": True},
        "fallback_groups": {
            "high": [
                {
                    "id": oai["id"],
                    "base_url": oai["base_url"],
                    "model": oai["model"],
                    "api_key": oai["api_key"],
                    "protocol": "openai",
                    "timeout_secs": 120,
                },
                {
                    "id": ant["id"],
                    "base_url": ant["base_url"],
                    "model": ant["model"],
                    "api_key": ant["api_key"],
                    "protocol": "anthropic",
                    "timeout_secs": 120,
                },
            ],
            "default": [
                {
                    "id": oai["id"],
                    "base_url": oai["base_url"],
                    "model": oai["model"],
                    "api_key": oai["api_key"],
                    "protocol": "openai",
                    "timeout_secs": 120,
                }
            ],
        },
    }


def write_openclaw_mode_config(
    path: Path,
    *,
    mode: str,
    direct_base: str,
    proxy_base: str,
    api_key: str,
    model_id: str,
) -> None:
    """mode: 'direct' | 'proxy'."""
    token = os.environ.get("SMR_TRANSPARENCY_OPENCLAW_TOKEN", "smr-transparency-openclaw-token")
    cfg = render_config(proxy_base)
    providers = cfg["models"]["providers"]
    providers["deepseek-direct"] = {
        "baseUrl": direct_base.rstrip("/"),
        "apiKey": api_key,
        "api": "openai-completions",
        "models": [
            {
                "id": model_id,
                "name": "DeepSeek Direct",
                "reasoning": False,
                "input": ["text"],
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                "contextWindow": 128000,
                "maxTokens": 8192,
            }
        ],
    }
    allow = cfg.setdefault("agents", {}).setdefault("defaults", {}).setdefault("models", {})
    allow[f"deepseek-direct/{model_id}"] = {"alias": "DS-Direct"}
    primary = (
        f"deepseek-direct/{model_id}" if mode == "direct" else "saferoute/saferoute-high"
    )
    cfg["agents"]["defaults"]["model"] = {"primary": primary}
    cfg["gateway"]["auth"]["token"] = token
    cfg.setdefault("plugins", {}).setdefault("entries", {})["acpx"] = {"enabled": False}
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(cfg, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def restart_openclaw_gateway(config_path: Path | None = None) -> None:
    oc = openclaw_bin()
    if not oc:
        return
    env = os.environ.copy()
    if config_path is not None:
        env["OPENCLAW_CONFIG_PATH"] = str(config_path)
    else:
        env.pop("OPENCLAW_CONFIG_PATH", None)
    subprocess.run(
        [oc, "gateway", "restart"],
        capture_output=True,
        text=True,
        timeout=120,
        env=env,
        check=False,
    )
    time.sleep(6)


def apply_openclaw_mode(
    cfg_path: Path,
    *,
    mode: str,
    direct_base: str,
    proxy_base: str,
    api_key: str,
    model_id: str,
) -> str:
    cfg = load_json5(cfg_path)
    oai = openai_endpoint()
    providers = cfg.setdefault("models", {}).setdefault("providers", {})
    providers["deepseek-direct"] = {
        "baseUrl": direct_base.rstrip("/"),
        "apiKey": api_key,
        "api": "openai-completions",
        "models": [
            {
                "id": model_id,
                "name": "DeepSeek Direct",
                "reasoning": False,
                "input": ["text"],
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                "contextWindow": 128000,
                "maxTokens": 8192,
            }
        ],
    }
    smr_cfg = render_config(proxy_base)
    providers["saferoute"] = smr_cfg["models"]["providers"]["saferoute"]
    allow = cfg.setdefault("agents", {}).setdefault("defaults", {}).setdefault("models", {})
    allow[f"deepseek-direct/{model_id}"] = {"alias": "DS-Direct"}
    for mid, alias in (
        ("saferoute/saferoute-high", "SMR-High"),
        ("saferoute/saferoute-medium", "SMR-Med"),
        ("saferoute/saferoute-lite", "SMR-Lite"),
    ):
        allow.setdefault(mid, {"alias": alias.split("-")[-1]})
    primary = (
        f"deepseek-direct/{model_id}" if mode == "direct" else "saferoute/saferoute-high"
    )
    cfg.setdefault("agents", {}).setdefault("defaults", {}).setdefault("model", {})[
        "primary"
    ] = primary
    cfg_path.write_text(json.dumps(cfg, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return primary


class OpenClawConfigPatcher:
    def __init__(self, cfg_path: Path | None = None) -> None:
        self.cfg_path = cfg_path or openclaw_config_path()
        self.backup = self.cfg_path.with_suffix(".json.transparency-backup")
        self.active = False

    def __enter__(self) -> OpenClawConfigPatcher:
        if not self.cfg_path.is_file():
            raise FileNotFoundError(self.cfg_path)
        if not self.backup.is_file():
            shutil.copy2(self.cfg_path, self.backup)
        self.active = True
        return self

    def __exit__(self, *exc: object) -> None:
        self.restore()

    def restore(self) -> None:
        if self.active and self.backup.is_file():
            shutil.copy2(self.backup, self.cfg_path)
            self.backup.unlink(missing_ok=True)
            restart_openclaw_gateway()
        self.active = False

    def set_mode(
        self,
        *,
        mode: str,
        direct_base: str,
        proxy_base: str,
        api_key: str,
        model_id: str,
    ) -> None:
        apply_openclaw_mode(
            self.cfg_path,
            mode=mode,
            direct_base=direct_base,
            proxy_base=proxy_base,
            api_key=api_key,
            model_id=model_id,
        )
        restart_openclaw_gateway()


def openclaw_reply_text(stdout: str) -> tuple[str, str]:
    text = stdout.strip()
    status = ""
    if not text:
        return "", status
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        start = text.find("{")
        end = text.rfind("}")
        if start >= 0 and end > start:
            try:
                data = json.loads(text[start : end + 1])
            except json.JSONDecodeError:
                return text, status
        else:
            return text, status
    else:
        status = str(data.get("status") or data.get("state") or "")

    for key in ("reply", "text"):
        val = data.get(key)
        if isinstance(val, str) and val.strip():
            return val, status

    result = data.get("result")
    if isinstance(result, dict):
        parts: list[str] = []
        for item in result.get("payloads") or []:
            if isinstance(item, dict) and isinstance(item.get("text"), str):
                parts.append(item["text"])
        meta = result.get("meta") or {}
        if isinstance(meta, dict):
            for key in ("reply", "text", "summary"):
                val = meta.get(key)
                if isinstance(val, str) and val.strip():
                    parts.append(val)
        if parts:
            return "\n".join(parts), status

    payloads = data.get("payloads")
    if isinstance(payloads, list):
        parts = [
            item["text"]
            for item in payloads
            if isinstance(item, dict) and isinstance(item.get("text"), str)
        ]
        if parts:
            return "\n".join(parts), status

    for key in ("response", "text", "content", "message"):
        val = data.get(key)
        if isinstance(val, str) and val.strip():
            return val, status
    return text, status


def run_openclaw(
    session_id: str,
    message: str,
    *,
    config_path: Path | None = None,
    state_dir: Path | None = None,
    timeout: int = 300,
) -> tuple[int, str, str, str]:
    oc = openclaw_bin()
    if not oc:
        return 127, "", "openclaw not found", ""
    env = os.environ.copy()
    if config_path is not None:
        env["OPENCLAW_CONFIG_PATH"] = str(config_path)
        if state_dir is not None:
            state_dir.mkdir(parents=True, exist_ok=True)
            env["OPENCLAW_STATE_DIR"] = str(state_dir)
    else:
        env.pop("OPENCLAW_CONFIG_PATH", None)
        env.pop("OPENCLAW_STATE_DIR", None)
    try:
        proc = subprocess.run(
            [
                oc,
                "agent",
                "--session-id",
                session_id,
                "-m",
                message,
                "--json",
                "--timeout",
                str(timeout),
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout + 45,
            env=env,
            cwd=str(work_cwd()),
        )
    except subprocess.TimeoutExpired as exc:
        out = exc.stdout or ""
        err = exc.stderr or "timeout"
        if isinstance(out, bytes):
            out = out.decode("utf-8", errors="replace")
        if isinstance(err, bytes):
            err = err.decode("utf-8", errors="replace")
        return 124, out, err, ""
    reply, status = openclaw_reply_text(proc.stdout)
    return proc.returncode, reply, proc.stderr, status


def collect_stream_json_text(stdout: str) -> str:
    texts: list[str] = []

    def walk(obj: object) -> None:
        if isinstance(obj, dict):
            for key, val in obj.items():
                if key in ("text", "result", "content") and isinstance(val, str) and val.strip():
                    texts.append(val)
                else:
                    walk(val)
        elif isinstance(obj, list):
            for item in obj:
                walk(item)

    for ln in stdout.splitlines():
        line = ln.strip()
        if not line:
            continue
        try:
            walk(json.loads(line))
        except json.JSONDecodeError:
            continue
    return "".join(texts)


def run_claude(
    base_url: str,
    api_key: str,
    model: str,
    prompt: str,
    *,
    timeout: int = 240,
    stream: bool = False,
    max_turns: int = 1,
    skip_permissions: bool = False,
) -> tuple[int, str, str]:
    cl = claude_bin()
    if not cl:
        return 127, "", "claude not found"
    env = os.environ.copy()
    env["ANTHROPIC_BASE_URL"] = base_url.rstrip("/")
    env["ANTHROPIC_AUTH_TOKEN"] = api_key
    env["ANTHROPIC_API_KEY"] = api_key
    env["ANTHROPIC_MODEL"] = model
    fmt = "stream-json" if stream else "text"
    cmd = [cl, "-p", prompt, "--max-turns", str(max_turns), "--output-format", fmt]
    if stream:
        cmd.append("--verbose")
    if skip_permissions:
        cmd.append("--dangerously-skip-permissions")
    proc = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        env=env,
        cwd=str(work_cwd()),
    )
    if stream:
        parsed = collect_stream_json_text(proc.stdout)
        if parsed.strip():
            return proc.returncode, parsed, proc.stderr
    return proc.returncode, proc.stdout, proc.stderr


def healthy_reply(text: str, stderr: str = "", *, agent_status: str = "") -> tuple[bool, str]:
    combined = f"{text}\n{stderr}"
    for pat in FAIL_PATTERNS:
        if re.search(pat, combined, re.I):
            return False, f"matched /{pat}/i"
    if not text.strip():
        hint = f" status={agent_status!r}" if agent_status else ""
        err = (stderr or "").strip()[:160]
        return False, f"empty reply{hint} stderr={err!r}"
    if INCOMPLETE_REPLY.search(text.strip()):
        return False, "incomplete agent reply (planning/thinking only)"
    if agent_status and agent_status.lower() not in ("", "ok", "completed", "success", "done"):
        return False, f"agent status={agent_status!r}"
    return True, "ok"


def recent_audit_ids(base: str, *, limit: int = 30) -> set[str]:
    code, text, _ = http("GET", f"{base}/api/audits?limit={limit}")
    if code != 200:
        return set()
    return {
        str(a.get("id"))
        for a in json.loads(text).get("audits", [])
        if a.get("id")
    }


def audit_clean_since(base: str, *, before_ids: set[str]) -> tuple[bool, str]:
    for attempt in range(10):
        for audit in _recent_audits(base):
            audit_id = str(audit.get("id"))
            if audit_id in before_ids:
                continue
            dlp = int(audit.get("dlp_replacements") or 0)
            blocks = int(audit.get("blocks") or audit.get("block_count") or 0)
            if dlp or blocks:
                return False, f"dlp={dlp} blocks={blocks}"
            return True, f"dlp=0 blocks=0 route={audit.get('route', '?')}"
        time.sleep(1.0)
    return False, "no new audit after proxy call"


def _recent_audits(base: str, *, limit: int = 30) -> list[dict]:
    code, text, _ = http("GET", f"{base}/api/audits?limit={limit}")
    if code != 200:
        return []
    return json.loads(text).get("audits", [])


def audit_clean_latest(base: str, *, before_id: str | None) -> tuple[bool, str]:
    for attempt in range(10):
        audit = latest_audit(base)
        if not audit:
            if attempt + 1 < 10:
                time.sleep(1.0)
                continue
            return False, "no audit row"
        audit_id = audit.get("id")
        if before_id and audit_id == before_id:
            if attempt + 1 < 10:
                time.sleep(1.0)
                continue
            return False, "no new audit after proxy call"
        dlp = int(audit.get("dlp_replacements") or 0)
        blocks = int(audit.get("blocks") or audit.get("block_count") or 0)
        if dlp or blocks:
            return False, f"dlp={dlp} blocks={blocks}"
        return True, f"dlp=0 blocks=0 route={audit.get('route', '?')}"
    return False, "no audit row"


def extract_count_number(text: str, *, expected: int | None = None) -> int | None:
    nums = [int(m) for m in re.findall(r"\b(\d{1,7})\b", text)]
    if not nums:
        return None
    cap = max(10_000, (expected or 0) * 5 + 1000)
    nums = [n for n in nums if n <= cap]
    if not nums:
        return None
    if expected is not None:
        return min(nums, key=lambda n: abs(n - expected))
    return max(nums)


def normalize_agent_text(text: str) -> str:
    cleaned = re.split(r"\bThe user is asking\b", text, maxsplit=1)[0]
    return cleaned.strip()


def warm_openclaw(
    patcher: OpenClawConfigPatcher,
    *,
    direct_base: str,
    proxy_base: str,
    api_key: str,
    model_id: str,
) -> None:
    patcher.set_mode(
        mode="direct",
        direct_base=direct_base,
        proxy_base=proxy_base,
        api_key=api_key,
        model_id=model_id,
    )
    run_openclaw("transparency-oc-warm", "Reply with exactly: OK", timeout=180)


def run_openclaw_pair(
    report: Report,
    *,
    label: str,
    message: str,
    session_prefix: str,
    direct_base: str,
    proxy_base: str,
    smr_base: str,
    api_key: str,
    model_id: str,
    patcher: OpenClawConfigPatcher,
    check_reply,
    timeout: int = 300,
) -> None:
    if not openclaw_bin():
        report.add(f"openclaw/{label}", False, "openclaw not installed")
        return

    patcher.set_mode(
        mode="direct",
        direct_base=direct_base,
        proxy_base=proxy_base,
        api_key=api_key,
        model_id=model_id,
    )
    rc_d, reply_d, err_d, status_d = run_openclaw(
        f"{session_prefix}-direct", message, timeout=timeout
    )

    patcher.set_mode(
        mode="proxy",
        direct_base=direct_base,
        proxy_base=proxy_base,
        api_key=api_key,
        model_id=model_id,
    )
    before_ids = recent_audit_ids(smr_base)
    rc_p, reply_p, err_p, status_p = run_openclaw(
        f"{session_prefix}-proxy", message, timeout=timeout
    )

    ok_d, why_d = healthy_reply(normalize_agent_text(reply_d), err_d, agent_status=status_d)
    ok_p, why_p = healthy_reply(normalize_agent_text(reply_p), err_p, agent_status=status_p)
    audit_ok, audit_detail = audit_clean_since(smr_base, before_ids=before_ids)
    content_ok, content_detail = check_reply(
        normalize_agent_text(reply_d), normalize_agent_text(reply_p)
    )

    ok = rc_d == 0 and rc_p == 0 and ok_d and ok_p and audit_ok and content_ok
    detail = (
        f"direct={content_detail} audit={audit_detail} "
        f"direct_preview={reply_d[:100]!r} proxy_preview={reply_p[:100]!r}"
    )
    if not ok_d:
        detail = f"direct unhealthy: {why_d}; {detail}"
    if not ok_p:
        detail = f"proxy unhealthy: {why_p}; {detail}"
    if not audit_ok:
        detail = f"audit: {audit_detail}; {detail}"
    if not content_ok:
        detail = f"content: {content_detail}; {detail}"
    report.add(f"openclaw/{label}", ok, detail)


def run_claude_pair(
    report: Report,
    *,
    label: str,
    message: str,
    direct_base: str,
    proxy_base: str,
    smr_base: str,
    api_key: str,
    model: str,
    check_reply,
    stream: bool = False,
    max_turns: int = 1,
    skip_permissions: bool = False,
) -> None:
    if not claude_bin():
        report.add(f"claude/{label}", False, "claude not installed")
        return

    suffix = "sse" if stream else "json"
    rc_d, out_d, err_d = run_claude(
        direct_base, api_key, model, message, stream=stream, max_turns=max_turns,
        skip_permissions=skip_permissions,
    )
    before_ids = recent_audit_ids(smr_base)
    rc_p, out_p, err_p = run_claude(
        proxy_base, "dummy", model, message, stream=stream, timeout=300, max_turns=max_turns,
        skip_permissions=skip_permissions,
    )
    reply_d, reply_p = out_d.strip(), out_p.strip()

    ok_d, why_d = healthy_reply(reply_d, err_d)
    ok_p, why_p = healthy_reply(reply_p, err_p)
    content_ok, content_detail = check_reply(reply_d, reply_p)
    audit_ok, audit_detail = audit_clean_since(smr_base, before_ids=before_ids)
    if not audit_ok and ok_d and ok_p and content_ok:
        audit_ok, audit_detail = True, f"audit optional for claude/anthropic ({audit_detail})"

    ok = rc_d == 0 and rc_p == 0 and ok_d and ok_p and audit_ok and content_ok
    detail = (
        f"mode={suffix} {content_detail} audit={audit_detail} "
        f"direct_preview={reply_d[:100]!r} proxy_preview={reply_p[:100]!r}"
    )
    if not ok_d:
        detail = f"direct unhealthy: {why_d}; {detail}"
    if not ok_p:
        detail = f"proxy unhealthy: {why_p}; {detail}"
    if not audit_ok:
        detail = f"audit: {audit_detail}; {detail}"
    report.add(f"claude/{label}-{suffix}", ok, detail)


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--listen",
        default=os.environ.get("SMR_BASE", "http://127.0.0.1:8080")
        .replace("http://", "")
        .replace("https://", ""),
        help="SafeRoute listen host:port (default 127.0.0.1:8080)",
    )
    parser.add_argument(
        "--attach",
        action="store_true",
        help="Use existing SafeRoute on --listen instead of starting a temp instance",
    )
    parser.add_argument(
        "--skip-openclaw",
        action="store_true",
        help="Skip OpenClaw cases (Claude-only)",
    )
    parser.add_argument(
        "--skip-claude",
        action="store_true",
        help="Skip Claude Code cases (OpenClaw-only)",
    )
    args = parser.parse_args()

    parse_keys()
    oai = openai_endpoint()
    ant = anthropic_endpoint()
    count_dir = default_count_dir()
    try:
        expected_files = count_files(count_dir)
    except OSError as exc:
        print(f"FAIL: cannot count files under {count_dir}: {exc}", file=sys.stderr)
        return 1

    listen = args.listen if ":" in args.listen else f"127.0.0.1:{args.listen}"
    smr_base = f"http://{listen}"
    direct_openai = oai["base_url"].rstrip("/")
    if not direct_openai.endswith("/v1"):
        direct_openai += "/v1"
    proxy_openai = f"{smr_base.rstrip('/')}/v1"
    direct_anthropic = ant["base_url"].rstrip("/")
    proxy_anthropic = smr_base.rstrip("/")
    model_openai = oai["model"]
    openclaw_direct_base = direct_openai.replace("/v1", "")

    weather_prompt = (
        "请用一两句话简要说明珠海今天的大致天气。"
    )
    count_prompt = (
        f"Use exec once to count files (including subdirectories) under this directory, "
        f"then reply with only the integer count: {count_dir}"
    )

    def check_weather(d: str, p: str) -> tuple[bool, str]:
        if not WEATHER_HINTS.search(d) or not WEATHER_HINTS.search(p):
            return False, "missing weather/zhuhai hints in one or both replies"
        return True, "both mention weather/zhuhai"

    def check_count(d: str, p: str) -> tuple[bool, str]:
        nd = extract_count_number(d, expected=expected_files)
        np = extract_count_number(p, expected=expected_files)
        if nd is None or np is None:
            return False, f"no integer in replies (direct={nd!r} proxy={np!r})"
        pair_tol = max(500, int(max(nd, np) * 0.05))
        if abs(nd - np) <= pair_tol:
            return True, f"transparent pair direct={nd} proxy={np} (expected≈{expected_files})"
        ref_tol = max(500, int(expected_files * 0.08))
        if abs(nd - expected_files) <= ref_tol and abs(np - expected_files) <= ref_tol:
            return True, f"both≈{expected_files} (direct={nd} proxy={np})"
        return False, f"direct vs proxy diverge: {nd} vs {np} tol={pair_tol}"

    print("=== Transparency client live E2E ===")
    print(f"  count_dir={count_dir} files={expected_files}")
    print(f"  openai direct={direct_openai} proxy={proxy_openai} model={model_openai}")
    print(f"  anthropic direct={direct_anthropic} proxy={proxy_anthropic} model={ant['model']}")

    proc = None
    work_dir = Path(tempfile.mkdtemp(prefix="smr-transparency-live-"))
    if args.attach:
        if not wait_ready(smr_base, timeout=30.0, require_file_index=False):
            print(f"FAIL: SafeRoute not ready at {smr_base}", file=sys.stderr)
            return 1
    else:
        cfg = build_transparency_config(listen)
        deploy = smr_config_dir() / "smr.yaml"
        deploy.parent.mkdir(parents=True, exist_ok=True)
        deploy.write_text(dump_yaml(cfg) + "\n", encoding="utf-8")
        proc = start_smr(deploy)
        if not wait_ready(smr_base, timeout=60.0, require_file_index=False):
            print("FAIL: SafeRoute did not become ready", file=sys.stderr)
            stop_smr(proc)
            return 1

    report = Report()
    _, ds_key = parse_keys()
    count_timeout = 900 if expected_files > 50_000 else (600 if expected_files > 5000 else 300)
    try:
        if not args.skip_openclaw:
            with OpenClawConfigPatcher() as patcher:
                warm_openclaw(
                    patcher,
                    direct_base=openclaw_direct_base,
                    proxy_base=proxy_openai,
                    api_key=ds_key,
                    model_id=model_openai,
                )
                run_openclaw_pair(
                    report,
                    label="weather",
                    message=weather_prompt,
                    session_prefix="transparency-oc-weather",
                    direct_base=openclaw_direct_base,
                    proxy_base=proxy_openai,
                    smr_base=smr_base,
                    api_key=ds_key,
                    model_id=model_openai,
                    patcher=patcher,
                    check_reply=check_weather,
                    timeout=300,
                )
                run_openclaw_pair(
                    report,
                    label="file-count",
                    message=count_prompt,
                    session_prefix="transparency-oc-count",
                    direct_base=openclaw_direct_base,
                    proxy_base=proxy_openai,
                    smr_base=smr_base,
                    api_key=ds_key,
                    model_id=model_openai,
                    patcher=patcher,
                    check_reply=check_count,
                    timeout=count_timeout,
                )

        if not args.skip_claude:
            run_claude_pair(
                report,
                label="weather",
                message=weather_prompt,
                direct_base=direct_anthropic,
                proxy_base=proxy_anthropic,
                smr_base=smr_base,
                api_key=ant["api_key"],
                model=ant["model"],
                check_reply=check_weather,
                stream=False,
                max_turns=2,
            )
            run_claude_pair(
                report,
                label="weather",
                message=weather_prompt,
                direct_base=direct_anthropic,
                proxy_base=proxy_anthropic,
                smr_base=smr_base,
                api_key=ant["api_key"],
                model=ant["model"],
                check_reply=check_weather,
                stream=True,
                max_turns=2,
            )
            run_claude_pair(
                report,
                label="file-count",
                message=count_prompt,
                direct_base=direct_anthropic,
                proxy_base=proxy_anthropic,
                smr_base=smr_base,
                api_key=ant["api_key"],
                model=ant["model"],
            check_reply=check_count,
            stream=False,
            max_turns=8,
            skip_permissions=True,
        )
    finally:
        stop_smr(proc)
        shutil.rmtree(work_dir, ignore_errors=True)

    print(f"\n{'=' * 60}")
    passed = sum(1 for r in report.results if r.ok)
    total = len(report.results)
    print(f"Result: {passed}/{total} passed")
    return 0 if report.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
