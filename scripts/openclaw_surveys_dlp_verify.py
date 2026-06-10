#!/usr/bin/env python3
"""Verify file DLP for OpenClaw + Surveys path via SafeRoute.

Modes:
  --replay     Simulate OpenClaw exec + tool output through /v1/chat/completions (default).
  --openclaw   Run one `openclaw agent` turn that exec-reads a PDF under Surveys.
  --analyze    Scan latest traffic snapshots for Surveys session leaks (no new requests).

Env:
  SMR_BASE=http://127.0.0.1:8080
  SMR_SURVEYS=path/to/Surveys
  SMR_SURVEYS_PDF=optional explicit pdf path
  SMR_CANARY=optional substring that must not reach upstream verbatim

Example:
  python3 scripts/openclaw_surveys_dlp_verify.py --replay
  python3 scripts/openclaw_surveys_dlp_verify.py --openclaw
  python3 scripts/openclaw_surveys_dlp_verify.py --analyze
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from test_common import (  # noqa: E402
    audits_for_session,
    http,
    wait_for_session_audit,
    wait_ready,
)

BASE = os.environ.get("SMR_BASE", "http://127.0.0.1:8080").rstrip("/")
DEFAULT_SURVEYS = str(Path.home() / "Documents/AI/Cyber-Security/Surveys")
TRAFFIC_DIR = Path.home() / "Library/Application Support/securemodelroute/traffic"
INDEX_RULE = "file-1781089475590"
DEFAULT_CANARY = (
    "Abstract: Cyber crime is proliferating everywhere exploiting every kind of "
    "vulnerability to computing environment"
)
INDIA_SIDECAR = "61f6b69c2efbc14a.txt"


def surveys_dir() -> Path:
    return Path(os.environ.get("SMR_SURVEYS", DEFAULT_SURVEYS))


def indexed_pdf_paths() -> list[Path]:
    files_json = (
        Path.home()
        / "Library/Application Support/securemodelroute/file-index"
        / INDEX_RULE
        / "gen"
    )
    if not files_json.is_dir():
        return []
    gens = sorted((p for p in files_json.iterdir() if p.is_dir()), key=lambda p: p.name)
    if not gens:
        return []
    manifest = gens[-1] / "files.json"
    if not manifest.is_file():
        return []
    data = json.loads(manifest.read_text(encoding="utf-8"))
    return [Path(entry["path"]) for entry in data.get("files", []) if entry.get("path")]


def pick_pdf() -> Path:
    override = os.environ.get("SMR_SURVEYS_PDF")
    if override:
        return Path(override)
    d = surveys_dir()
    if d.is_dir():
        pdfs = sorted(d.glob("*.pdf"))
        if pdfs:
            preferred = d / "A Survey On Machine Learning For Cyber Security, India.pdf"
            return preferred if preferred.is_file() else pdfs[0]
    indexed = indexed_pdf_paths()
    if indexed:
        preferred_name = "A Survey On Machine Learning For Cyber Security, India.pdf"
        for path in indexed:
            if path.name == preferred_name:
                return path
        return indexed[0]
    raise SystemExit(f"No PDF under {d} and no indexed paths in file-index/{INDEX_RULE}")


def india_sidecar_text(max_chars: int = 2500) -> str:
    sidecar = (
        Path.home()
        / "Library/Application Support/securemodelroute/file-index"
        / INDEX_RULE
        / "extracted"
        / INDIA_SIDECAR
    )
    if sidecar.is_file():
        return sidecar.read_text(encoding="utf-8", errors="replace")[:max_chars]
    return (
        "A Survey On Machine Learning For Cyber Security\n"
        + DEFAULT_CANARY
        + ". Ethical Hackers pay more attention towards assessing vulnerabilities."
    )


def canary_from_index() -> str:
    env = os.environ.get("SMR_CANARY", "").strip()
    if env:
        return env
    return DEFAULT_CANARY


def wait_traffic_request_out(audit_id: str, timeout: float = 15.0) -> str | None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        code, text, _ = http("GET", f"{BASE}/api/traffic?limit=40")
        if code != 200:
            time.sleep(0.3)
            continue
        for row in json.loads(text).get("records", []):
            if row.get("audit_id") != audit_id:
                continue
            if row.get("phase") != "request_out":
                continue
            body_id = row.get("id")
            if not body_id:
                continue
            c2, body, _ = http("GET", f"{BASE}/api/traffic/{body_id}")
            if c2 == 200:
                return body
        time.sleep(0.3)
    return None


def leak_report(body: str, canary: str) -> tuple[bool, str]:
    """Return (leaked, detail). Treat [[smr:...]]-only hits as partial redaction."""
    if canary not in body:
        return False, "canary absent from request_out (fully removed or not present)"
    idx = body.find(canary)
    window = body[max(0, idx - 40) : idx + len(canary) + 40]
    smr_nearby = "[[smr:" in window
    if smr_nearby:
        return True, f"partial leak near token: {window!r}"
    return True, f"verbatim leak: {window!r}"


def replay_openclaw_exec(session: str, pdf: Path, canary: str) -> int:
    surveys = surveys_dir()
    ls_cmd = f'ls -la "{surveys}"'
    pdf_cmd = (
        f'pdftotext "{pdf}" - 2>/dev/null | head -120'
    )
    tool_output = india_sidecar_text()
    payload = {
        "model": "saferoute-high",
        "messages": [
            {
                "role": "user",
                "content": f"List and read PDFs under {surveys}",
            },
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c_ls",
                        "type": "function",
                        "function": {
                            "name": "exec",
                            "arguments": json.dumps({"command": ls_cmd}),
                        },
                    }
                ],
            },
            {"role": "tool", "tool_call_id": "c_ls", "content": f"total 32\n-rw-r--r--  1 user  staff  325854 Jan  1  2020 {pdf.name}\n"},
            {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "c_pdf",
                        "type": "function",
                        "function": {
                            "name": "exec",
                            "arguments": json.dumps({"command": pdf_cmd}),
                        },
                    }
                ],
            },
            {"role": "tool", "tool_call_id": "c_pdf", "content": tool_output},
            {"role": "user", "content": "Summarize the abstract you extracted."},
        ],
        "max_tokens": 64,
    }
    headers = {"Authorization": "Bearer dummy", "X-SMR-Session-Id": session}
    code, text, ms = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=payload,
        headers=headers,
        timeout=120.0,
    )
    print(f"==> replay POST status={code} ({ms:.0f} ms)")
    if code != 200:
        print(text[:800], file=sys.stderr)
        return 1

    audit = wait_for_session_audit(BASE, session, timeout=15.0)
    if not audit:
        print("FAIL: no audit row for session", file=sys.stderr)
        return 1
    dlp = int(audit.get("dlp_replacements", 0))
    print(f"    audit dlp_replacements={dlp} id={audit.get('id')}")

    out_body = wait_traffic_request_out(audit["id"])
    if not out_body:
        print("WARN: request_out snapshot not found (enable save_traffic_bodies)", file=sys.stderr)
        return 0 if dlp > 0 else 1

    leaked, detail = leak_report(out_body, canary)
    print(f"    traffic check: {detail}")
    if dlp <= 0:
        print("FAIL: DLP did not run (dlp_replacements=0)", file=sys.stderr)
        return 1
    if leaked:
        print("FAIL: protected content still readable in request_out", file=sys.stderr)
        return 1
    print("PASS: replay — DLP ran and canary not verbatim in request_out")
    return 0


def run_openclaw_agent(pdf: Path) -> int:
    surveys = surveys_dir()
    message = (
        f"Use exec exactly once. Run this shell command and reply with ONLY the first 200 "
        f"characters of stdout (no commentary):\n"
        f'pdftotext "{pdf}" - 2>/dev/null | head -20'
    )
    session_id = f"openclaw-surveys-{int(time.time())}"
    cmd = [
        "openclaw",
        "agent",
        "--session-id",
        session_id,
        "-m",
        message,
        "--json",
        "--timeout",
        "180",
    ]
    print("==> openclaw agent (gateway must be live, provider=saferoute)")
    print("    ", " ".join(cmd[:4]), "...", sep="")
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=200,
            check=False,
        )
    except (subprocess.TimeoutExpired, FileNotFoundError) as exc:
        print(f"FAIL: openclaw agent: {exc}", file=sys.stderr)
        return 1
    if proc.returncode != 0:
        print(proc.stderr or proc.stdout, file=sys.stderr)
        return proc.returncode

    session_hint = None
    try:
        out = json.loads(proc.stdout)
        session_hint = out.get("sessionId") or out.get("session_id")
        reply = out.get("reply") or out.get("text") or str(out)[:400]
        print(f"    agent reply preview: {reply[:240]!r}")
    except json.JSONDecodeError:
        print(proc.stdout[:400])

    time.sleep(2.0)
    code, text, _ = http("GET", f"{BASE}/api/audits?limit=5")
    if code == 200:
        audits = json.loads(text).get("audits", [])
        if audits:
            a = audits[0]
            print(
                f"    latest audit dlp={a.get('dlp_replacements')} "
                f"session={a.get('session_id')}"
            )
            session_hint = session_hint or a.get("session_id")

    if session_hint:
        audits = audits_for_session(BASE, session_hint, limit=10)
        max_dlp = max(int(a.get("dlp_replacements", 0)) for a in audits) if audits else 0
        print(f"    session max dlp_replacements={max_dlp}")

    # Scan recent traffic for Surveys path + long readable PDF extract
    leaked = analyze_traffic(limit=8, canary=canary_from_index())
    if leaked:
        return 1
    audits = audits_for_session(BASE, session_id, limit=10)
    max_dlp = max(int(a.get("dlp_replacements", 0)) for a in audits) if audits else 0
    if max_dlp <= 0:
        print("FAIL: openclaw — no DLP replacements in session audits", file=sys.stderr)
        return 1
    print(f"PASS: openclaw — session {session_id} max dlp_replacements={max_dlp}")
    return 0


def analyze_traffic(*, limit: int = 20, canary: str | None = None) -> bool:
    canary = canary or canary_from_index()
    surveys = str(surveys_dir())
    if not TRAFFIC_DIR.is_dir():
        print(f"WARN: no traffic dir {TRAFFIC_DIR}")
        return False
    files = sorted(TRAFFIC_DIR.glob("*request_out*.body"), key=lambda p: p.stat().st_mtime, reverse=True)
    files = files[:limit]
    print(f"==> analyze {len(files)} recent request_out snapshots")
    any_surveys = False
    partial_leak = False
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        if surveys not in text and "Surveys" not in text:
            continue
        any_surveys = True
        smr_count = len(re.findall(r"\[\[smr:\d+\]\]", text))
        abs_count = text.count("Abstract: Cyber crime")
        print(f"    {path.name}: smr_tokens={smr_count} abstract_prefix_hits={abs_count}")
        if canary in text:
            _, detail = leak_report(text, canary)
            print(f"      abstract prefix still readable: {detail}")
            partial_leak = True
        elif "[[smr:" in text and abs_count > 0:
            print("      partial redaction: tokens present but abstract prefix still in history")
            partial_leak = True
    if not any_surveys:
        print("    (no Surveys-related request_out in window)")
        return False
    if partial_leak:
        print(
            "FAIL: analyze — fragment DLP leaves readable abstract prefix in upstream-bound "
            "request (expected with match_mode=fragment + signature gaps)"
        )
    else:
        print("PASS: analyze — abstract prefix not found in recent Surveys request_out")
    return partial_leak


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--replay", action="store_true", help="Simulate OpenClaw exec via API")
    parser.add_argument("--openclaw", action="store_true", help="Run openclaw agent one turn")
    parser.add_argument("--analyze", action="store_true", help="Analyze existing traffic only")
    args = parser.parse_args()
    if not (args.replay or args.openclaw or args.analyze):
        args.replay = True
        args.analyze = True

    code, health, _ = http("GET", f"{BASE}/health")
    if code != 200 or "OK" not in health:
        print(f"FAIL: SafeRoute not healthy at {BASE}", file=sys.stderr)
        return 1
    print(f"OK: {health.strip()}")
    if not wait_ready(BASE, timeout=120.0):
        print("FAIL: file index not ready", file=sys.stderr)
        return 1
    print("OK: file index ready")

    pdf = pick_pdf()
    canary = canary_from_index()
    print(f"    surveys={surveys_dir()}")
    print(f"    pdf={pdf.name}")
    print(f"    canary={canary[:60]}...")

    rc = 0
    if args.analyze:
        rc = max(rc, 1 if analyze_traffic(canary=canary) else 0)
    if args.replay:
        session = f"openclaw-surveys-verify-{int(time.time())}"
        rc = max(rc, replay_openclaw_exec(session, pdf, canary))
    if args.openclaw:
        rc = max(rc, run_openclaw_agent(pdf))
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
