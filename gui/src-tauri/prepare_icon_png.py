#!/usr/bin/env python3
"""Prepare a raster app-icon source: resize, rounded alpha mask, RGBA PNG (no white fringe)."""
from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image, ImageDraw


def rounded_icon(
    src: Path,
    out: Path,
    *,
    size: int = 1024,
    corner_ratio: float = 0.22,
) -> None:
    im = Image.open(src).convert("RGBA")
    im = im.resize((size, size), Image.Resampling.LANCZOS)

    radius = max(1, int(size * corner_ratio))
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)

    r, g, b, a = im.split()
    a = Image.composite(a, Image.new("L", (size, size), 0), mask)
    out.parent.mkdir(parents=True, exist_ok=True)
    Image.merge("RGBA", (r, g, b, a)).save(out, format="PNG", optimize=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("src", type=Path, help="Input PNG (e.g. AgentMirror-ICON.png)")
    parser.add_argument(
        "-o",
        "--out",
        type=Path,
        default=Path(__file__).resolve().parent / "icons" / "icon-source.png",
        help="Output RGBA PNG for tauri icon",
    )
    parser.add_argument("-s", "--size", type=int, default=1024, help="Output square size")
    parser.add_argument(
        "--corner-ratio",
        type=float,
        default=0.22,
        help="Rounded-corner radius as fraction of size (macOS squircle approx.)",
    )
    args = parser.parse_args()
    rounded_icon(args.src, args.out, size=args.size, corner_ratio=args.corner_ratio)
    print(f"Wrote {args.out} ({args.size}x{args.size}, RGBA, rounded alpha)")


if __name__ == "__main__":
    main()
