#!/usr/bin/env bash
# Downloads static ffmpeg/ffprobe builds for a Rust target triple into target/ffmpeg/<triple>/ as
# kimchi-ffmpeg[.exe] and kimchi-ffprobe[.exe] (the names kimchi-media's Tools::locate looks for
# next to the executable), plus FFMPEG-LICENSE.txt. The bundle scripts copy them into the app.
#   scripts/fetch-ffmpeg.sh [triple] [--force]      (triple defaults to this machine's)
# Builds come from https://github.com/eugeneware/ffmpeg-static (GPL; licence copied alongside).
set -euo pipefail
cd "$(dirname "$0")/.."

release="https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1"
triple=""
force=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    *) triple="$arg" ;;
  esac
done
[ -n "$triple" ] || triple=$(rustc -vV | sed -n 's/^host: //p')

case "$triple" in
  aarch64-apple-darwin) platform=darwin-arm64 ;;
  x86_64-apple-darwin) platform=darwin-x64 ;;
  x86_64-unknown-linux-gnu) platform=linux-x64 ;;
  aarch64-unknown-linux-gnu) platform=linux-arm64 ;;
  x86_64-pc-windows-msvc) platform=win32-x64 ;;
  *) echo "No ffmpeg build for $triple (supported: aarch64/x86_64-apple-darwin, x86_64/aarch64-unknown-linux-gnu, x86_64-pc-windows-msvc)" >&2; exit 1 ;;
esac
ext=""
case "$triple" in *windows*) ext=".exe" ;; esac

dir="target/ffmpeg/$triple"
mkdir -p "$dir"
for tool in ffmpeg ffprobe; do
  out="$dir/kimchi-$tool$ext"
  if [ -s "$out" ] && [ "$force" = 0 ]; then
    echo "have $out"
    continue
  fi
  curl -fsSL --retry 3 "$release/$tool-$platform.gz" | gunzip > "$out.part"
  chmod 755 "$out.part"
  mv "$out.part" "$out"
  echo "fetched $out"
done
[ -s "$dir/FFMPEG-LICENSE.txt" ] && [ "$force" = 0 ] || curl -fsSL --retry 3 "$release/$platform.LICENSE" -o "$dir/FFMPEG-LICENSE.txt"
echo "ffmpeg for $triple is in $dir"
