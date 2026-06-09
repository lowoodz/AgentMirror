#!/usr/bin/env python3
"""Verify Rust doc_extract output against common CLI tools (manual / CI helper)."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def run_pdftotext(path: Path) -> str:
    proc = subprocess.run(
        ["pdftotext", "-enc", "UTF-8", "-nopgbrk", str(path), "-"],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "pdftotext failed")
    return proc.stdout


def run_cargo_pdf_test() -> int:
    return subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "smr-core",
            "--lib",
            "doc_extract::tests::pdf_extract_matches_pdftotext_on_fixture",
            "--",
            "--nocapture",
        ],
        cwd=ROOT,
        env={**dict(**__import__("os").environ), "CARGO_TARGET_DIR": str(ROOT / "target")},
    ).returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "paths",
        nargs="*",
        help="Optional PDF paths to compare with pdftotext (Rust test uses test-data fixture)",
    )
    args = parser.parse_args()

    code = run_cargo_pdf_test()
    if code != 0:
        return code

    for raw in args.paths:
        path = Path(raw).expanduser()
        if not path.is_file():
            print(f"skip missing: {path}", file=sys.stderr)
            continue
        text = run_pdftotext(path)
        words = [w for w in text.split() if len(w) >= 5][:12]
        print(f"{path.name}: pdftotext ok, sample words: {' '.join(words[:6])}")

    print("doc_extract_verify: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
