#!/usr/bin/env python3
"""Generate Windows VM user smr.yaml (high group + Z:\\NLP\\CDSSM file DLP)."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from test_common import parse_keys  # noqa: E402

DEFAULT_DLP_PATH = "Z:/NLP/CDSSM"


def render_config(glm: str, ds: str, dlp_path: str) -> str:
    dlp = dlp_path.replace("\\", "/")
    return f"""server:
  listen: "127.0.0.1:8080"
  default_fallback_group: high
  ui_language: zh

pipeline:
  security_enabled: true
  dlp_enabled: true
  dlp_reversible: true
  operation_security_mode: enforce
  path_protection_mode: enforce
  builtin_credential_presets: true

logging:
  level: info
  redact_content: true
  save_traffic_bodies: true
  traffic_request_capture: before_dlp
  traffic_max_body_bytes: 20971520

fallback_groups:
  high:
    - id: glm-primary
      base_url: "https://open.bigmodel.cn/api/anthropic"
      model: "glm-4.7"
      protocol: anthropic
      api_key: "{glm}"
      timeout_secs: 120
    - id: deepseek-fallback
      base_url: "https://api.deepseek.com"
      model: "deepseek-v4-flash"
      api_key: "{ds}"
      timeout_secs: 120

content_rules: []

file_rules:
  - id: nlp-cdssm
    enabled: true
    path: "{dlp}"
    recursive: true
    trigger_window: 15
    match_mode: fragment
    min_fragment_len: 65
    min_fragment_ratio: 0.5
    formats:
      - txt
      - md
      - json
      - yaml
      - yml
      - py
      - html
      - csv
      - pdf
    index:
      chunk_size: 8192
      chunk_overlap: 64
      signature_stride: 128
      signatures_per_chunk: 16
      max_full_file_bytes: 524288
      max_haystack_bytes: 2097152
      bloom_megabytes: 64
      build_workers: 8
      scan_stride: 16
      scan_workers: 4
      scan_rg_prefilter: true
      scan_rg_literals_max: 2048
      scan_time_budget_ms: 1000
      scan_charset_skip_threshold: 0.5
      scan_charset_skip: true

operation_rules:
  - id: block-rm-rf
    enabled: true
    operation: command_exec
    object:
      pattern: "(?i)rm\\\\s+-rf"
      is_regex: true

path_protection_rules: []
"""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--dlp-path", default=DEFAULT_DLP_PATH)
    args = parser.parse_args()
    glm, ds = parse_keys()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render_config(glm, ds, args.dlp_path), encoding="utf-8")
    print(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
