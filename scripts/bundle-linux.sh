#!/usr/bin/env bash
# Builds kimchi for x86_64 Linux and packages it (run on Linux, e.g. Ubuntu 22.04 for an old
# enough glibc):
#   scripts/bundle-linux.sh
# Writes to target/dist/:
#   kimchi_amd64.AppImage (+ .sig)   one-file app; the in-app updaters replace it in place
#   kimchi_amd64.deb (+ .sig)        Debian/Ubuntu package (/usr/lib/kimchi, /usr/bin/kimchi)
# These are the names lsuite.xyz/kimchi/download/linux-{appimage,deb} look for.
#
# The binaries look for shared libraries in ../lib first (rpath, like Zed), where the libraries
# ldd finds are copied, except glibc's and the GPU/display stack the system must provide.
#
# Environment (all optional):
#   TAURI_SIGNING_PRIVATE_KEY, TAURI_SIGNING_PRIVATE_KEY_PASSWORD   update key: signs both files
#   APPIMAGETOOL      path to appimagetool (downloaded otherwise)
#   KIMCHI_SKIP_BUILD=1
set -euo pipefail
cd "$(dirname "$0")/.."

triple=x86_64-unknown-linux-gnu
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
resources=crates/kimchi-desktop/resources
dist=target/dist
work="$dist/$triple"
bins=(kimchi kimchi-cli)
[ -f crates/kimchi-mcp/Cargo.toml ] && bins+=(kimchi-mcp)

if [ "${KIMCHI_SKIP_BUILD:-}" != 1 ]; then
  locked=()
  [ -n "${CI:-}" ] && locked=(--locked)
  packages=()
  for b in "${bins[@]}"; do packages+=(-p "$b"); done
  RUSTFLAGS="${RUSTFLAGS:-} -C link-args=-Wl,--disable-new-dtags,-rpath,\$ORIGIN/../lib" \
    cargo build --release ${locked[@]+"${locked[@]}"} --target "$triple" "${packages[@]}"
fi
bash scripts/fetch-ffmpeg.sh "$triple"

# One tree for both packages: bin/ (our binaries and ffmpeg), lib/ (bundled libraries), share/.
tree="$work/kimchi"
rm -rf "$work"
mkdir -p "$tree/bin" "$tree/lib" "$tree/share/applications" "$tree/share/icons/hicolor/512x512/apps" "$tree/share/doc/kimchi"
for b in "${bins[@]}"; do
  cp "target/$triple/release/$b" "$tree/bin/$b"
  strip --strip-debug "$tree/bin/$b" || true
done
cp "target/ffmpeg/$triple/kimchi-ffmpeg" "target/ffmpeg/$triple/kimchi-ffprobe" "$tree/bin/"
cp "target/ffmpeg/$triple/FFMPEG-LICENSE.txt" LICENSE "$tree/share/doc/kimchi/"
cp "$resources/kimchi.desktop" "$tree/share/applications/kimchi.desktop"
cp "$resources/kimchi.png" "$tree/share/icons/hicolor/512x512/apps/kimchi.png"

# Libraries: everything ldd resolves except glibc, the C++/GCC runtime, and the graphics and
# display stack (drivers must match the system's).
skip='^(linux-vdso|ld-linux|libc|libm|libdl|libpthread|librt|libresolv|libgcc_s|libstdc\+\+|libGL|libEGL|libGLX|libGLdispatch|libvulkan|libdrm|libgbm|libwayland|libX|libxcb|libxkbcommon|libasound|libdbus-1|libsystemd|libfontconfig|libfreetype|libexpat|libz)\.'
ldd "$tree/bin/kimchi" | awk '/=> \// { print $1 " " $3 }' | while read -r name path; do
  if ! printf '%s\n' "$name" | grep -Eq "$skip"; then
    cp -L "$path" "$tree/lib/"
    echo "bundled $name"
  fi
done

sign_update() {
  if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    cargo run --quiet --release -p kimchi-release -- sign "$1" --version "$version"
  else
    echo "No TAURI_SIGNING_PRIVATE_KEY: $1 is not signed, so updaters will refuse it." >&2
  fi
}

# ---- AppImage -------------------------------------------------------------
appdir="$work/kimchi.AppDir"
mkdir -p "$appdir/usr"
cp -a "$tree/bin" "$tree/lib" "$tree/share" "$appdir/usr/"
cp "$resources/kimchi.desktop" "$appdir/kimchi.desktop"
cp "$resources/kimchi.png" "$appdir/kimchi.png"
ln -s kimchi.png "$appdir/.DirIcon"
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/kimchi" "$@"
APPRUN
chmod 755 "$appdir/AppRun"

tool=${APPIMAGETOOL:-}
if [ -z "$tool" ]; then
  tool="$work/appimagetool"
  curl -fsSL --retry 3 -o "$tool" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod 755 "$tool"
fi
appimage="$dist/kimchi_amd64.AppImage"
rm -f "$appimage" "$appimage.sig"
# No FUSE on CI runners: let the tool unpack itself.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 VERSION="$version" "$tool" --no-appstream "$appdir" "$appimage"
chmod 755 "$appimage"
sign_update "$appimage"

# ---- .deb -----------------------------------------------------------------
pkg="$work/deb"
mkdir -p "$pkg/DEBIAN" "$pkg/usr/lib/kimchi" "$pkg/usr/bin" "$pkg/usr/share"
cp -a "$tree/bin" "$tree/lib" "$pkg/usr/lib/kimchi/"
cp -a "$tree/share/." "$pkg/usr/share/"
for b in "${bins[@]}"; do ln -s "../lib/kimchi/bin/$b" "$pkg/usr/bin/$b"; done
size=$(du -sk "$pkg/usr" | cut -f1)
cat > "$pkg/DEBIAN/control" <<CONTROL
Package: kimchi
Version: $version
Section: video
Priority: optional
Architecture: amd64
Maintainer: Ludovic Marie <ludovic111@users.noreply.github.com>
Installed-Size: $size
Depends: libc6 (>= 2.35), libvulkan1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libx11-xcb1, libxcb1, libfontconfig1, libfreetype6, libdbus-1-3
Homepage: https://lsuite.xyz/kimchi
Description: Video editor with generation in the cut
 kimchi is an open-source video editor with a built-in harness for local and cloud
 image and video generation models. Part of lsuite.
CONTROL
deb="$dist/kimchi_amd64.deb"
rm -f "$deb" "$deb.sig"
dpkg-deb --root-owner-group --build "$pkg" "$deb"
sign_update "$deb"

echo "Built kimchi $version for $triple:"
ls -lh "$appimage" "$deb" "$appimage.sig" "$deb.sig" 2> /dev/null || true
