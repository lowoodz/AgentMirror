"""Shared helpers for live_test.py and blackbox_test.py."""

from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KEYS_FILE = Path(os.environ.get("SMR_KEYS_FILE", str(ROOT / "test_model_api_key.txt")))


def _default_smr_bin() -> Path:
    override = os.environ.get("SMR_BIN")
    if override:
        return Path(override)
    if os.name == "nt":
        for candidate in (
            Path(os.environ.get("USERPROFILE", "")) / ".local" / "bin" / "smr.exe",
            Path(r"C:\Users\Public\smr-home\bin\smr.exe"),
            ROOT / "target" / "release" / "smr.exe",
        ):
            if candidate.exists():
                return candidate
        return ROOT / "target" / "release" / "smr.exe"
    return ROOT / "target" / "release" / "smr"


SMR_BIN = _default_smr_bin()


def parse_keys(path: Path = KEYS_FILE) -> tuple[str, str]:
    text = path.read_text(encoding="utf-8")
    glm = re.search(r"GLM\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    ds = re.search(r"Deepseek\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    if not glm or not ds:
        raise SystemExit(f"Could not parse keys from {path}")
    return glm.group(1), ds.group(1)


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
    if body is not None:
        data = json.dumps(body).encode()
    elif method in ("PUT", "PATCH"):
        # Windows Python urllib rejects bodyless PUT (reload API).
        data = b"{}"
    else:
        data = None
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


def start_smr(config_path: Path, cwd: Path = ROOT) -> subprocess.Popen:
    if not SMR_BIN.exists():
        raise SystemExit(f"Missing {SMR_BIN}; run: cargo build --release")
    logf = open(config_path.with_suffix(".log"), "w", encoding="utf-8")
    kwargs: dict = {
        "args": [str(SMR_BIN), "--config", str(config_path)],
        "stdout": logf,
        "stderr": subprocess.STDOUT,
        "cwd": str(cwd),
    }
    if os.name == "nt":
        kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP  # type: ignore[attr-defined]
    else:
        kwargs["preexec_fn"] = os.setsid
    return subprocess.Popen(**kwargs)


def stop_smr(proc: subprocess.Popen | None) -> None:
    if not proc:
        return
    try:
        if os.name == "nt":
            proc.terminate()
        elif hasattr(os, "killpg"):
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        else:
            proc.terminate()
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def wait_ready(base: str, timeout: float = 30.0, require_file_index: bool = True) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        code, text, _ = http("GET", f"{base}/health")
        if code == 200 and "OK" in text:
            if not require_file_index:
                return True
            c2, status, _ = http("GET", f"{base}/api/status")
            if c2 == 200 and json.loads(status).get("file_index_ready"):
                return True
        time.sleep(0.3)
    return False


def get_config(base: str) -> dict | None:
    code, text, _ = http("GET", f"{base}/api/config")
    if code != 200:
        return None
    return json.loads(text)


def put_config(base: str, config: dict, *, timeout: float = 120.0) -> int:
    code = 0
    for attempt in range(3):
        code, _, _ = http("PUT", f"{base}/api/config", body=config, timeout=timeout)
        if code == 200:
            return code
        time.sleep(1.0 + attempt)
    return code


def latest_audit(base: str) -> dict | None:
    code, text, _ = http("GET", f"{base}/api/audits?limit=1")
    if code != 200:
        return None
    audits = json.loads(text).get("audits", [])
    return audits[0] if audits else None


def latest_audit_for_session(base: str, session_id: str) -> dict | None:
    code, text, _ = http("GET", f"{base}/api/audits?limit=40")
    if code != 200:
        return None
    for audit in json.loads(text).get("audits", []):
        if audit.get("session_id") == session_id:
            return audit
    return None


def dlp_after_chat(base: str, session_id: str, chat_fn) -> int:
    before = latest_audit_for_session(base, session_id)
    before_id = before.get("id") if before else None
    chat_fn()
    for _ in range(12):
        audit = latest_audit_for_session(base, session_id)
        if audit and audit.get("id") != before_id:
            return int(audit.get("dlp_replacements", 0))
        time.sleep(0.25)
    audit = latest_audit_for_session(base, session_id)
    return int(audit.get("dlp_replacements", 0)) if audit else 0
