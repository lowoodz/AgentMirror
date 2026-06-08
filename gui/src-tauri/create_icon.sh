#!/usr/bin/env bash
# Generate Tauri app icon PNG from the canonical SVG source.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"
mkdir -p icons

SRC="${ROOT}/icons/icon.svg"
OUT="${ROOT}/icons/icon.png"

SIZE="${SMR_ICON_SIZE:-2048}"
if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w "$SIZE" -h "$SIZE" "$SRC" -o "$OUT"
  echo "Generated $OUT from icon.svg (rsvg-convert, ${SIZE}px)"
elif command -v magick >/dev/null 2>&1; then
  magick -background none -density 384 "$SRC" -resize "${SIZE}x${SIZE}" "$OUT"
  echo "Generated $OUT from icon.svg (ImageMagick)"
elif [[ "$(uname -s)" == "Darwin" ]] && command -v qlmanage >/dev/null 2>&1; then
  TMPDIR=$(mktemp -d)
  qlmanage -t -s "$SIZE" -o "$TMPDIR" "$SRC" >/dev/null 2>&1
  mv "$TMPDIR/$(basename "$SRC").png" "$OUT"
  rmdir "$TMPDIR"
  echo "Generated $OUT from icon.svg (qlmanage, ${SIZE}px)"
elif [[ -f "$OUT" ]]; then
  echo "Using existing $OUT (install rsvg-convert or ImageMagick to rebuild from SVG)"
else
  echo "Error: no icon.png and cannot rasterize icon.svg" >&2
  exit 1
fi

# Tauri requires RGBA PNG before icon set generation
python3 << PY
import struct, zlib
from pathlib import Path

def chunk(tag, data):
    return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xffffffff)

def ensure_rgba(path: Path) -> None:
    data = path.read_bytes()
    pos = 8
    w = h = ctype = None
    idat = []
    while pos < len(data):
        ln = struct.unpack('>I', data[pos:pos + 4])[0]
        tag = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + ln]
        if tag == b'IHDR':
            w, h, _, ctype, *_ = struct.unpack('>IIBBBBB', body)
        elif tag == b'IDAT':
            idat.append(body)
        pos += 12 + ln
        if tag == b'IEND':
            break
    if ctype == 6:
        return
    raw = zlib.decompress(b''.join(idat))
    stride = w * 3
    out = bytearray()
    for y in range(h):
        row = raw[y * (1 + stride) + 1:y * (1 + stride) + 1 + stride]
        out.append(0)
        for i in range(0, len(row), 3):
            out.extend(row[i:i + 3])
            out.append(255)
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0)
    png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(bytes(out), 9)) + chunk(b'IEND', b'')
    path.write_bytes(png)

ensure_rgba(Path("$OUT"))
print("Ensured RGBA:", "$OUT")
PY

GUI_ROOT="$(cd "$ROOT/.." && pwd)"
if [[ -f "$GUI_ROOT/package.json" ]] && command -v npm >/dev/null 2>&1; then
  (cd "$GUI_ROOT" && npx tauri icon "$OUT" >/dev/null)
  echo "Regenerated Tauri icon set via 'tauri icon'"
fi

# Web favicon for embedded admin UI (64px source for sharper browser downscale)
FAV="${ROOT}/../../crates/smr-core/assets/favicon.png"
if command -v sips >/dev/null 2>&1; then
  sips -z 64 64 "$OUT" --out "$FAV" >/dev/null
  echo "Updated $FAV (64x64)"
elif command -v magick >/dev/null 2>&1; then
  magick "$OUT" -resize 64x64 "$FAV"
  echo "Updated $FAV (64x64)"
fi
