#!/usr/bin/env python3
"""Download Windows 11 x64 ISO for UTM. Set SMR_WIN_ISO_URL to skip API lookup."""
from __future__ import annotations

import json
import os
import sys
import urllib.parse
import urllib.request
from pathlib import Path

UA = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36"
)
CONNECTOR = "https://www.microsoft.com/software-download-connector/api"


def request_json(url: str, payload: dict | None = None) -> dict:
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "User-Agent": UA,
            "Accept": "application/json",
            "Content-Type": "application/json",
        },
        method="POST" if payload else "GET",
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        return json.loads(resp.read().decode("utf-8"))


def download_file(url: str, dest: Path) -> None:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=60) as resp:
        total = int(resp.headers.get("Content-Length", 0))
        done = 0
        chunk = 2 * 1024 * 1024
        with dest.open("wb") as out:
            while True:
                block = resp.read(chunk)
                if not block:
                    break
                out.write(block)
                done += len(block)
                if total:
                    print(f"\r  {done * 100 // total:3d}%", end="", flush=True)
        print()


def resolve_download_url() -> str:
    direct = os.environ.get("SMR_WIN_ISO_URL", "").strip()
    if direct:
        return direct

    # Windows 11 consumer ISO (English x64) via Microsoft connector (same flow as download page).
    session = request_json(
        f"{CONNECTOR}/getsession",
        {"profile": "E5D66D8C-EA04-4DA0-B01D-784B00DA5570"},
    )
    sid = session.get("sessionId") or session.get("SessionId")
    if not sid:
        raise RuntimeError("No sessionId from Microsoft connector")

    skus = request_json(
        f"{CONNECTOR}/getskuinformationbyproductedition",
        {
            "sessionId": sid,
            "productEditionId": "Windows 11",
            "language": "English",
            "architecture": "x64",
        },
    )
    sku_list = skus.get("Skus") or skus.get("skus") or []
    if not sku_list:
        raise RuntimeError("No SKU list returned")

    # Prefer multi-edition ISO SKU when present.
    sku_id = str(sku_list[0].get("Id") or sku_list[0].get("id"))
    for item in sku_list:
        name = (item.get("Name") or item.get("name") or "").lower()
        if "iso" in name and "multi" in name:
            sku_id = str(item.get("Id") or item.get("id"))
            break

    links = request_json(
        f"{CONNECTOR}/getproductdownloadlinksbysku",
        {
            "sessionId": sid,
            "skuId": sku_id,
            "language": "English",
            "architecture": "x64",
        },
    )
    for item in links.get("ProductDownloadLinks") or links.get("productDownloadLinks") or []:
        href = item.get("Uri") or item.get("uri") or item.get("Url") or item.get("url")
        if href:
            return href

    raise RuntimeError("No download URI in connector response")


def main() -> int:
    dest = Path(sys.argv[1] if len(sys.argv) > 1 else "Win11_x64.iso").expanduser()
    if dest.exists() and dest.stat().st_size > 4_000_000_000:
        print(f"ISO already present: {dest}")
        return 0

    try:
        url = resolve_download_url()
    except Exception as exc:
        print(f"Automatic download failed: {exc}", file=sys.stderr)
        print(
            "\nManual: https://www.microsoft.com/software-download/windows11\n"
            f"Save ISO as: {dest}\n"
            "Or set SMR_WIN_ISO_URL to a direct https://software.download.prss.microsoft.com/... link.",
            file=sys.stderr,
        )
        return 1

    print(f"Downloading:\n  {urllib.parse.unquote(url)[:120]}...\n  -> {dest}")
    dest.parent.mkdir(parents=True, exist_ok=True)
    download_file(url, dest)

    if dest.stat().st_size < 4_000_000_000:
        dest.unlink(missing_ok=True)
        print("Downloaded file too small.", file=sys.stderr)
        return 1

    print(f"OK ({dest.stat().st_size // (1024**3)} GiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
