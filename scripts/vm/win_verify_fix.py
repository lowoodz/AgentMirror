import subprocess, sys, tempfile, time
from pathlib import Path
sys.path.insert(0, r"C:\Users\Public\smr-test-suite\scripts")
from test_common import http, parse_keys, put_config, wait_ready, wait_server_idle
from blackbox_test import (
    build_config_dict, start_mock, apply_test_config,
    MockEmptySseHandler, MockDangerousJsonHandler, MockDangerousSseHandler, MockAnthropicJsonHandler,
    chat_openai, latest_audit_for_session, BASE as _,
)

BASE = "http://127.0.0.1:18091"
python = r"C:\Users\Public\python312\python.exe"
smr = r"C:\Users\Public\smr-fix-test\smr.exe"

glm, ds = parse_keys()
secrets = Path(tempfile.mkdtemp(prefix="smr-fix-verify-"))
(secrets / "project.txt").write_text("probe-secret-data", encoding="utf-8")
cfg_path = Path(r"C:\Users\Public\smr-fix-verify-min.yaml")
subprocess.run([python, r"C:\Users\Public\smr-test-suite\scripts\generate_test_config.py", str(cfg_path), str(secrets)], check=True)

ports = {"ops_json": 18191, "ops_sse": 18192, "empty_sse": 18193, "anthropic_json": 18194}
for p, h in [(ports["ops_json"], MockDangerousJsonHandler), (ports["ops_sse"], MockDangerousSseHandler), (ports["empty_sse"], MockEmptySseHandler), (ports["anthropic_json"], MockAnthropicJsonHandler)]:
    start_mock(p, h)

# Patch listen port in minimal config
text = cfg_path.read_text(encoding="utf-8").replace("127.0.0.1:8080", "127.0.0.1:18091")
cfg_path.write_text(text, encoding="utf-8")

proc = subprocess.Popen([smr, "--config", str(cfg_path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
exit_code = 1
try:
    if not wait_ready(BASE, timeout=60, require_file_index=False):
        print("FAIL server not ready", flush=True)
        raise SystemExit(1)

    # Load full blackbox config via API
    import blackbox_test as bb
    bb.BASE = BASE
    if not apply_test_config(glm, ds, secrets, ports):
        print("FAIL apply_test_config", flush=True)
        raise SystemExit(1)

    import httpx
    body = {"model": "glm-4-flash", "messages": [{"role": "user", "content": "Reply: python-sdk-ok"}], "max_tokens": 16}
    r = httpx.post(f"{BASE}/v1/chat/completions", json=body, headers={"Authorization": "Bearer dummy"}, timeout=60.0)
    print(f"httpx code={r.status_code}", flush=True)
    if r.status_code != 200:
        print(f"httpx body={r.text[:200]!r}", flush=True)
        raise SystemExit(1)

    from openai import OpenAI
    c = OpenAI(base_url=f"{BASE}/v1", api_key="dummy", max_retries=0, timeout=60.0)
    resp = c.chat.completions.create(model="glm-4-flash", messages=[{"role": "user", "content": "Reply: python-sdk-ok"}], max_tokens=16)
    print(f"sdk ok len={len(resp.choices[0].message.content or '')}", flush=True)

    wait_server_idle(BASE, timeout=30.0)
    sid = "verify-stream-fb"
    code, raw, ms = chat_openai([{"role": "user", "content": "hello stream"}], model="deepseek-chat", group="stream-fallback-test", stream=True, max_tokens=16, session=sid)
    audit = latest_audit_for_session(BASE, sid)
    chain = audit.get("fallback_chain") if audit else None
    ok_stream = code == 200 and chain and len(chain) >= 2 and (("content" in raw) or ("delta" in raw))
    print(f"stream code={code} chain={chain} ok={ok_stream}", flush=True)

    sid2 = "verify-silent-fb"
    code2, _, ms2 = chat_openai([{"role": "user", "content": "Reply ok"}], model="deepseek-chat", group="fallback-test", max_tokens=12, session=sid2)
    audit2 = latest_audit_for_session(BASE, sid2)
    chain2 = audit2.get("fallback_chain") if audit2 else None
    ok_fb = code2 == 200 and chain2 and len(chain2) >= 2
    print(f"fallback code={code2} chain={chain2} ok={ok_fb}", flush=True)

    if ok_stream and ok_fb:
        print("ALL THREE OK", flush=True)
        exit_code = 0
    else:
        print("FAIL stream or fallback", flush=True)
finally:
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
sys.exit(exit_code)
