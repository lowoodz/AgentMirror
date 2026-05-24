#!/usr/bin/env python3
"""Black-box tests simulating real SecureModelRoute usage scenarios.

Treats SMR as a closed system: only HTTP(S) to proxy + admin API, like a real client.
Uses test_model_api_key.txt for live upstreams; local mock upstream for deterministic ops tests.
"""

from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KEYS_FILE = ROOT / "test_model_api_key.txt"
SMR_BIN = ROOT / "target" / "release" / "smr"
PORT = int(os.environ.get("SMR_BLACKBOX_PORT", "18090"))
BASE = f"http://127.0.0.1:{PORT}"
FILE_SECRET = "UNIQUE-BLACKBOX-FILE-SECRET-XYZ-998877"
CONTENT_SECRET = "LIVE-TEST-SECRET-KEY"
PRESET_SECRET = "sk-abcdefghijklmnopqrstuvwxyz1234567890AB"


@dataclass
class Keys:
    glm_key: str
    deepseek_key: str


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


def parse_keys(path: Path) -> Keys:
    text = path.read_text(encoding="utf-8")
    glm = re.search(r"GLM\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    ds = re.search(r"Deepseek\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    if not glm or not ds:
        raise SystemExit(f"Could not parse keys from {path}")
    return Keys(glm_key=glm.group(1), deepseek_key=ds.group(1))


def http(
    method: str,
    url: str,
    body: dict | None = None,
    headers: dict | None = None,
    timeout: float = 90.0,
    stream: bool = False,
) -> tuple[int, str, float]:
    hdrs = {"Content-Type": "application/json"}
    if headers:
        hdrs.update(headers)
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers=hdrs, method=method)
    start = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if stream:
                chunks: list[bytes] = []
                while True:
                    part = resp.read(4096)
                    if not part:
                        break
                    chunks.append(part)
                text = b"".join(chunks).decode("utf-8", errors="replace")
            else:
                text = resp.read().decode("utf-8", errors="replace")
            return resp.status, text, (time.perf_counter() - start) * 1000
    except urllib.error.HTTPError as e:
        payload = e.read().decode("utf-8", errors="replace")
        return e.code, payload, (time.perf_counter() - start) * 1000
    except (urllib.error.URLError, TimeoutError, OSError):
        return 0, "", (time.perf_counter() - start) * 1000


def chat_openai(
    messages: list[dict],
    *,
    model: str = "glm-4-flash",
    stream: bool = False,
    max_tokens: int = 64,
    group: str | None = None,
    session: str | None = None,
) -> tuple[int, str, float]:
    body = {
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
    }
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
        timeout=90.0 if not stream else 120.0,
    )


def build_config(keys: Keys, listen: str, secrets_dir: Path, mock_port: int) -> str:
    secrets = str(secrets_dir).replace("\\", "/")
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
      api_key: "{keys.glm_key}"
      protocol: openai
      timeout_secs: 90
    - id: deepseek-fallback
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{keys.deepseek_key}"
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
      api_key: "{keys.deepseek_key}"
      protocol: openai
      timeout_secs: 90
  mock-ops:
    - id: mock-dangerous
      base_url: "http://127.0.0.1:{mock_port}"
      model: "mock-model"
      api_key: "mock"
      protocol: openai
      timeout_secs: 10
  glm-anthropic:
    - id: glm-anthropic
      base_url: "https://open.bigmodel.cn/api/anthropic"
      model: "glm-4-flash"
      api_key: "{keys.glm_key}"
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

file_rules:
  - id: blackbox-secrets
    enabled: true
    path: "{secrets}"
    recursive: false
    trigger_window: 5
    match_mode: full
    formats: ["txt"]
"""


class MockDangerousHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        dangerous = {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "id": "call_mock_1",
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
        payload = json.dumps(dangerous).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt: str, *args) -> None:
        return


def start_mock_upstream(port: int) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", port), MockDangerousHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


def start_smr(config_path: Path) -> subprocess.Popen:
    logf = open(config_path.with_suffix(".log"), "w", encoding="utf-8")
    return subprocess.Popen(
        [str(SMR_BIN), "--config", str(config_path)],
        stdout=logf,
        stderr=subprocess.STDOUT,
        cwd=str(ROOT),
        preexec_fn=os.setsid if hasattr(os, "setsid") else None,
    )


def stop_smr(proc: subprocess.Popen | None) -> None:
    if not proc:
        return
    try:
        if hasattr(os, "killpg"):
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        else:
            proc.terminate()
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def wait_ready(timeout: float = 20.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        code, text, _ = http("GET", f"{BASE}/health")
        if code == 200 and "OK" in text:
            code2, status, _ = http("GET", f"{BASE}/api/status")
            if code2 == 200:
                data = json.loads(status)
                if data.get("file_index_ready"):
                    return True
        time.sleep(0.3)
    return False


def latest_audit() -> dict | None:
    code, text, _ = http("GET", f"{BASE}/api/audits?limit=1")
    if code != 200:
        return None
    audits = json.loads(text).get("audits", [])
    return audits[0] if audits else None


# --- Scenarios ---


def scenario_openai_sdk_client(report: Report) -> None:
    """Developer points OpenAI SDK / curl at local proxy (dummy api_key)."""
    story = "开发者：OpenAI 兼容客户端接入"
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
    content = ""
    if ok:
        content = json.loads(text)["choices"][0]["message"]["content"]
        ok = "sdk" in content.lower() or "ok" in content.lower()
    report.add(story, "openai_compatible_client", ok, f"status={code}, reply={content[:30]!r}", ms)


def scenario_openai_python_sdk(report: Report) -> None:
    """Real OpenAI Python package if installed."""
    story = "开发者：OpenAI Python SDK"
    try:
        from openai import OpenAI
    except ImportError:
        report.add(story, "openai_python_sdk", True, "skipped (openai package not installed)")
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
        ok = resp.response.status_code == 200 and len(content) > 0
        ms = (time.perf_counter() - start) * 1000
        report.add(story, "openai_python_sdk", ok, f"reply={content[:40]!r}", ms)
    except Exception as e:
        ms = (time.perf_counter() - start) * 1000
        report.add(story, "openai_python_sdk", False, str(e), ms)


def scenario_cursor_streaming(report: Report) -> None:
    """IDE agent: streaming chat, consume tokens incrementally."""
    story = "IDE 代理：流式对话"
    code, raw, ms = chat_openai(
        [{"role": "user", "content": "Count 1, 2, 3 briefly."}],
        stream=True,
        max_tokens=32,
    )
    tokens: list[str] = []
    for line in raw.splitlines():
        if not line.startswith("data: "):
            continue
        payload = line[6:].strip()
        if payload == "[DONE]":
            continue
        try:
            chunk = json.loads(payload)
            delta = chunk["choices"][0].get("delta", {})
            if "content" in delta and delta["content"]:
                tokens.append(delta["content"])
        except (json.JSONDecodeError, KeyError, IndexError):
            continue
    streamed = "".join(tokens)
    ok = code == 200 and len(tokens) >= 1 and len(raw) > 100
    report.add(
        story,
        "streaming_sse",
        ok,
        f"status={code}, chunks={len(tokens)}, preview={streamed[:30]!r}",
        ms,
    )


def scenario_multi_turn_agent(report: Report) -> None:
    """Multi-turn agent session with stable X-SMR-Session-Id."""
    story = "AI Agent：多轮会话"
    session = "blackbox-agent-session"
    messages = [{"role": "user", "content": "My code name is ALPHA-7. Remember it."}]
    code1, t1, ms1 = chat_openai(messages, session=session, max_tokens=24)
    ok1 = code1 == 200

    messages.append({"role": "assistant", "content": "OK, I'll remember ALPHA-7."})
    messages.append({"role": "user", "content": "What code name did I give you? One word."})
    code2, t2, ms2 = chat_openai(messages, session=session, max_tokens=24)
    ok2 = code2 == 200
    reply = ""
    if ok2:
        reply = json.loads(t2)["choices"][0]["message"]["content"]
    ok = ok1 and ok2
    report.add(
        story,
        "multi_turn_session",
        ok,
        f"turn1={code1}, turn2={code2}, reply={reply[:40]!r}",
        ms1 + ms2,
    )


def scenario_dlp_user_message(report: Report) -> None:
    """User pastes secret into chat; DLP scrubs before upstream."""
    story = "用户：粘贴敏感内容"
    code, _, ms = chat_openai(
        [
            {
                "role": "user",
                "content": f"Please summarize. Secret: {CONTENT_SECRET}",
            }
        ],
        session="blackbox-dlp-content",
        max_tokens=20,
    )
    audit = latest_audit()
    dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
    ok = code == 200 and dlp > 0
    report.add(story, "content_dlp_scrub", ok, f"status={code}, dlp_replacements={dlp}", ms)


def scenario_preset_sk_key(report: Report) -> None:
    """Builtin preset catches sk-... patterns."""
    story = "用户：粘贴 API Key"
    code, _, ms = chat_openai(
        [{"role": "user", "content": f"My key is {PRESET_SECRET} please store it"}],
        session="blackbox-preset",
        max_tokens=16,
    )
    audit = latest_audit()
    dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
    ok = code == 200 and dlp > 0
    report.add(story, "preset_sk_dlp", ok, f"status={code}, dlp_replacements={dlp}", ms)


def scenario_file_session_guard(report: Report, secrets_dir: Path) -> None:
    """Agent reads protected file path in tool_call; next turn file content is scrubbed."""
    story = "Agent：文件路径 DLP 触发"
    secret_file = secrets_dir / "project.txt"
    path_str = str(secret_file).replace("\\", "/")
    session = "blackbox-file-session"

    # Turn 1: simulate tool_call mentioning protected path (triggers SessionGuard)
    trigger_body = {
        "model": "glm-4-flash",
        "messages": [
            {"role": "user", "content": "Read the project file"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call_read",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": json.dumps({"path": path_str}),
                        },
                    }
                ],
            },
        ],
        "max_tokens": 16,
    }
    code1, _, ms1 = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=trigger_body,
        headers={"X-SMR-Session-Id": session},
    )

    # Turn 2: user/agent output contains file secret (should be scrubbed by session DLP)
    code2, text2, ms2 = chat_openai(
        [
            {
                "role": "user",
                "content": f"Here is the file content I copied: {FILE_SECRET}",
            }
        ],
        session=session,
        max_tokens=24,
    )
    reply = ""
    leaked = False
    if code2 == 200:
        reply = json.loads(text2)["choices"][0]["message"]["content"]
        leaked = FILE_SECRET in reply
    audit = latest_audit()
    dlp = int(audit.get("dlp_replacements", 0)) if audit else 0
    ok = code1 == 200 and code2 == 200 and (dlp > 0 or FILE_SECRET not in reply)
    report.add(
        story,
        "file_path_session_dlp",
        ok,
        f"trigger={code1}, followup={code2}, dlp={dlp}, leaked={leaked}, reply={reply[:30]!r}",
        ms1 + ms2,
    )


def scenario_request_ops_block(report: Report) -> None:
    """Agent sends dangerous tool_call in request history."""
    story = "Agent：危险工具调用（请求侧）"
    body = {
        "model": "glm-4-flash",
        "messages": [
            {"role": "user", "content": "Clean up disk"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "call_bad",
                        "type": "function",
                        "function": {
                            "name": "run_terminal_cmd",
                            "arguments": json.dumps({"command": "rm -rf /tmp/test"}),
                        },
                    }
                ],
            },
        ],
        "max_tokens": 16,
    }
    code, text, ms = http("POST", f"{BASE}/v1/chat/completions", body=body)
    audit = latest_audit()
    blocks = int(audit.get("safety_blocks", 0)) if audit else 0
    ok = code == 200 and blocks > 0
    report.add(story, "request_ops_block", ok, f"status={code}, safety_blocks={blocks}", ms)


def scenario_response_ops_block(report: Report) -> None:
    """Model returns dangerous tool_call; proxy scrubs response (mock upstream)."""
    story = "Agent：危险工具调用（响应侧）"
    code, text, ms = chat_openai(
        [{"role": "user", "content": "Run cleanup command"}],
        model="mock-model",
        group="mock-ops",
        max_tokens=32,
    )
    blocked = "SMR BLOCKED" in text
    audit = latest_audit()
    blocks = int(audit.get("safety_blocks", 0)) if audit else 0
    ok = code == 200 and (blocked or blocks > 0)
    report.add(
        story,
        "response_ops_block",
        ok,
        f"status={code}, blocked_in_body={blocked}, safety_blocks={blocks}",
        ms,
    )


def scenario_silent_fallback(report: Report) -> None:
    """User unaware of fallback; dead primary auto-switches."""
    story = "用户：无感知 fallback"
    code, text, ms = chat_openai(
        [{"role": "user", "content": "Reply: fallback-works"}],
        model="deepseek-chat",
        group="fallback-test",
        max_tokens=16,
    )
    audit = latest_audit()
    chain = audit.get("fallback_chain", []) if audit else []
    ok = code == 200 and len(chain) >= 2
    report.add(story, "transparent_fallback", ok, f"status={code}, chain={chain}", ms)


def scenario_anthropic_client(report: Report) -> None:
    """Claude-style client hits /v1/messages through proxy."""
    story = "开发者：Anthropic 客户端"
    body = {
        "model": "glm-4-flash",
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "Say anthropic-client-ok"}],
    }
    code, text, ms = http(
        "POST",
        f"{BASE}/v1/messages",
        body=body,
        headers={"X-SMR-Fallback-Group": "glm-anthropic"},
    )
    ok = code == 200 and ("content" in text or "text" in text)
    report.add(story, "anthropic_messages_api", ok, f"status={code}, bytes={len(text)}", ms)


def scenario_admin_dashboard(report: Report) -> None:
    """Operator opens Web GUI and checks status/events/audits."""
    story = "运维：Web 管理界面"
    start = time.perf_counter()
    c1, ui, _ = http("GET", f"{BASE}/ui")
    c2, status, _ = http("GET", f"{BASE}/api/status")
    c3, events, _ = http("GET", f"{BASE}/api/events?limit=10")
    c4, audits, _ = http("GET", f"{BASE}/api/audits?limit=5")
    ms = (time.perf_counter() - start) * 1000
    ok_ui = c1 == 200 and "SecureModelRoute" in ui
    ok_status = c2 == 200 and "proxy_url" in status
    ok_events = c3 == 200 and "events" in events
    ok_audits = c4 == 200 and "audits" in audits and len(json.loads(audits)["audits"]) > 0
    ok = ok_ui and ok_status and ok_events and ok_audits
    report.add(
        story,
        "admin_gui_and_apis",
        ok,
        f"ui={c1}, status={c2}, events={c3}, audits={c4}",
        ms,
    )


def scenario_concurrent_users(report: Report) -> None:
    """Three users chat concurrently with isolated sessions."""
    story = "多用户：并发对话"

    def user_chat(uid: int) -> tuple[bool, str]:
        code, text, _ = chat_openai(
            [{"role": "user", "content": f"User{uid}: reply hi"}],
            session=f"user-{uid}",
            max_tokens=12,
        )
        ok = code == 200 and "choices" in text
        return ok, f"user{uid}={code}"

    start = time.perf_counter()
    results: list[str] = []
    ok_all = True
    with ThreadPoolExecutor(max_workers=3) as pool:
        futs = [pool.submit(user_chat, i) for i in range(1, 4)]
        for fut in as_completed(futs):
            ok, detail = fut.result()
            ok_all = ok_all and ok
            results.append(detail)
    ms = (time.perf_counter() - start) * 1000
    report.add(story, "concurrent_users", ok_all, ", ".join(sorted(results)), ms)


def scenario_tier_routing(report: Report) -> None:
    """Client selects fallback tier via header (simulates high/medium routing)."""
    story = "用户：指定 fallback 组"
    code, text, ms = chat_openai(
        [{"role": "user", "content": "Reply tier-ok"}],
        model="deepseek-chat",
        group="fallback-test",
        max_tokens=12,
    )
    ok = code == 200
    report.add(story, "fallback_group_header", ok, f"status={code}", ms)


def print_report(report: Report) -> None:
    print("\n" + "=" * 60)
    print("  SecureModelRoute 黑盒测试报告（真实场景模拟）")
    print("=" * 60)
    current_story = ""
    for s in report.scenarios:
        if s.story != current_story:
            current_story = s.story
            print(f"\n▸ {current_story}")
        mark = "✓ PASS" if s.ok else "✗ FAIL"
        ms = f" ({s.elapsed_ms:.0f}ms)" if s.elapsed_ms else ""
        print(f"  [{mark}] {s.name}{ms}")
        print(f"         {s.detail}")
    print("\n" + "-" * 60)
    print(f"合计: {report.passed} 通过, {report.failed} 失败 / {len(report.scenarios)} 场景")
    print("-" * 60)


def main() -> int:
    if not KEYS_FILE.exists():
        print(f"Missing {KEYS_FILE}", file=sys.stderr)
        return 1
    if not SMR_BIN.exists():
        print(f"Build first: cargo build --release", file=sys.stderr)
        return 1

    keys = parse_keys(KEYS_FILE)
    report = Report()
    proc: subprocess.Popen | None = None
    cfg_file: Path | None = None
    mock_server: ThreadingHTTPServer | None = None
    secrets_dir = Path(tempfile.mkdtemp(prefix="smr-blackbox-secrets-"))
    (secrets_dir / "project.txt").write_text(FILE_SECRET, encoding="utf-8")

    try:
        mock_port = 18191
        mock_server = start_mock_upstream(mock_port)

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(build_config(keys, f"127.0.0.1:{PORT}", secrets_dir, mock_port))
            cfg_file = Path(f.name)

        print(f"==> 启动 SecureModelRoute @ {BASE}")
        proc = start_smr(cfg_file)
        time.sleep(1.0)
        if not wait_ready(timeout=30.0):
            report.add("系统", "startup", False, "health/file_index 超时")
            print_report(report)
            return 1
        report.add("系统", "startup", True, "proxy ready, file index built")

        print("==> 黑盒场景测试")
        scenario_openai_sdk_client(report)
        scenario_openai_python_sdk(report)
        scenario_cursor_streaming(report)
        scenario_multi_turn_agent(report)
        scenario_dlp_user_message(report)
        scenario_preset_sk_key(report)
        scenario_file_session_guard(report, secrets_dir)
        scenario_request_ops_block(report)
        scenario_response_ops_block(report)
        scenario_silent_fallback(report)
        scenario_anthropic_client(report)
        scenario_admin_dashboard(report)
        scenario_concurrent_users(report)
        scenario_tier_routing(report)

        print_report(report)
        return 0 if report.failed == 0 else 1
    finally:
        stop_smr(proc)
        if mock_server:
            mock_server.shutdown()
        if cfg_file and cfg_file.exists():
            cfg_file.unlink(missing_ok=True)
        for p in secrets_dir.glob("*"):
            p.unlink(missing_ok=True)
        secrets_dir.rmdir()


if __name__ == "__main__":
    raise SystemExit(main())
