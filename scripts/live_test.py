#!/usr/bin/env python3
"""Stress / load tests against live upstreams via SafeRoute.

Functional and black-box scenarios live in scripts/blackbox_test.py.
Requires API keys in config/test.env (copy from config/test.env.example) or legacy test_model_api_key.txt.
"""

from __future__ import annotations

import os
import statistics
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from test_common import KEYS_FILE, parse_keys, start_smr, stop_smr, wait_ready, wait_server_idle, http

ROOT = Path(__file__).resolve().parents[1]
PORT = int(os.environ.get("SMR_STRESS_PORT", "18081"))
BASE = f"http://127.0.0.1:{PORT}"


@dataclass
class Result:
    name: str
    ok: bool
    detail: str
    elapsed_ms: float = 0.0


@dataclass
class Report:
    results: list[Result] = field(default_factory=list)

    def add(self, name: str, ok: bool, detail: str, elapsed_ms: float = 0.0) -> None:
        self.results.append(Result(name, ok, detail, elapsed_ms))


def build_stress_config(glm_key: str, ds_key: str, listen: str) -> str:
    return f"""server:
  listen: "{listen}"
  default_fallback_group: stress

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce
  builtin_credential_presets: true

logging:
  level: info
  redact_content: true

fallback_groups:
  stress:
    - id: deepseek-fast
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds_key}"
      protocol: openai
      timeout_secs: 90
    - id: glm-backup
      base_url: "https://open.bigmodel.cn/api/coding/paas/v4"
      model: "glm-4-flash"
      api_key: "{glm_key}"
      protocol: openai
      timeout_secs: 90

content_rules: []
operation_rules: []
file_rules: []
"""


def worker_chat(i: int, *, stream: bool) -> tuple[bool, float, str]:
    body = {
        "model": "deepseek-chat",
        "messages": [{"role": "user", "content": f"ping {i}"}],
        "max_tokens": 8,
    }
    if stream:
        body["stream"] = True
    headers = {
        "X-SMR-Session-Id": f"stress-{i % 8}",
    }
    try:
        code, text, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body=body,
            headers=headers,
            stream=stream,
            timeout=120.0 if stream else 90.0,
        )
        if stream:
            ok = code == 200 and "data:" in text
        else:
            ok = code == 200 and "choices" in text
        return ok, ms, f"status={code}"
    except Exception as e:
        return False, 0.0, str(e)


def run_pool(
    report: Report,
    name: str,
    total: int,
    workers: int,
    stream: bool,
    min_success: float,
) -> None:
    latencies: list[float] = []
    ok_count = 0
    errors: list[str] = []
    start = time.perf_counter()
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futs = [pool.submit(worker_chat, i, stream=stream) for i in range(total)]
        for fut in as_completed(futs):
            ok, ms, detail = fut.result()
            if ok:
                ok_count += 1
                latencies.append(ms)
            else:
                errors.append(detail)
    wall = time.perf_counter() - start
    rate = ok_count / total if total else 0.0
    p50 = statistics.median(latencies) if latencies else 0.0
    p95 = (
        sorted(latencies)[int(len(latencies) * 0.95) - 1] if len(latencies) >= 2 else p50
    )
    ok = rate >= min_success
    report.add(
        name,
        ok,
        f"ok={ok_count}/{total} ({rate:.0%}), wall={wall:.1f}s, "
        f"p50={p50:.0f}ms, p95={p95:.0f}ms, errors={errors[:3]}",
        wall * 1000,
    )


def run_soak(report: Report, duration_sec: float, interval_sec: float) -> None:
    """Light soak: one request every interval for duration_sec."""
    start = time.perf_counter()
    ok_count = 0
    total = 0
    while time.perf_counter() - start < duration_sec:
        total += 1
        ok, _, _ = worker_chat(total, stream=False)
        if ok:
            ok_count += 1
        time.sleep(interval_sec)
    wall = time.perf_counter() - start
    rate = ok_count / total if total else 0.0
    ok = rate >= 0.85
    report.add(
        "soak_serial",
        ok,
        f"ok={ok_count}/{total} ({rate:.0%}) over {wall:.0f}s",
        wall * 1000,
    )


def print_report(report: Report) -> None:
    print("\n=== Stress Test Report ===")
    for r in report.results:
        mark = "PASS" if r.ok else "FAIL"
        ms = f" ({r.elapsed_ms:.0f}ms)" if r.elapsed_ms else ""
        print(f"[{mark}] {r.name}{ms}: {r.detail}")
    passed = sum(1 for r in report.results if r.ok)
    failed = sum(1 for r in report.results if not r.ok)
    print(f"\nTotal: {passed} passed, {failed} failed")


def main() -> int:
    if not KEYS_FILE.exists():
        print(f"Missing {KEYS_FILE}", file=sys.stderr)
        return 1

    glm, ds = parse_keys()
    report = Report()
    proc = None
    cfg_file: Path | None = None

    total = int(os.environ.get("SMR_STRESS_TOTAL", "40"))
    workers = int(os.environ.get("SMR_STRESS_WORKERS", "12"))
    stream_total = int(os.environ.get("SMR_STRESS_STREAM_TOTAL", "15"))
    stream_workers = int(os.environ.get("SMR_STRESS_STREAM_WORKERS", "5"))
    min_success = float(os.environ.get("SMR_STRESS_MIN_SUCCESS", "0.9"))
    stream_min_success = float(os.environ.get("SMR_STRESS_STREAM_MIN_SUCCESS", "0.85"))
    soak_sec = float(os.environ.get("SMR_STRESS_SOAK_SEC", "0"))  # 0 = skip

    try:
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(build_stress_config(glm, ds, f"127.0.0.1:{PORT}"))
            cfg_file = Path(f.name)

        print(f"==> Stress tests @ {BASE}")
        proc = start_smr(cfg_file)
        time.sleep(2.0)
        if not wait_ready(BASE, timeout=120.0, require_file_index=False):
            report.add("startup", False, "health timeout")
            print_report(report)
            return 1
        wait_server_idle(BASE, timeout=60.0)
        report.add("startup", True, "ready")

        print(f"==> Non-streaming concurrent ({total} req, {workers} workers)")
        run_pool(report, "concurrent_chat", total, workers, stream=False, min_success=min_success)

        print(f"==> Streaming concurrent ({stream_total} req, {stream_workers} workers)")
        run_pool(
            report,
            "concurrent_stream",
            stream_total,
            stream_workers,
            stream=True,
            min_success=stream_min_success,
        )

        if soak_sec > 0:
            print(f"==> Soak ({soak_sec:.0f}s, one request every 2s)")
            run_soak(report, soak_sec, interval_sec=2.0)

        print_report(report)
        return 0 if all(r.ok for r in report.results) else 1
    finally:
        stop_smr(proc)
        if cfg_file and cfg_file.exists():
            cfg_file.unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())
