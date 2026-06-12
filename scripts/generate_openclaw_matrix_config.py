#!/usr/bin/env python3
"""Generate portable smr.yaml + matrix fixtures for OpenClaw security tests."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "scripts"))

from openclaw_matrix_common import (  # noqa: E402
    CONTENT_RULE_SECRET,
    DLP_SSH_NEEDLE,
    ensure_fixtures,
    matrix_layout,
    matrix_root,
    safe_matrix_root,
    write_env_file,
)
from test_common import parse_high_group  # noqa: E402


def render_config(paths: dict[str, str]) -> str:
    dlp_path = paths["dlp_dir"]
    access = paths["path_deny_access"]
    modify = paths["path_deny_modify"]
    delete = paths["path_deny_delete"]

    endpoint_lines: list[str] = []
    for ep in parse_high_group():
        endpoint_lines.append(
            f"""    - id: {ep["id"]}
      base_url: "{ep["base_url"]}"
      model: "{ep["model"]}"
      protocol: {ep["protocol"]}
      api_key: "{ep["api_key"]}"
      timeout_secs: 120"""
        )
    high_block = "\n".join(endpoint_lines)

    return f"""server:
  listen: "127.0.0.1:8080"
  default_fallback_group: high

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
{high_block}

content_rules:
  - id: matrix-content-secret
    enabled: true
    match_mode: full
    value: "{CONTENT_RULE_SECRET}"
    category: secret
  - id: matrix-ssh-canary
    enabled: true
    match_mode: full
    value: "{DLP_SSH_NEEDLE}"
    category: secret

file_rules:
  - id: matrix-dlp-dir
    enabled: true
    path: "{dlp_path}"
    recursive: true
    trigger_window: 15
    match_mode: fragment
    min_fragment_len: 24
    min_fragment_ratio: 0.4
    formats: ["txt", "md", "pub"]

operation_rules:
  - id: matrix-rm-rf
    enabled: true
    operation: command_exec
    object:
      pattern: "(?i)rm\\\\s+-rf"
      is_regex: true

path_protection_rules:
  - id: matrix-deny-access
    enabled: true
    path: "{access}"
    level: deny_access
  - id: matrix-deny-modify
    enabled: true
    path: "{modify}"
    level: deny_modify
  - id: matrix-deny-delete
    enabled: true
    path: "{delete}"
    level: deny_delete
"""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True, help="smr.yaml path")
    parser.add_argument(
        "--env-file",
        type=Path,
        help="Write SMR_MATRIX_* env file for openclaw_security_matrix_test.py",
    )
    parser.add_argument(
        "--matrix-root",
        type=Path,
        help="Fixture root (default: SMR_MATRIX_ROOT or system temp)",
    )
    parser.add_argument("--fixtures", action="store_true", help="Create fixture dirs/files")
    args = parser.parse_args()

    root = args.matrix_root or matrix_root()
    root_text = str(root).replace("\\", "/")
    if args.fixtures:
        paths = ensure_fixtures(safe_matrix_root(root))
    elif args.matrix_root:
        paths = matrix_layout(Path(str(args.matrix_root).replace("\\", "/")))
    else:
        paths = matrix_layout(safe_matrix_root(root))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render_config(paths), encoding="utf-8")
    print(args.output)

    if args.env_file:
        guest_platform = None
        if args.matrix_root and str(args.matrix_root).replace("\\", "/")[:1].isalpha():
            guest_platform = "windows"
        write_env_file(args.env_file, paths, platform=guest_platform)
        print(args.env_file)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
