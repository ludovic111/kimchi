# Plugins: the author's reference

kimchi meets lsuite's plugin contract (`PLUGINS.md` in the lsuite repository). This page is for
people and agents who write or install plugins; the window's side is in the
[user guide](guide/plugins.md).

## What kimchi loads

| Format | What | Where kimchi looks |
| --- | --- | --- |
| **lsuite plugins** | Effects, generators and transitions in Rust, built with the `kimchi-plugin` SDK (`crates/kimchi-plugin`) | `~/.lsuite/plugins/kimchi/<id>/` (`LSUITE_HOME` replaces `~/.lsuite`), `KIMCHI_PLUGIN_PATH`, Settings' extra folders (`plugins.videoFolders`) |
| **frei0r** | The video filters, sources and mixers Kdenlive, Shotcut and ffmpeg use | `FREI0R_PATH` (replaces the list), `~/.frei0r-1/lib`, `/usr/lib/frei0r-1`, `/usr/local/lib/frei0r-1`, `/usr/lib/<triple>/frei0r-1`, `/opt/homebrew/lib/frei0r-1`, Shotcut's and Kdenlive's own copies |
| **LUTs** | `.cube`, `.3dl`, `.csp`, `.spi1d`, `.spi3d`, Hald CLUT pictures | imported as looks (`looks.import`) |
| **Sound** | ryolune's effects, CLAP, VST3, Audio Units (macOS) | through ryolune's engine (`audio.effects`, `audio.rescanPlugins`) |

OpenFX and other formats are not loaded, so kimchi doesn't list them.

Every library is first opened in a separate kimchi process (`--scan-video-plugin`), so one that
crashes while loading is listed as failed instead of taking kimchi down. Results are cached in
`<data>/plugins/video.json`; a rescan opens only new and changed files. While the window runs, the
lsuite folder is watched: a bundle added, removed or rebuilt there is picked up within two seconds.

On a clip, plugins run after its colour effects, first to last, in the compositor (the preview and
the export alike). A transition plugin is a clip's `transition.plugin`. A plugin that panics is
switched off for the run and saved as disabled (`plugins.disabled`), and the picture is drawn
without it; `plugin.enable` switches it on again.

## The SDK and the ABI

`kimchi-plugin` has the plugin trait, frames, colour helpers, seeded randomness and a test bench
(`testing::Bench`) that runs a plugin through the real ABI. `kimchi-cli plugin.guide` prints the
whole guide with the three templates; it is also `crates/kimchi-plugin/GUIDE.md`.

The ABI (`crates/kimchi-plugin/src/ffi.rs`) is frozen at version 1, modelled on ryolune's
`sdk/src/ffi.rs`: a library exports `kimchi_plugin_entry` → `Entry { abi_version, plugin_count,
plugin(index) }`; each plugin is a `repr(C)` vtable (`manifest` as JSON, `create`, `destroy`,
`set_param`, `render`, `last_error`), its layout checked at compile time. The host checks
`abi_version` before calling anything, and every call catches panics on the plugin's side: a
plugin that panicked is poisoned and answers errors only. Structures carry their own `size` so later
SDKs can add fields without breaking older hosts or plugins.

## Bundles

A bundle is a folder with `plugin.toml` and the library:

```toml
id = "com.example.halftone"   # reverse-DNS: the folder name once installed
name = "Halftone"
version = "0.1.0"
app = "kimchi"
kind = "effect"               # effect, generator or transition
abi = 1
description = "Dots like print."
authors = ["Ada"]

[library]
macos = "libhalftone.dylib"
linux = "libhalftone.so"
windows = "halftone.dll"
```

`plugin.install path=<folder>` checks it loads, copies it into `~/.lsuite/plugins/kimchi/<id>/` and
loads it; installing it again replaces it in the running app (hot reload: the app loads a copy of
each version of a library, so a new build is a new file). `plugin.remove id=<bundle id>` removes it.

## Making one (the recipe)

```sh
kimchi-cli plugin.toolchain                     # is Rust installed?
kimchi-cli plugin.new name=vhs kind=effect      # ~/.lsuite/plugins-src/kimchi/vhs/
kimchi-cli plugin.writeSource name=vhs path=src/lib.rs contents="$(cat lib.rs)"
kimchi-cli plugin.build name=vhs                # errors as {file, line, column, message}
kimchi-cli plugin.publishLocal name=vhs         # bundle, install, load
kimchi-cli clip.addPlugin clipIds='["Intro"]' plugin=vhs
kimchi-cli project.renderFrame time=1
```

The crate's `Cargo.toml` takes the SDK from `https://github.com/ludovic111/kimchi` at the tag of
the kimchi that made it (`tag = "v0.10.0"`; any tag or branch works). With `KIMCHI_PLUGIN_SDK` set to
a local `crates/kimchi-plugin` folder, `plugin.new` points the crate at that copy instead (a kimchi
built from a checkout does this by itself while the checkout is there). Builds share
`~/.lsuite/plugins-src/kimchi/.target`, so the SDK compiles once.

## Stock SDK plugins

`plugins/examples` (Halftone, Chromatic aberration, Gradient, Radial wipe) is linked into kimchi as
stock plugins through the same vtables, and is also a `cdylib` with its own `plugin.toml`, a bundle
like any other: `cargo build -p kimchi-plugin-examples --release`, then copy the library next to
`plugins/examples/plugin.toml` and `plugin.install` the folder.
