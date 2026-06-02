#!/usr/bin/env python3
"""Black-box tests simulating real SecureModelRoute usage scenarios."""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

# Allow `from test_common import ...` when run as script
sys.path.insert(0, str(Path(__file__).resolve().parent))

from test_common import (  # noqa: E402
    KEYS_FILE,
    get_config,
    http,
    latest_audit,
    dlp_after_chat,
    parse_keys,
    put_config,
    start_smr,
    stop_smr,
    wait_ready,
    warm_file_index,
)

ROOT = Path(__file__).resolve().parents[1]
ATTACH = __import__("os").environ.get("SMR_ATTACH", "").lower() in ("1", "true", "yes")
PORT = int(__import__("os").environ.get("SMR_BLACKBOX_PORT", "18090"))
BASE = __import__("os").environ.get(
    "SMR_BASE", "http://127.0.0.1:8080" if ATTACH else f"http://127.0.0.1:{PORT}"
)
FILE_SECRET = "UNIQUE-BLACKBOX-FILE-SECRET-XYZ-998877"
OTHER_FILE_SECRET = "ORPHAN-WIDGET-QUANTUM-ZULU-776655"
PARENT_ONLY_SECRET = "PARENT-TOP-REPORT-SECRET-ALPHA"
CHILD_ONLY_SECRET = "CHILD-SUB-REPORT-SECRET-BETA"
CONTENT_SECRET = "LIVE-TEST-SECRET-KEY"
PRESET_SK = "sk-abcdefghijklmnopqrstuvwxyz1234567890AB"
PRESET_AKIA = "AKIA1234567890ABCDEF"
PRESET_GHP = "ghp_abcdefghijklmnopqrstuvwxyz1234"


@dataclass
class Scenario:
    story: str
    name: str
    ok: bool
    detail: str
    elapsed_ms: float = 0.0


@dataclass
class Report:
    scenarios: list[Scenario] = field(default_factory=list)

    def add(self, story: str, name: str, ok: bool, detail: str, elapsed_ms: float = 0.0) -> None:
        self.scenarios.append(Scenario(story, name, ok, detail, elapsed_ms))

    @property
    def passed(self) -> int:
        return sum(1 for s in self.scenarios if s.ok)

    @property
    def failed(self) -> int:
        return sum(1 for s in self.scenarios if not s.ok)


def build_config_dict(
    glm_key: str,
    ds_key: str,
    listen: str,
    secrets_dir: Path,
    mock_ports: dict[str, int],
) -> dict:
    secrets = str(secrets_dir).replace("\\", "/")
    p = mock_ports
    endpoint = lambda port: f"http://127.0.0.1:{port}"
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
                    "id": "glm-primary",
                    "base_url": "https://open.bigmodel.cn/api/coding/paas/v4",
                    "model": "glm-4-flash",
                    "api_key": glm_key,
                    "protocol": "openai",
                    "timeout_secs": 90,
                },
                {
                    "id": "deepseek-fallback",
                    "base_url": "https://api.deepseek.com",
                    "model": "deepseek-chat",
                    "api_key": ds_key,
                    "protocol": "openai",
                    "timeout_secs": 90,
                },
            ],
            "fallback-test": [
                {
                    "id": "dead-endpoint",
                    "base_url": "http://127.0.0.1:9",
                    "model": "fake-model",
                    "api_key": "dead",
                    "timeout_secs": 3,
                },
                {
                    "id": "deepseek-rescue",
                    "base_url": "https://api.deepseek.com",
                    "model": "deepseek-chat",
                    "api_key": ds_key,
                    "protocol": "openai",
                    "timeout_secs": 90,
                },
            ],
            "mock-ops": [
                {
                    "id": "mock-dangerous",
                    "base_url": endpoint(p["ops_json"]),
                    "model": "mock-model",
                    "api_key": "mock",
                    "protocol": "openai",
                    "timeout_secs": 10,
                }
            ],
            "mock-sse-ops": [
                {
                    "id": "mock-sse-dangerous",
                    "base_url": endpoint(p["ops_sse"]),
                    "model": "mock-model",
                    "api_key": "mock",
                    "protocol": "openai",
                    "timeout_secs": 10,
                }
            ],
            "stream-fallback-test": [
                {
                    "id": "mock-empty-sse",
                    "base_url": endpoint(p["empty_sse"]),
                    "model": "mock-model",
                    "api_key": "mock",
                    "protocol": "openai",
                    "timeout_secs": 5,
                },
                {
                    "id": "deepseek-rescue",
                    "base_url": "https://api.deepseek.com",
                    "model": "deepseek-chat",
                    "api_key": ds_key,
                    "protocol": "openai",
                    "timeout_secs": 90,
                },
            ],
            "mock-anthropic": [
                {
                    "id": "mock-anthropic-json",
                    "base_url": endpoint(p["anthropic_json"]),
                    "model": "mock-model",
                    "api_key": "mock",
                    "protocol": "anthropic",
                    "timeout_secs": 10,
                }
            ],
            "glm-anthropic": [
                {
                    "id": "glm-anthropic",
                    "base_url": "https://open.bigmodel.cn/api/anthropic",
                    "model": "glm-4-flash",
                    "api_key": glm_key,
                    "protocol": "anthropic",
                    "timeout_secs": 90,
                }
            ],
        },
        "content_rules": [
            {
                "id": "live-test-secret",
                "enabled": True,
                "match_mode": "full",
                "category": "secret",
                "value": CONTENT_SECRET,
            }
        ],
        "operation_rules": [
            {
                "id": "block-rm-rf",
                "enabled": True,
                "operation": "command_exec",
                "object": {"pattern": "rm -rf", "is_regex": False},
            }
        ],
        "path_protection_rules": [
            {
                "id": "blackbox-protected-dir",
                "enabled": True,
                "path": secrets,
                "level": "deny_access",
            }
        ],
        "file_rules": [
            {
                "id": "blackbox-secrets",
                "enabled": True,
                "path": secrets,
                "recursive": True,
                "trigger_window": 2,
                "match_mode": "full",
                "formats": ["txt"],
            },
            {
                "id": "blackbox-parent",
                "enabled": True,
                "path": f"{secrets}/parent",
                "recursive": True,
                "trigger_window": 2,
                "match_mode": "full",
                "formats": ["txt"],
            },
            {
                "id": "blackbox-child",
                "enabled": True,
                "path": f"{secrets}/parent/child",
                "recursive": True,
                "trigger_window": 2,
                "match_mode": "full",
                "formats": ["txt"],
            },
        ],
    }


def build_config(
    glm_key: str,
    ds_key: str,
    listen: str,
    secrets_dir: Path,
    mock_ports: dict[str, int],
) -> str:
    secrets = str(secrets_dir).replace("\\", "/")
    p = mock_ports
    return f"""server:
  listen: "{listen}"
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
      api_key: "{glm_key}"
      protocol: openai
      timeout_secs: 90
    - id: deepseek-fallback
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds_key}"
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
      api_key: "{ds_key}"
      protocol: openai
      timeout_secs: 90
  mock-ops:
    - id: mock-dangerous
      base_url: "http://127.0.0.1:{p['ops_json']}"
      model: "mock-model"
      api_key: "mock"
      protocol: openai
      timeout_secs: 10
  mock-sse-ops:
    - id: mock-sse-dangerous
      base_url: "http://127.0.0.1:{p['ops_sse']}"
      model: "mock-model"
      api_key: "mock"
      protocol: openai
      timeout_secs: 10
  stream-fallback-test:
    - id: mock-empty-sse
      base_url: "http://127.0.0.1:{p['empty_sse']}"
      model: "mock-model"
      api_key: "mock"
      protocol: openai
      timeout_secs: 5
    - id: deepseek-rescue
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds_key}"
      protocol: openai
      timeout_secs: 90
  mock-anthropic:
    - id: mock-anthropic-json
      base_url: "http://127.0.0.1:{p['anthropic_json']}"
      model: "mock-model"
      api_key: "mock"
      protocol: anthropic
      timeout_secs: 10
  glm-anthropic:
    - id: glm-anthropic
      base_url: "https://open.bigmodel.cn/api/anthropic"
      model: "glm-4-flash"
      api_key: "{glm_key}"
      protocol: anthropic
      timeout_secs: 90

content_rules:
  - id: live-test-secret
    enabled: true
    match_mode: full
    category: secret
    value: "{CONTENT_SECRET}"

operation_rules:
  - id: block-rm-rf
    enabled: true
    operation: command_exec
    object:
      pattern: "rm -rf"
      is_regex: false

path_protection_rules:
  - id: blackbox-protected-dir
    enabled: true
    path: "{secrets}"
    level: deny_access

file_rules:
  - id: blackbox-secrets
    enabled: true
    path: "{secrets}"
    recursive: true
    trigger_window: 2
    match_mode: full
    formats: ["txt"]
  - id: blackbox-parent
    enabled: true
    path: "{secrets}/parent"
    recursive: true
    trigger_window: 2
    match_mode: full
    formats: ["txt"]
  - id: blackbox-child
    enabled: true
    path: "{secrets}/parent/child"
    recursive: true
    trigger_window: 2
    match_mode: full
    formats: ["txt"]
"""


class MockDangerousJsonHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        body = {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "run_terminal_cmd",
                                    "arguments": json.dumps(
                                        {"command": "rm -rf /important/data"}
                                    ),
                                },
                            }
                        ],
                    }
                }
            ]
        }
        self._json(body)

    def _json(self, body: dict) -> None:
        payload = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt: str, *args) -> None:
        return


class MockDangerousSseHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        chunk = {
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "function": {
                                    "arguments": json.dumps({"command": "rm -rf /data"}),
                                },
                            }
                        ]
                    }
                }
            ]
        }
        lines = (
            f"data: {json.dumps(chunk)}\n\n".encode()
            + b"data: [DONE]\n\n"
        )
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(lines)))
        self.end_headers()
        self.wfile.write(lines)

    def log_message(self, fmt: str, *args) -> None:
        return


class MockEmptySseHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        lines = b"data: [DONE]\n\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(lines)))
        self.end_headers()
        self.wfile.write(lines)

    def log_message(self, fmt: str, *args) -> None:
        return


class MockAnthropicJsonHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        body = {
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "model": "mock-model",
            "content": [{"type": "text", "text": "cross-protocol-response-ok"}],
            "stop_reason": "end_turn",
        }
        payload = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt: str, *args) -> None:
        return


def start_mock(port: int, handler: type) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", port), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def apply_test_config(glm: str, ds: str, secrets_dir: Path, mock_ports: dict[str, int]) -> bool:
    listen = BASE.split("://", 1)[-1]
    cfg = build_config_dict(glm, ds, listen, secrets_dir, mock_ports)
    if put_config(BASE, cfg) != 200:
        return False
    time.sleep(1.5)
    return wait_ready(BASE)


def run_all_scenarios(report: Report, secrets_dir: Path) -> None:
    scenario_openai_sdk_client(report)
    scenario_openai_python_sdk(report)
    scenario_cursor_streaming(report)
    scenario_multi_turn_agent(report)
    scenario_dlp_user_message(report)
    scenario_preset_credentials(report)
    warm_file_index(BASE)
    scenario_file_session_guard(report, secrets_dir)
    scenario_file_scoped_sibling_not_scrubbed(report, secrets_dir)
    scenario_directory_only_no_file_dlp(report, secrets_dir)
    scenario_most_specific_child_file_scoped(report, secrets_dir)
    scenario_session_window_exhaustion(report, secrets_dir)
    scenario_request_ops_block(report)
    scenario_path_protection(report, secrets_dir)
    scenario_response_ops_block(report)
    scenario_streaming_ops_block(report)
    scenario_stream_fallback_no_token(report)
    scenario_cross_protocol_response(report)
    scenario_silent_fallback(report)
    scenario_anthropic_client(report)
    scenario_ops_observe_mode(report)
    scenario_security_disabled(report)
    scenario_config_reload_preserves_session(report, secrets_dir)
    scenario_admin_dashboard(report)
    scenario_concurrent_users(report)


def chat_openai(
    messages: list[dict],
    *,
    model: str = "glm-4-flash",
    stream: bool = False,
    max_tokens: int = 64,
    group: str | None = None,
    session: str | None = None,
) -> tuple[int, str, float]:
    body = {"model": model, "messages": messages, "max_tokens": max_tokens}
    if stream:
        body["stream"] = True
    headers: dict[str, str] = {}
    if group:
        headers["X-SMR-Fallback-Group"] = group
    if session:
        headers["X-SMR-Session-Id"] = session
    return http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=body,
        headers=headers or None,
        stream=stream,
        timeout=120.0 if stream else 90.0,
    )


# --- Scenarios (unchanged core + new P0/P1) ---


def scenario_openai_sdk_client(report: Report) -> None:
    story = "开发者：OpenAI 兼容客户端"
    body = {
        "model": "glm-4-flash",
        "messages": [{"role": "user", "content": "Reply exactly: sdk-ok"}],
        "max_tokens": 16,
    }
    code, text, ms = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=body,
        headers={"Authorization": "Bearer dummy-local-key"},
    )
    ok = code == 200 and "choices" in text
    content = json.loads(text)["choices"][0]["message"]["content"] if ok else ""
    report.add(story, "openai_compatible_client", ok, f"status={code}, reply={content[:30]!r}", ms)


def scenario_openai_python_sdk(report: Report) -> None:
    story = "开发者：OpenAI Python SDK"
    try:
        from openai import OpenAI
    except ImportError:
        report.add(story, "openai_python_sdk", True, "skipped (openai not installed)")
        return
    start = time.perf_counter()
    try:
        client = OpenAI(base_url=f"{BASE}/v1", api_key="dummy")
        resp = client.chat.completions.create(
            model="glm-4-flash",
            messages=[{"role": "user", "content": "Reply: python-sdk-ok"}],
            max_tokens=16,
        )
        content = resp.choices[0].message.content or ""
        ok = len(content) > 0
        report.add(story, "openai_python_sdk", ok, f"reply={content[:40]!r}", (time.perf_counter() - start) * 1000)
    except Exception as e:
        report.add(story, "openai_python_sdk", False, str(e), (time.perf_counter() - start) * 1000)


def scenario_cursor_streaming(report: Report) -> None:
    story = "IDE 代理：流式对话"
    code, raw, ms = chat_openai(
        [{"role": "user", "content": "Count 1, 2, 3 briefly."}],
        stream=True,
        max_tokens=32,
    )
    tokens: list[str] = []
    for line in raw.splitlines():
        if line.startswith("data: "):
            payload = line[6:].strip()
            if payload != "[DONE]":
                try:
                    delta = json.loads(payload)["choices"][0].get("delta", {})
                    if delta.get("content"):
                        tokens.append(delta["content"])
                except (json.JSONDecodeError, KeyError, IndexError):
                    pass
    ok = code == 200 and len(tokens) >= 1
    report.add(story, "streaming_sse", ok, f"chunks={len(tokens)}", ms)


def scenario_multi_turn_agent(report: Report) -> None:
    story = "AI Agent：多轮会话"
    session = "blackbox-agent-session"
    messages = [{"role": "user", "content": "My code name is ALPHA-7."}]
    code1, _, ms1 = chat_openai(messages, session=session, max_tokens=24)
    messages += [
        {"role": "assistant", "content": "OK."},
        {"role": "user", "content": "What code name did I give? One word."},
    ]
    code2, t2, ms2 = chat_openai(messages, session=session, max_tokens=24)
    reply = json.loads(t2)["choices"][0]["message"]["content"] if code2 == 200 else ""
    ok = code1 == 200 and code2 == 200
    report.add(story, "multi_turn_session", ok, f"reply={reply[:40]!r}", ms1 + ms2)


def scenario_dlp_user_message(report: Report) -> None:
    story = "用户：粘贴敏感内容"
    code, _, ms = chat_openai(
        [{"role": "user", "content": f"Secret: {CONTENT_SECRET}"}],
        session="blackbox-dlp-content",
        max_tokens=16,
    )
    audit = latest_audit(BASE)
    dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
    report.add(story, "content_dlp_scrub", code == 200 and dlp > 0, f"dlp={dlp}", ms)


def scenario_preset_credentials(report: Report) -> None:
    story = "用户：粘贴各类凭证"
    cases = [
        ("sk", PRESET_SK),
        ("akia", PRESET_AKIA),
        ("ghp", PRESET_GHP),
    ]
    for label, secret in cases:
        code, _, ms = chat_openai(
            [{"role": "user", "content": f"credential {secret} here"}],
            session=f"preset-{label}",
            max_tokens=12,
        )
        audit = latest_audit(BASE)
        dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
        report.add(story, f"preset_{label}", code == 200 and dlp > 0, f"dlp={dlp}", ms)


def scenario_file_session_guard(report: Report, secrets_dir: Path) -> None:
    story = "Agent：文件路径 DLP"
    path_str = str(secrets_dir / "project.txt").replace("\\", "/")
    session = "blackbox-file-session"
    trigger = {
        "model": "glm-4-flash",
        "messages": [
            {"role": "user", "content": "Read file"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": json.dumps({"path": path_str}),
                        },
                    }
                ],
            },
        ],
        "max_tokens": 8,
    }
    code1, _, ms1 = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=trigger,
        headers={"X-SMR-Session-Id": session},
    )
    chat_result: dict[str, object] = {}

    def do_chat() -> None:
        code, text, ms = chat_openai(
            [{"role": "user", "content": f"Copied: {FILE_SECRET}"}],
            session=session,
            max_tokens=20,
        )
        chat_result.update(code=code, text=text, ms=ms)

    dlp = dlp_after_chat(BASE, session, do_chat)
    code2 = int(chat_result.get("code", 0))
    t2 = str(chat_result.get("text", ""))
    ms2 = float(chat_result.get("ms", 0.0))
    leaked = FILE_SECRET in (json.loads(t2)["choices"][0]["message"]["content"] if code2 == 200 else "")
    ok = code2 == 200 and not leaked and dlp > 0
    report.add(
        story,
        "file_path_session_dlp",
        ok,
        f"trigger_status={code1}, dlp={dlp}, leaked={leaked}",
        ms1 + ms2,
    )


def scenario_file_scoped_sibling_not_scrubbed(report: Report, secrets_dir: Path) -> None:
    """Trigger only project.txt; leaking other.txt secret must not scrub (file-scoped)."""
    story = "Agent：文件路径 DLP"
    path_str = str(secrets_dir / "project.txt").replace("\\", "/")
    session = "blackbox-file-scoped-sibling"
    trigger = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
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
    }
    code1, _, ms1 = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=trigger,
        headers={"X-SMR-Session-Id": session},
    )
    code2, t2, ms2 = chat_openai(
        [{"role": "user", "content": f"Sibling leak: {OTHER_FILE_SECRET}"}],
        session=session,
        max_tokens=24,
    )
    leaked = OTHER_FILE_SECRET in (
        json.loads(t2)["choices"][0]["message"]["content"] if code2 == 200 else ""
    )
    audit = latest_audit(BASE)
    dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
    ok = code2 == 200 and dlp == 0
    report.add(
        story,
        "file_scoped_sibling_not_scrubbed",
        ok,
        f"dlp={dlp}, sibling_leaked={leaked}",
        ms1 + ms2,
    )


def scenario_directory_only_no_file_dlp(report: Report, secrets_dir: Path) -> None:
    """Directory path in tool must not activate file DLP (concrete file required)."""
    story = "Agent：文件路径 DLP"
    dir_str = str(secrets_dir).replace("\\", "/")
    session = "blackbox-dir-only-session"
    trigger = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "list_dir",
                            "arguments": json.dumps({"path": dir_str}),
                        },
                    }
                ],
            }
        ],
        "max_tokens": 8,
    }
    code1, _, ms1 = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=trigger,
        headers={"X-SMR-Session-Id": session},
    )
    code2, t2, ms2 = chat_openai(
        [{"role": "user", "content": f"After dir trigger: {FILE_SECRET}"}],
        session=session,
        max_tokens=24,
    )
    leaked = FILE_SECRET in (
        json.loads(t2)["choices"][0]["message"]["content"] if code2 == 200 else ""
    )
    audit = latest_audit(BASE)
    dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
    ok = code2 == 200 and dlp == 0
    report.add(
        story,
        "directory_only_no_file_dlp",
        ok,
        f"trigger_status={code1}, dlp={dlp}, file_secret_leaked={leaked}",
        ms1 + ms2,
    )


def scenario_most_specific_child_file_scoped(report: Report, secrets_dir: Path) -> None:
    """Child path rule + trigger child file only: scrub child secret, not parent sibling."""
    story = "Agent：文件路径 DLP"
    child_path = str(secrets_dir / "parent" / "child" / "report.txt").replace("\\", "/")
    session = "blackbox-child-scope-session"
    trigger = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": json.dumps({"path": child_path}),
                        },
                    }
                ],
            }
        ],
        "max_tokens": 8,
    }
    http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=trigger,
        headers={"X-SMR-Session-Id": session},
    )
    chat_openai(
        [{"role": "user", "content": f"child leak: {CHILD_ONLY_SECRET}"}],
        session=session,
        max_tokens=16,
    )
    audit_child = latest_audit(BASE)
    dlp_child = int(audit_child.get("dlp_replacements", 0)) if audit_child else 0
    chat_openai(
        [{"role": "user", "content": f"parent leak: {PARENT_ONLY_SECRET}"}],
        session=session,
        max_tokens=16,
    )
    audit_parent = latest_audit(BASE)
    dlp_parent = int(audit_parent.get("dlp_replacements", 0)) if audit_parent else 0
    ok = dlp_child > 0 and dlp_parent == 0
    report.add(
        story,
        "most_specific_child_file_scoped",
        ok,
        f"child_dlp={dlp_child}, parent_dlp={dlp_parent}",
        0,
    )


def scenario_session_window_exhaustion(report: Report, secrets_dir: Path) -> None:
    """trigger_window=2: third request after trigger should not scrub."""
    story = "Agent：SessionGuard 窗口耗尽"
    path_str = str(secrets_dir / "project.txt").replace("\\", "/")
    session = "blackbox-window-session"
    trigger = {
        "model": "glm-4-flash",
        "messages": [
            {"role": "user", "content": "Read file"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": json.dumps({"path": path_str}),
                        },
                    }
                ],
            },
        ],
        "max_tokens": 8,
    }
    http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=trigger,
        headers={"X-SMR-Session-Id": session},
    )
    dlp_counts: list[int] = []
    for i in range(3):
        dlp_counts.append(
            dlp_after_chat(
                BASE,
                session,
                lambda i=i: chat_openai(
                    [{"role": "user", "content": f"leak {FILE_SECRET} turn{i}"}],
                    session=session,
                    max_tokens=12,
                ),
            )
        )
    ok = dlp_counts[0] > 0 and dlp_counts[1] > 0 and dlp_counts[2] == 0
    report.add(story, "session_window_exhaustion", ok, f"dlp_per_turn={dlp_counts}", 0)


def scenario_request_ops_block(report: Report) -> None:
    story = "Agent：危险 tool（请求侧）"
    body = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "run_terminal_cmd",
                            "arguments": json.dumps({"command": "rm -rf /tmp/x"}),
                        },
                    }
                ],
            }
        ],
        "max_tokens": 8,
    }
    code, _, ms = http("POST", f"{BASE}/v1/chat/completions", body=body)
    audit = latest_audit(BASE)
    blocks = int(audit.get("safety_blocks", 0)) if audit else 0
    report.add(story, "request_ops_block", blocks > 0, f"status={code}, blocks={blocks}", ms)


def scenario_path_protection(report: Report, secrets_dir: Path) -> None:
    story = "Agent：路径防护"
    path_str = str(secrets_dir / "project.txt").replace("\\", "/")
    body = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
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
    }
    code, _, ms = http("POST", f"{BASE}/v1/chat/completions", body=body)
    audit = latest_audit(BASE)
    blocks = int(audit.get("safety_blocks", 0)) if audit else 0
    report.add(story, "path_protection_deny_access", blocks > 0, f"status={code}, blocks={blocks}", ms)


def scenario_response_ops_block(report: Report) -> None:
    story = "Agent：危险 tool（响应侧 JSON）"
    code, text, ms = chat_openai(
        [{"role": "user", "content": "cleanup"}],
        model="mock-model",
        group="mock-ops",
        max_tokens=16,
    )
    blocked = "SMR BLOCKED" in text
    audit = latest_audit(BASE)
    blocks = int(audit.get("safety_blocks", 0)) if audit else 0
    report.add(story, "response_ops_block", code == 200 and blocked, f"blocks={blocks}", ms)


def scenario_streaming_ops_block(report: Report) -> None:
    story = "Agent：危险 tool（响应侧 SSE）"
    code, raw, ms = chat_openai(
        [{"role": "user", "content": "stream cleanup"}],
        model="mock-model",
        group="mock-sse-ops",
        stream=True,
        max_tokens=16,
    )
    blocked = "SMR BLOCKED" in raw
    report.add(story, "streaming_ops_block", code == 200 and blocked, f"bytes={len(raw)}", ms)


def scenario_stream_fallback_no_token(report: Report) -> None:
    story = "IDE 代理：流式无 token fallback"
    code, raw, ms = chat_openai(
        [{"role": "user", "content": "hello stream"}],
        model="deepseek-chat",
        group="stream-fallback-test",
        stream=True,
        max_tokens=16,
    )
    audit = latest_audit(BASE)
    chain = audit.get("fallback_chain", []) if audit else []
    has_token = "content" in raw or "delta" in raw
    ok = code == 200 and len(chain) >= 2 and has_token
    report.add(story, "stream_no_token_fallback", ok, f"chain={chain}, has_token={has_token}", ms)


def scenario_cross_protocol_response(report: Report) -> None:
    """OpenAI client receives converted response from Anthropic-shaped upstream."""
    story = "开发者：跨协议响应转换"
    code, text, ms = chat_openai(
        [{"role": "user", "content": "hi"}],
        model="mock-model",
        group="mock-anthropic",
        max_tokens=16,
    )
    ok = code == 200 and "choices" in text
    content = ""
    if ok:
        content = json.loads(text)["choices"][0]["message"]["content"]
        ok = "cross-protocol" in content.lower() or len(content) > 0
    report.add(story, "cross_protocol_response", ok, f"content={content[:40]!r}", ms)


def scenario_silent_fallback(report: Report) -> None:
    story = "用户：无感知 fallback"
    code, _, ms = chat_openai(
        [{"role": "user", "content": "Reply ok"}],
        model="deepseek-chat",
        group="fallback-test",
        max_tokens=12,
    )
    audit = latest_audit(BASE)
    chain = audit.get("fallback_chain", []) if audit else []
    report.add(story, "transparent_fallback", code == 200 and len(chain) >= 2, f"chain={chain}", ms)


def scenario_anthropic_client(report: Report) -> None:
    story = "开发者：Anthropic 客户端"
    body = {
        "model": "glm-4-flash",
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "Say hi"}],
    }
    code, text, ms = http(
        "POST",
        f"{BASE}/v1/messages",
        body=body,
        headers={"X-SMR-Fallback-Group": "glm-anthropic"},
    )
    report.add(story, "anthropic_messages_api", code == 200 and "content" in text, f"status={code}", ms)


def scenario_ops_observe_mode(report: Report) -> None:
    story = "运维：Observe 模式"
    cfg = get_config(BASE)
    if not cfg:
        report.add(story, "observe_mode", False, "cannot GET config")
        return
    cfg["pipeline"]["operation_security_mode"] = "observe"
    put_code = put_config(BASE, cfg)
    body = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "run_terminal_cmd",
                            "arguments": json.dumps({"command": "rm -rf /tmp/observe"}),
                        },
                    }
                ],
            }
        ],
        "max_tokens": 8,
    }
    code, text, ms = http("POST", f"{BASE}/v1/chat/completions", body=body)
    audit = latest_audit(BASE)
    blocks = int(audit.get("safety_blocks", 0)) if audit else 0
    observes = int(audit.get("safety_observations", 0)) if audit else 0
    not_blocked = "SMR BLOCKED" not in text
    ok = put_code == 200 and observes > 0 and blocks == 0 and not_blocked
    report.add(
        story,
        "observe_mode",
        ok,
        f"put={put_code}, status={code}, observes={observes}, blocks={blocks}",
        ms,
    )
    # restore enforce
    cfg["pipeline"]["operation_security_mode"] = "enforce"
    put_config(BASE, cfg)


def scenario_security_disabled(report: Report) -> None:
    story = "运维：security_enabled 关闭"
    cfg = get_config(BASE)
    if not cfg:
        report.add(story, "security_disabled", False, "cannot GET config")
        return
    cfg["pipeline"]["security_enabled"] = False
    put_code = put_config(BASE, cfg)
    code, _, ms = chat_openai(
        [{"role": "user", "content": f"secret {CONTENT_SECRET}"}],
        session="security-off",
        max_tokens=12,
    )
    audit = latest_audit(BASE)
    dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
    ok = put_code == 200 and code == 200 and dlp == 0
    report.add(story, "security_disabled_bypass", ok, f"put={put_code}, dlp={dlp}", ms)
    cfg["pipeline"]["security_enabled"] = True
    put_config(BASE, cfg)


def scenario_config_reload_preserves_session(report: Report, secrets_dir: Path) -> None:
    story = "运维：热加载保留 Session"
    path_str = str(secrets_dir / "project.txt").replace("\\", "/")
    session = "blackbox-reload-session"
    trigger = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c1",
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
    }
    http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=trigger,
        headers={"X-SMR-Session-Id": session},
    )
    reload_code = 0
    for attempt in range(3):
        reload_code, _, _ = http("PUT", f"{BASE}/api/reload", timeout=120.0)
        if reload_code == 200:
            break
        time.sleep(1.0 + attempt)
    wait_ready(BASE, timeout=30.0, require_file_index=True)
    code, _, ms = chat_openai(
        [{"role": "user", "content": f"after reload {FILE_SECRET}"}],
        session=session,
        max_tokens=16,
    )
    audit = None
    dlp = 0
    for _ in range(5):
        audit = latest_audit(BASE)
        dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
        if dlp > 0:
            break
        time.sleep(0.5)
    ok = reload_code == 200 and code == 200 and dlp > 0
    report.add(story, "reload_session_persist", ok, f"reload={reload_code}, dlp={dlp}", ms)


def scenario_admin_dashboard(report: Report) -> None:
    story = "运维：Web 管理界面"
    start = time.perf_counter()
    c1, ui, _ = http("GET", f"{BASE}/ui")
    c2, status, _ = http("GET", f"{BASE}/api/status")
    c3, events, _ = http("GET", f"{BASE}/api/events?limit=10")
    c4, audits, _ = http("GET", f"{BASE}/api/audits?limit=5")
    ms = (time.perf_counter() - start) * 1000
    ok = (
        c1 == 200
        and "SafeRoute" in ui
        and c2 == 200
        and c3 == 200
        and c4 == 200
        and len(json.loads(audits).get("audits", [])) > 0
    )
    report.add(story, "admin_gui_and_apis", ok, f"ui/status/events/audits OK", ms)


def scenario_concurrent_users(report: Report) -> None:
    story = "多用户：并发对话"

    def one(uid: int) -> bool:
        code, text, _ = chat_openai(
            [{"role": "user", "content": f"hi from {uid}"}],
            session=f"user-{uid}",
            max_tokens=10,
        )
        return code == 200 and "choices" in text

    start = time.perf_counter()
    with ThreadPoolExecutor(max_workers=3) as pool:
        results = list(pool.map(one, range(1, 4)))
    ok = all(results)
    report.add(story, "concurrent_users", ok, f"users={results}", (time.perf_counter() - start) * 1000)


def print_report(report: Report) -> None:
    print("\n" + "=" * 60)
    print("  SafeRoute 黑盒测试报告")
    print("=" * 60)
    story = ""
    for s in report.scenarios:
        if s.story != story:
            story = s.story
            print(f"\n▸ {story}")
        mark = "✓ PASS" if s.ok else "✗ FAIL"
        ms = f" ({s.elapsed_ms:.0f}ms)" if s.elapsed_ms else ""
        print(f"  [{mark}] {s.name}{ms}: {s.detail}")
    print(f"\n合计: {report.passed} 通过, {report.failed} 失败 / {len(report.scenarios)} 场景")


def main() -> int:
    if not KEYS_FILE.exists():
        print(f"Missing {KEYS_FILE}", file=sys.stderr)
        return 1

    glm, ds = parse_keys()
    report = Report()
    proc = None
    cfg_file: Path | None = None
    mock_servers: list[ThreadingHTTPServer] = []
    secrets_dir = Path(tempfile.mkdtemp(prefix="smr-blackbox-secrets-"))
    (secrets_dir / "project.txt").write_text(FILE_SECRET, encoding="utf-8")
    (secrets_dir / "other.txt").write_text(OTHER_FILE_SECRET, encoding="utf-8")
    parent_dir = secrets_dir / "parent"
    child_dir = parent_dir / "child"
    parent_dir.mkdir()
    child_dir.mkdir()
    (parent_dir / "top.txt").write_text(PARENT_ONLY_SECRET, encoding="utf-8")
    (child_dir / "report.txt").write_text(CHILD_ONLY_SECRET, encoding="utf-8")

    mock_ports = {
        "ops_json": 18191,
        "ops_sse": 18192,
        "empty_sse": 18193,
        "anthropic_json": 18194,
    }

    try:
        mock_servers = [
            start_mock(mock_ports["ops_json"], MockDangerousJsonHandler),
            start_mock(mock_ports["ops_sse"], MockDangerousSseHandler),
            start_mock(mock_ports["empty_sse"], MockEmptySseHandler),
            start_mock(mock_ports["anthropic_json"], MockAnthropicJsonHandler),
        ]

        if ATTACH:
            print(f"==> Installed-app black-box @ {BASE} (attach mode)")
            if not wait_ready(BASE, timeout=60.0):
                report.add("系统", "startup", False, "installed server not ready")
                print_report(report)
                return 1
            report.add("系统", "startup", True, "installed server ready")
            if not apply_test_config(glm, ds, secrets_dir, mock_ports):
                report.add("系统", "apply_test_config", False, "PUT /api/config failed")
                print_report(report)
                return 1
            report.add("系统", "apply_test_config", True, "test config loaded")
            run_all_scenarios(report, secrets_dir)
            print_report(report)
            return 0 if report.failed == 0 else 1

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(build_config(glm, ds, f"127.0.0.1:{PORT}", secrets_dir, mock_ports))
            cfg_file = Path(f.name)

        print(f"==> Black-box tests @ {BASE}")
        proc = start_smr(cfg_file)
        time.sleep(1.0)
        if not wait_ready(BASE, timeout=120.0):
            report.add("系统", "startup", False, "timeout")
            print_report(report)
            return 1
        report.add("系统", "startup", True, "ready")

        run_all_scenarios(report, secrets_dir)

        print_report(report)
        return 0 if report.failed == 0 else 1
    finally:
        stop_smr(proc)
        for srv in mock_servers:
            srv.shutdown()
        if cfg_file and cfg_file.exists():
            cfg_file.unlink(missing_ok=True)
        shutil.rmtree(secrets_dir, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
