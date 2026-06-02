#!/usr/bin/env python3
"""Write a live-test smr.yaml for installed-app black-box runs."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from test_common import parse_keys  # noqa: E402


def render_config(out: Path, secrets_dir: Path, listen: str = "127.0.0.1:8080") -> None:
    glm, ds = parse_keys()
    secrets = str(secrets_dir).replace("\\", "/")
    content_secret = "LOCAL-INSTALL-TEST-SECRET"
    cfg = f"""server:
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
      api_key: "{glm}"
      protocol: openai
      timeout_secs: 90
    - id: deepseek-fallback
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{ds}"
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
      api_key: "{ds}"
      protocol: openai
      timeout_secs: 90
  glm-anthropic:
    - id: ds-anthropic
      base_url: "https://api.deepseek.com/anthropic"
      model: "deepseek-chat"
      api_key: "{ds}"
      protocol: anthropic
      timeout_secs: 90

content_rules:
  - id: live-test-secret
    enabled: true
    match_mode: full
    category: secret
    value: "{content_secret}"

operation_rules:
  - id: block-rm-rf
    enabled: true
    operation: command_exec
    object:
      pattern: "rm -rf"
      is_regex: false

path_protection_rules:
  - id: install-protected-dir
    enabled: true
    path: "{secrets}"
    level: deny_access

file_rules:
  - id: install-secrets
    enabled: true
    path: "{secrets}"
    recursive: false
    trigger_window: 2
    match_mode: full
    formats: ["txt"]
"""
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(cfg, encoding="utf-8")
    print(out)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: generate_test_config.py <output.yaml> <secrets_dir>")
    render_config(Path(sys.argv[1]), Path(sys.argv[2]))
