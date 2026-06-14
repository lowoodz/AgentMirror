#!/usr/bin/env python3
"""Black-box and post-install tests for AgentMirror V2 (profile, patterns, risk, print)."""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_common import SMR_BIN, http, start_smr, stop_smr, wait_ready  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
CONTENT_SECRET = "INSIGHT-MIRROR-TEST-SECRET-XYZZY"

TOOL_AGENT_REQUEST = {
    "model": "mock-model",
    "messages": [
        {"role": "system", "content": "You are a coding agent."},
        {"role": "user", "content": "Fix the login bug in auth.rs"},
    ],
    "tools": [
        {
            "type": "function",
            "function": {"name": "Read", "description": "read file"},
        }
    ],
}

TOOL_AGENT_RESPONSE = {
    "id": "chatcmpl-insight",
    "object": "chat.completion",
    "choices": [
        {
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "I'll read auth.rs first.",
                "tool_calls": [
                    {
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "Read",
                            "arguments": '{"path":"auth.rs"}',
                        },
                    }
                ],
            },
            "finish_reason": "tool_calls",
        }
    ],
}


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def make_mock_handler():
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:  # noqa: N802
            length = int(self.headers.get("Content-Length", "0"))
            self.rfile.read(length)
            payload = json.dumps(TOOL_AGENT_RESPONSE).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, fmt: str, *args) -> None:
            return

    return Handler


def build_config(port: int, mock_port: int) -> str:
    return f"""server:
  listen: "127.0.0.1:{port}"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: observe

logging:
  level: info
  redact_content: false
  save_traffic_bodies: true

insight:
  enabled: true
  require_traffic_bodies: true
  llm_critic: false
  retention_days: 7

fallback_groups:
  high:
    - id: mock-primary
      base_url: "http://127.0.0.1:{mock_port}/v1"
      model: "mock-model"
      api_key: "mock-key"
      protocol: openai
      timeout_secs: 30

content_rules:
  - id: insight-test-secret
    enabled: true
    match_mode: full
    category: secret
    value: "{CONTENT_SECRET}"
"""


def check(name: str, ok: bool, detail: str) -> bool:
    mark = "PASS" if ok else "FAIL"
    print(f"[{mark}] {name}: {detail}")
    return ok


def wait_insight_agents(base: str, timeout: float = 15.0) -> list[dict]:
    deadline = time.time() + timeout
    while time.time() < deadline:
        code, text, _ = http("GET", f"{base}/api/insight/agents")
        if code == 200:
            agents = json.loads(text).get("agents") or []
            if agents:
                return agents
        time.sleep(0.4)
    return []


def run_v2_api_checks(base: str, results: list[bool]) -> str | None:
    code, text, _ = http("GET", f"{base}/api/insight/status")
    status = json.loads(text) if code == 200 else {}
    results.append(
        check(
            "insight_status",
            code == 200 and status.get("enabled") is True,
            f"enabled={status.get('enabled')}",
        )
    )

    agents = wait_insight_agents(base)
    if not agents:
        results.append(check("insight_agents", False, "no agents after ingest"))
        return None
    agent_id = agents[0]["agent_id"]
    results.append(check("insight_agents", True, f"count={len(agents)} id={agent_id[:16]}"))

    code, text, _ = http("GET", f"{base}/api/insight/agents/{agent_id}/profile")
    profile = json.loads(text).get("profile") if code == 200 else {}
    results.append(
        check(
            "agent_profile",
            code == 200 and profile.get("agent_id") == agent_id,
            f"tools={len(profile.get('tools') or [])} caps={len(profile.get('capabilities') or [])}",
        )
    )

    code, text, _ = http("GET", f"{base}/api/insight/agents/{agent_id}/patterns")
    pat = json.loads(text) if code == 200 else {}
    results.append(
        check(
            "agent_patterns",
            code == 200 and "patterns" in pat,
            f"patterns={len(pat.get('patterns') or [])} sample_runs={pat.get('sample_runs')}",
        )
    )

    code, text, _ = http("GET", f"{base}/api/insight/runs?agent_id={agent_id}&limit=10")
    runs_payload = json.loads(text) if code == 200 else {}
    runs = runs_payload.get("runs") or []
    has_risk_shape = all("run" in r and "risk" in r for r in runs) if runs else False
    results.append(
        check(
            "runs_with_risk",
            code == 200 and runs and has_risk_shape,
            f"runs={len(runs)} risk_keys={has_risk_shape}",
        )
    )

    run_id = runs[0]["run"]["run_id"] if runs else None
    if run_id:
        code, text, _ = http("GET", f"{base}/api/insight/runs/{run_id}")
        detail = json.loads(text) if code == 200 else {}
        results.append(
            check(
                "run_detail_risk",
                code == 200 and "risk" in detail,
                f"dlp={detail.get('risk', {}).get('dlp_replacements', '?')}",
            )
        )

    today = time.strftime("%Y-%m-%d")
    code, _, _ = http("POST", f"{base}/api/insight/daily/generate", body={"date": today})
    results.append(check("daily_generate", code == 200, f"status={code}"))

    code, text, _ = http("GET", f"{base}/api/insight/daily/{today}?agent_id={agent_id}")
    daily = json.loads(text) if code == 200 else {}
    results.append(
        check(
            "daily_get",
            code == 200,
            f"reports={len(daily.get('reports') or [])}",
        )
    )

    code, html, _ = http("GET", f"{base}/api/insight/daily/{today}/print?agent_id={agent_id}")
    results.append(
        check(
            "daily_print_html",
            code == 200 and "<html" in html.lower() and "AgentMirror" in html,
            f"bytes={len(html)}",
        )
    )

    code, ui, _ = http("GET", f"{base}/ui")
    results.append(
        check(
            "ui_agentmirror_v2",
            code == 200
            and "insightAgentProfile" in ui
            and "insight.printDaily" in ui
            and "insight-risk-badge" in ui,
            "V2 UI markers present",
        )
    )

    return agent_id


def run_blackbox() -> int:
    results: list[bool] = []
    port = free_port()
    mock_port = free_port()
    base = f"http://127.0.0.1:{port}"
    tmp = tempfile.TemporaryDirectory(prefix="smr-insight-test-")
    cfg_path = Path(tmp.name) / "smr.yaml"
    cfg_path.write_text(build_config(port, mock_port), encoding="utf-8")

    mock = ThreadingHTTPServer(("127.0.0.1", mock_port), make_mock_handler())
    thread = threading.Thread(target=mock.serve_forever, daemon=True)
    thread.start()

    proc = start_smr(cfg_path)
    try:
        if not wait_ready(base, timeout=60):
            results.append(check("smr_ready", False, "timeout"))
            return 1

        code, _, ms = http(
            "POST",
            f"{base}/v1/chat/completions",
            body=TOOL_AGENT_REQUEST,
            headers={"X-SMR-Session-Id": "insight-v2-test", "X-SMR-Agent-Id": "matrix-test-agent"},
        )
        results.append(check("proxy_turn", code == 200, f"{ms:.0f}ms status={code}"))

        # DLP turn to populate audit dlp_replacements for risk cross-highlight
        dlp_body = {
            "model": "mock-model",
            "messages": [{"role": "user", "content": f"My secret is {CONTENT_SECRET}"}],
            "max_tokens": 32,
        }
        code, _, _ = http(
            "POST",
            f"{base}/v1/chat/completions",
            body=dlp_body,
            headers={"X-SMR-Session-Id": "insight-v2-dlp"},
        )
        results.append(check("dlp_turn", code == 200, f"status={code}"))

        run_v2_api_checks(base, results)
    finally:
        stop_smr(proc)
        mock.shutdown()

    passed = sum(results)
    total = len(results)
    print(f"\nSUMMARY: {passed}/{total} passed")
    return 0 if passed == total else 1


def run_installed(base: str) -> int:
    results: list[bool] = []
    code, text, _ = http("GET", f"{base}/health")
    results.append(check("health", code == 200, f"status={code}"))

    code, text, _ = http("GET", f"{base}/api/insight/status")
    st = json.loads(text) if code == 200 else {}
    if not st.get("enabled"):
        results.append(check("insight_enabled", False, "insight disabled on installed instance"))
        passed = sum(results)
        print(f"\nSUMMARY: {passed}/{len(results)} passed")
        return 1

    # Seed one turn if no agents yet
    agents_code, agents_text, _ = http("GET", f"{base}/api/insight/agents")
    agents = json.loads(agents_text).get("agents") if agents_code == 200 else []
    if not agents:
        code, _, _ = http(
            "POST",
            f"{base}/v1/chat/completions",
            body=TOOL_AGENT_REQUEST,
            headers={
                "X-SMR-Session-Id": "insight-installed-seed",
                "X-SMR-Agent-Id": "installed-insight-probe",
            },
        )
        results.append(check("seed_turn", code == 200, f"status={code}"))
        time.sleep(1.5)

    run_v2_api_checks(base, results)
    passed = sum(results)
    total = len(results)
    print(f"\nSUMMARY: {passed}/{total} passed (installed @ {base})")
    return 0 if passed == total else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="AgentMirror V2 API tests")
    parser.add_argument(
        "--installed",
        action="store_true",
        help="Test against running install (default http://127.0.0.1:8080)",
    )
    parser.add_argument("--base", default="http://127.0.0.1:8080", help="Base URL for --installed")
    args = parser.parse_args()

    if args.installed:
        return run_installed(args.base.rstrip("/"))

    if not SMR_BIN.exists():
        print(f"Missing {SMR_BIN}; run: cargo build --release", file=sys.stderr)
        return 2
    return run_blackbox()


if __name__ == "__main__":
    raise SystemExit(main())
