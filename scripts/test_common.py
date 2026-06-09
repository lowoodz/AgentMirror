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
TEST_ENV_FILE = Path(os.environ.get("SMR_TEST_ENV", str(ROOT / "config" / "test.env")))
KEYS_FILE = Path(os.environ.get("SMR_KEYS_FILE", str(ROOT / "test_model_api_key.txt")))

# Keep Python functional tests aligned with scripts/verify.sh (workspace target/release/smr).
os.environ.setdefault("CARGO_TARGET_DIR", str(ROOT / "target"))


def load_test_env(path: Path | None = None) -> None:
    """Load KEY=VALUE pairs from config/test.env (does not override existing env)."""
    env_path = path or TEST_ENV_FILE
    if not env_path.is_file():
        return
    for raw in env_path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, val = line.partition("=")
        key = key.strip()
        val = val.strip().strip('"').strip("'")
        if key and key not in os.environ:
            os.environ[key] = val


load_test_env()


def has_test_keys() -> bool:
    if os.environ.get("SMR_GLM_API_KEY") and os.environ.get("SMR_DEEPSEEK_API_KEY"):
        return True
    return KEYS_FILE.is_file()


def _default_smr_bin() -> Path:
    override = os.environ.get("SMR_BIN")
    if override:
        return Path(override)
    if os.name == "nt":
        for candidate in (
            Path(os.environ.get("USERPROFILE", "")) / ".local" / "bin" / "smr.exe",
            Path(
                os.environ.get("SMR_GUEST_STAGING", "")
                or str(Path(os.environ.get("USERPROFILE", "")) / "smr-staging")
            )
            / "smr-home"
            / "bin"
            / "smr.exe",
            ROOT / "target" / "release" / "smr.exe",
        ):
            if candidate.exists():
                return candidate
        return ROOT / "target" / "release" / "smr.exe"
    return ROOT / "target" / "release" / "smr"


SMR_BIN = _default_smr_bin()


def parse_keys(path: Path | None = None) -> tuple[str, str]:
    glm = os.environ.get("SMR_GLM_API_KEY", "").strip()
    ds = os.environ.get("SMR_DEEPSEEK_API_KEY", "").strip()
    if glm and ds:
        return glm, ds
    keys_path = path or KEYS_FILE
    text = keys_path.read_text(encoding="utf-8")
    glm_m = re.search(r"GLM\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    ds_m = re.search(r"Deepseek\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    if not glm_m or not ds_m:
        raise SystemExit(
            f"Set SMR_GLM_API_KEY and SMR_DEEPSEEK_API_KEY in config/test.env "
            f"(copy from config/test.env.example), or provide keys in {keys_path}"
        )
    return glm_m.group(1), ds_m.group(1)


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


def wait_server_idle(base: str, timeout: float = 90.0) -> bool:
    """Wait until health + status respond reliably (avoids PUT during reload/index)."""
    deadline = time.time() + timeout
    streak = 0
    while time.time() < deadline:
        c1, t1, _ = http("GET", f"{base}/health", timeout=10.0)
        c2, _, _ = http("GET", f"{base}/api/status", timeout=10.0)
        if c1 == 200 and "OK" in t1 and c2 == 200:
            streak += 1
            if streak >= 2:
                return True
        else:
            streak = 0
        time.sleep(0.75)
    return False


def _attach_config_path() -> Path | None:
    if os.environ.get("SMR_ATTACH", "").lower() not in ("1", "true", "yes"):
        return None
    raw = os.environ.get("SMR_CONFIG", "").strip()
    return Path(raw) if raw else None


def _yaml_scalar(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return "null"
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, str):
        if (
            not value
            or value in ("true", "false", "null", "yes", "no", "~")
            or any(ch in value for ch in ":#[]{}&*!|>'\"%@`,\n")
        ):
            return json.dumps(value, ensure_ascii=False)
        return value
    raise TypeError(f"unsupported yaml scalar: {type(value)!r}")


def dump_yaml(obj: object, indent: int = 0) -> str:
    pad = "  " * indent
    if isinstance(obj, dict):
        lines: list[str] = []
        for key, value in obj.items():
            if isinstance(value, dict) and value:
                lines.append(f"{pad}{key}:")
                lines.append(dump_yaml(value, indent + 1))
            elif isinstance(value, list) and value:
                lines.append(f"{pad}{key}:")
                lines.append(dump_yaml(value, indent + 1))
            else:
                lines.append(f"{pad}{key}: {_yaml_scalar(value)}")
        return "\n".join(lines)
    if isinstance(obj, list):
        lines = []
        for item in obj:
            if isinstance(item, dict):
                lines.append(f"{pad}-")
                for key, value in item.items():
                    if isinstance(value, (dict, list)) and value:
                        lines.append(f"{pad}  {key}:")
                        lines.append(dump_yaml(value, indent + 2))
                    else:
                        lines.append(f"{pad}  {key}: {_yaml_scalar(value)}")
            else:
                lines.append(f"{pad}- {_yaml_scalar(item)}")
        return "\n".join(lines)
    return _yaml_scalar(obj)


def put_config(base: str, config: dict, *, timeout: float = 300.0) -> int:
    attach_path = _attach_config_path()
    if attach_path:
        attach_timeout = max(timeout, 300.0)
        wait_server_idle(base, timeout=min(120.0, attach_timeout / 2))
        attach_path.parent.mkdir(parents=True, exist_ok=True)
        attach_path.write_text(dump_yaml(config) + "\n", encoding="utf-8")
        code = reload_config(base, timeout=attach_timeout)
        if code == 200 and not wait_ready(base, timeout=min(attach_timeout, 240.0)):
            return 0
        return code
    code = 0
    for attempt in range(8):
        wait_server_idle(base, timeout=min(60.0, timeout / 5))
        code, _, _ = http("PUT", f"{base}/api/config", body=config, timeout=timeout)
        if code == 200:
            if wait_ready(base, timeout=min(timeout, 240.0)):
                return code
        time.sleep(2.0 * (attempt + 1))
    return code


def reload_config(base: str, *, timeout: float = 180.0) -> int:
    attach = _attach_config_path() is not None
    idle_timeout = 120.0 if attach else 60.0
    http_timeout = max(timeout, 300.0) if attach else timeout
    code = 0
    for attempt in range(8 if attach else 6):
        wait_server_idle(base, timeout=min(idle_timeout, http_timeout / 4))
        code, _, _ = http("PUT", f"{base}/api/reload", timeout=http_timeout)
        if code == 200:
            if wait_ready(base, timeout=min(http_timeout, 240.0)):
                return code
        time.sleep(2.0 * (attempt + 1))
    return code


AUDIT_QUERY_LIMIT = 200


def latest_audit(base: str) -> dict | None:
    code, text, _ = http("GET", f"{base}/api/audits?limit=1")
    if code != 200:
        return None
    audits = json.loads(text).get("audits", [])
    return audits[0] if audits else None


def audits_for_session(base: str, session_id: str, *, limit: int = AUDIT_QUERY_LIMIT) -> list[dict]:
    from urllib.parse import quote

    sid = quote(session_id, safe="")
    code, text, _ = http("GET", f"{base}/api/audits?limit={limit}&session_id={sid}")
    if code != 200:
        return []
    audits = json.loads(text).get("audits", [])
    return [audit for audit in audits if audit.get("session_id") == session_id]


def latest_audit_for_session(base: str, session_id: str) -> dict | None:
    audits = audits_for_session(base, session_id)
    return audits[0] if audits else None


def wait_for_session_audit(
    base: str,
    session_id: str,
    *,
    after_ids: set[str] | None = None,
    timeout: float = 6.0,
) -> dict | None:
    known = set(after_ids or ())
    deadline = time.time() + timeout
    while time.time() < deadline:
        for audit in audits_for_session(base, session_id):
            audit_id = audit.get("id")
            if audit_id and audit_id not in known:
                return audit
        time.sleep(0.25)
    for audit in audits_for_session(base, session_id):
        audit_id = audit.get("id")
        if audit_id and audit_id not in known:
            return audit
    return None


def dlp_after_chat(base: str, session_id: str, chat_fn) -> int:
    before_ids = {a.get("id") for a in audits_for_session(base, session_id) if a.get("id")}
    chat_fn()
    max_dlp = 0
    for _ in range(24):
        for audit in audits_for_session(base, session_id):
            audit_id = audit.get("id")
            if audit_id and audit_id not in before_ids:
                max_dlp = max(max_dlp, int(audit.get("dlp_replacements", 0)))
        if max_dlp > 0:
            return max_dlp
        time.sleep(0.25)
    for audit in audits_for_session(base, session_id):
        audit_id = audit.get("id")
        if audit_id and audit_id not in before_ids:
            max_dlp = max(max_dlp, int(audit.get("dlp_replacements", 0)))
    return max_dlp


def warm_file_index(base: str) -> bool:
    for attempt in range(5):
        code, _, _ = http("PUT", f"{base}/api/reload", timeout=180.0)
        if code == 200 and wait_ready(base, timeout=180.0):
            return True
        time.sleep(2.0 * (attempt + 1))
    return False
