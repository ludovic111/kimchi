#!/usr/bin/env bash
# Renders kimchi's app icon from brand/icon.svg (the lsuite icon template) into every format the
# packages need, and writes them to the repository (run it after changing the SVG, then commit):
#   brand/icon.png                                   1024 px
#   crates/kimchi-desktop/resources/kimchi.icns      macOS (needs iconutil, so run on a Mac)
#   crates/kimchi-desktop/resources/kimchi.ico       Windows (16–256 px, PNG-compressed)
#   crates/kimchi-desktop/resources/kimchi.png       Linux (512 px; .desktop, AppImage, .deb)
# Needs resvg (`cargo install resvg`) or rsvg-convert (`brew install librsvg`), and python3.
set -euo pipefail
cd "$(dirname "$0")/.."
svg=brand/icon.svg
out=crates/kimchi-desktop/resources
mkdir -p "$out"

render() { # size output
  if command -v resvg > /dev/null; then
    resvg -w "$1" -h "$1" "$svg" "$2"
  elif command -v rsvg-convert > /dev/null; then
    rsvg-convert -w "$1" -h "$1" "$svg" -o "$2"
  else
    echo "Install resvg (cargo install resvg) or rsvg-convert to render the icon." >&2
    exit 1
  fi
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

render 1024 brand/icon.png
render 512 "$out/kimchi.png"

# Each size is rendered from the SVG, not scaled down, so small sizes stay sharp.
if command -v iconutil > /dev/null; then
  set_dir="$work/kimchi.iconset"
  mkdir -p "$set_dir"
  for size in 16 32 128 256 512; do
    render "$size" "$set_dir/icon_${size}x${size}.png"
    render $((size * 2)) "$set_dir/icon_${size}x${size}@2x.png"
  done
  iconutil -c icns "$set_dir" -o "$out/kimchi.icns"
else
  echo "iconutil not found (not a Mac): kept the existing $out/kimchi.icns" >&2
fi

sizes=(16 24 32 48 64 128 256)
for size in "${sizes[@]}"; do render "$size" "$work/ico-$size.png"; done
python3 - "$out/kimchi.ico" "${sizes[@]/#/$work/ico-}" <<'PY'
# An .ico whose entries are PNGs (Windows Vista and later), largest last.
import struct, sys
out, files = sys.argv[1], [f + ".png" for f in sys.argv[2:]]
images = [open(f, "rb").read() for f in files]
header = struct.pack("<HHH", 0, 1, len(images))
offset = 6 + 16 * len(images)
entries = b""
for f, data in zip(files, images):
    w, h = struct.unpack(">II", data[16:24])
    entries += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
open(out, "wb").write(header + entries + b"".join(images))
PY
ls -l brand/icon.png "$out"/kimchi.*
