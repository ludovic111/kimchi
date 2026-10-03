#!/usr/bin/env bash
# Downloads ffmpeg/ffprobe builds for a Rust target triple into target/ffmpeg/<triple>/ as
# kimchi-ffmpeg[.exe] and kimchi-ffprobe[.exe] (the names kimchi-media's Tools::locate looks for
# next to the executable), plus FFMPEG-LICENSE.txt. The bundle scripts copy them into the app.
# On Linux they are links into bin/, next to lib/ with the ffmpeg libraries they share (found
# through their $ORIGIN/../lib rpath, the layout of the Linux packages too).
#   scripts/fetch-ffmpeg.sh [triple] [--force]      (triple defaults to this machine's)
# All GPL builds, each with the hardware encoders of its platform (kimchi-media's accel.rs):
#   macOS, Windows  https://github.com/eugeneware/ffmpeg-static (VideoToolbox; NVENC, AMF, Quick Sync, Media Foundation)
#   Linux           https://github.com/BtbN/FFmpeg-Builds, the 9.0 branch, shared (NVENC, VA-API, Quick Sync, AMF,
#                   Vulkan); needs only glibc ≥ 2.28 and loads the GPU drivers at run time
set -euo pipefail
cd "$(dirname "$0")/.."

static="https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1"
btbn="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest"
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
  aarch64-apple-darwin) source="$static#darwin-arm64" ;;
  x86_64-apple-darwin) source="$static#darwin-x64" ;;
  x86_64-pc-windows-msvc) source="$static#win32-x64" ;;
  x86_64-unknown-linux-gnu) source="$btbn/ffmpeg-n9.0-latest-linux64-gpl-shared-9.0.tar.xz" ;;
  aarch64-unknown-linux-gnu) source="$btbn/ffmpeg-n9.0-latest-linuxarm64-gpl-shared-9.0.tar.xz" ;;
  *) echo "No ffmpeg build for $triple (supported: aarch64/x86_64-apple-darwin, x86_64/aarch64-unknown-linux-gnu, x86_64-pc-windows-msvc)" >&2; exit 1 ;;
esac
ext=""
case "$triple" in *windows*) ext=".exe" ;; esac

dir="target/ffmpeg/$triple"
mkdir -p "$dir"
# A checkout that fetched from another source (an older script) gets the new build.
if [ "$force" = 0 ] && [ -s "$dir/kimchi-ffmpeg$ext" ] && [ -s "$dir/kimchi-ffprobe$ext" ] && [ -s "$dir/FFMPEG-LICENSE.txt" ] \
  && [ "$(cat "$dir/SOURCE" 2>/dev/null)" = "$source" ]; then
  echo "have ffmpeg for $triple in $dir"
  exit 0
fi

case "$source" in
  *.tar.xz)
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL --retry 3 "$source" | tar -xJ -C "$tmp"
    root=$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -1)
    rm -rf "$dir/bin" "$dir/lib" "$dir/kimchi-ffmpeg" "$dir/kimchi-ffprobe"
    mkdir -p "$dir/bin" "$dir/lib"
    for tool in ffmpeg ffprobe; do
      install -m 755 "$root/bin/$tool" "$dir/bin/kimchi-$tool"
      ln -s "bin/kimchi-$tool" "$dir/kimchi-$tool"
    done
    # The runtime libraries only (libavcodec.so.63 and its versioned file), not the dev links.
    cp -a "$root"/lib/lib*.so.[0-9]* "$dir/lib/"
    cp "$root/LICENSE.txt" "$dir/FFMPEG-LICENSE.txt"
    ;;
  *)
    base="${source%#*}"
    platform="${source#*#}"
    for tool in ffmpeg ffprobe; do
      out="$dir/kimchi-$tool$ext"
      curl -fsSL --retry 3 "$base/$tool-$platform.gz" | gunzip > "$out.part"
      chmod 755 "$out.part"
      mv "$out.part" "$out"
    done
    curl -fsSL --retry 3 "$base/$platform.LICENSE" -o "$dir/FFMPEG-LICENSE.txt"
    ;;
esac
echo "$source" > "$dir/SOURCE"
echo "fetched ffmpeg for $triple into $dir ($("$dir/kimchi-ffmpeg$ext" -hide_banner -version 2>/dev/null | head -1 || echo "not runnable here"))"
