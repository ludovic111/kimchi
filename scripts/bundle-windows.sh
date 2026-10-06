#!/usr/bin/env bash
# Builds kimchi for x86_64 Windows and packages it (run in Git Bash on Windows, with NSIS's
# makensis on PATH or in its default folder; GitHub's windows runners have both):
#   scripts/bundle-windows.sh
# Writes to target/dist/:
#   kimchi_x64-setup.exe (+ .sig)    per-user installer (lsuite.xyz/kimchi/download/windows-x86_64);
#                                    kimchi 0.1.x's updater installs it
#   kimchi_x64-portable.zip (+ .sig) the same files, to run from any folder (updated by hand: its
#                                    update notice links to the next zip)
#
# Environment (all optional):
#   TAURI_SIGNING_PRIVATE_KEY, TAURI_SIGNING_PRIVATE_KEY_PASSWORD   update key: signs the installer and the zip
#   KIMCHI_SKIP_BUILD=1
set -euo pipefail
cd "$(dirname "$0")/.."

triple=x86_64-pc-windows-msvc
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
resources=crates/kimchi-desktop/resources
dist=target/dist
stage="$dist/$triple/kimchi"
bins=(kimchi kimchi-cli)
[ -f crates/kimchi-mcp/Cargo.toml ] && bins+=(kimchi-mcp)

if [ "${KIMCHI_SKIP_BUILD:-}" != 1 ]; then
  locked=()
  [ -n "${CI:-}" ] && locked=(--locked)
  packages=()
  for b in "${bins[@]}"; do packages+=(-p "$b"); done
  # Static CRT: no Visual C++ redistributable needed.
  RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" \
    cargo build --release ${locked[@]+"${locked[@]}"} --target "$triple" "${packages[@]}"
fi
bash scripts/fetch-ffmpeg.sh "$triple"

rm -rf "$stage"
mkdir -p "$stage"
for b in "${bins[@]}"; do cp "target/$triple/release/$b.exe" "$stage/"; done
cp "target/ffmpeg/$triple/kimchi-ffmpeg.exe" "target/ffmpeg/$triple/kimchi-ffprobe.exe" "target/ffmpeg/$triple/FFMPEG-LICENSE.txt" "$stage/"
cp "$resources/kimchi.ico" "$stage/"
cp LICENSE "$stage/LICENSE.txt"

zip="$dist/kimchi_x64-portable.zip"
rm -f "$zip" "$zip.sig"
(cd "$dist/$triple" && 7z a -tzip -mx=9 "../kimchi_x64-portable.zip" kimchi > /dev/null)

makensis=$(command -v makensis || true)
[ -n "$makensis" ] || makensis="/c/Program Files (x86)/NSIS/makensis.exe"
setup="$dist/kimchi_x64-setup.exe"
rm -f "$setup" "$setup.sig"
win() { cygpath -w "$1" 2> /dev/null || printf '%s' "$1"; }
"$makensis" -V2 -DVERSION="$version" -DSRC="$(win "$PWD/$stage")" -DOUTFILE="$(win "$PWD/$setup")" \
  "$(win "$PWD/$resources/windows/installer.nsi")"

if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  cargo run --quiet --release -p kimchi-release -- sign "$setup" --version "$version"
  cargo run --quiet --release -p kimchi-release -- sign "$zip" --version "$version"
else
  echo "No TAURI_SIGNING_PRIVATE_KEY: $setup and $zip are not signed, so updaters will refuse them." >&2
fi

echo "Built kimchi $version for $triple:"
ls -lh "$setup" "$zip" "$setup.sig" "$zip.sig" 2> /dev/null || true
