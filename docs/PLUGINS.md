# Video plugins

kimchi runs three kinds of video plugins on clips:

| Format | Made for | Where kimchi looks |
|---|---|---|
| **kimchi** | kimchi, with the `kimchi-plugin` Rust SDK | `<kimchi data>/plugins/video`, `KIMCHI_PLUGIN_PATH`, `~/.local/lib/kimchi/plugins`, `/usr/lib/kimchi/plugins` (Linux), `~/Library/Application Support/kimchi/Plugins` (macOS), `%COMMONPROGRAMFILES%\kimchi\Plugins` (Windows) |
| **frei0r** | Kdenlive, Shotcut, Flowblade, ffmpeg (`frei0r-plugins` packages) | `FREI0R_PATH`, `~/.frei0r-1/lib`, `/usr/lib/frei0r-1`, `/usr/local/lib/frei0r-1` and the other usual places |
| **OpenFX** | DaVinci Resolve, Natron, VEGAS, Nuke, Fusion (Boris FX, Sapphire, Neat Video, openfx-misc…) | `OFX_PLUGIN_PATH`, `/usr/OFX/Plugins`, `/Library/OFX/Plugins`, `%COMMONPROGRAMFILES%\OFX\Plugins` |

Folders added in **Settings › Plugins** are searched for every format. kimchi also has plugins
built in (Glow, Pixelate, RGB split, Film grain, Duotone, Gradient, Checker, Noise, Radial wipe):
they are the examples below, linked into kimchi through the same interface.

Every host is written in Rust and opens the plugin's library itself; there is nothing else to
install. Each file is first opened in a separate kimchi process, so a plugin that crashes while
loading is listed under "failed" instead of taking kimchi down. Results are remembered: a rescan
(`plugins.rescan`, or **Rescan** in Settings › Plugins) only opens new and changed files.

On a clip, plugins run on its picture after its colour corrections, first to last. Effects change
the picture; generators draw one (put a generator on a solid); transitions go between two clips
(`transition.set plugin="Radial wipe"`, or the transition's menu on the timeline). Their numbers,
points and colours take keyframes as `plugins.<slot>.<parameter>` (`plugins.p1.Radius`). A
project opened on a computer without one of its plugins keeps the plugin's settings and shows it
as missing; the clip is drawn without it.

```sh
kimchi-cli plugins.list query=glow
kimchi-cli clip.addPlugin clipIds='["Intro"]' plugin=Glow params='{"Radius": 60}'
kimchi-cli clip.setKeyframes clipId=Intro property=plugins.p1.Intensity keyframes='[{"time":0,"value":0},{"time":2,"value":200}]'
kimchi-cli plugins.params clipId=Intro slot=p1
```

## Writing a kimchi plugin

A plugin is a Rust type implementing `kimchi_plugin::Plugin`. Start a library crate:

```toml
[package]
name = "my-plugins"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib", "rlib"]   # the cdylib is what kimchi loads; the rlib lets tests use it

[dependencies]
kimchi-plugin = { git = "https://github.com/ludovic111/kimchi" }
```

```rust
use kimchi_plugin::prelude::*;

pub struct Posterize { levels: f32 }

impl Plugin for Posterize {
    const INFO: Info = Info::effect("com.example.posterize", "Posterize", "Example", "Stylize")
        .describe("Fewer shades of each colour.");

    fn params() -> Vec<Param> {
        vec![integer("Levels", 2, 32, 6).hint("Shades per colour.")]
    }

    fn new(_: &Setup) -> Self {
        Self { levels: 6.0 }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        if index == 0 {
            self.levels = value.integer() as f32;
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        let n = self.levels - 1.0;
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let c = input.straight(x, y);
                let q = |v: f32| (v * n).round() / n;
                *px = from_straight([q(c[0]), q(c[1]), q(c[2]), c[3]]);
            }
        });
    }
}

kimchi_plugin::export_plugins!(Posterize);
```

What to know:

- **Kinds.** `Info::effect` gets one input frame; `Info::generator` none (its output starts
  transparent); `Info::transition` two (outgoing, incoming) and `ctx.progress` from 0 to 1.
- **Parameters** are typed: `number` (with `.unit("px")`), `integer`, `toggle`, `choice`, `color`,
  `point`, `angle`, `text`, `file`. kimchi draws the controls (sliders, switches, menus, colour wells,
  x/y fields, a file picker), stores the values in the project, undoes them, and animates numbers,
  points and colours. `set_param` always gets a value of the parameter's kind, inside its range.
  The plugin's `id` and its parameters' names are stored in projects: never change them; add new
  parameters at the end.
- **Frames** are premultiplied RGBA, 8 bits, sRGB, like kimchi's compositor. `Frame::straight`,
  `Frame::linear` and `Frame::sample` (bilinear) read them in the form the maths needs, and the
  `set_*` methods write them back. `ctx.rows` and `ctx.chunks` spread work over kimchi's threads.
- **Sizes.** The preview draws smaller frames than the export: multiply distances given in project
  pixels by `ctx.scale` (or `ctx.px(…)`). Points are fractions of the frame and need nothing.
- **Time.** `ctx.time` is seconds from the clip's start, `ctx.frame()` the frame number. Randomness
  must repeat for a given frame (preview and export must match): use `kimchi_plugin::random` with
  `ctx.seed(…)`. Mark plugins that change on their own over time with `.animated()`.
- **Threads.** `render` may run on a different thread each call, never two at once on one instance.
- **Failures.** A panic is caught where kimchi calls the plugin: the picture shows unchanged, the
  error is logged once, and that instance isn't called again. Don't build with `panic = "abort"`.

### Testing

`kimchi_plugin::testing::Bench` runs the plugin as kimchi does, through the C interface, with
values checked against its parameter list:

```rust
#[test]
fn two_levels_are_black_and_white() {
    use kimchi_plugin::testing::{Bench, Image};
    let mut bench = Bench::<Posterize>::new();
    bench.set("Levels", 2);
    let out = bench.effect(&Image::solid(4, 4, [100, 200, 30, 255]));
    assert_eq!(out.pixel(0, 0), [0, 255, 0, 255]);
}
```

`Bench` also has `generate`, `transition`, `at(time)`, `scale` and `draft`, and `Image` makes test
pictures (`solid`, `card`, `from_fn`) and compares them (`difference`, `mean`). The plugins in
`plugins/examples` in kimchi's repository each have tests like this.

### Installing

```sh
cargo build --release
cp target/release/libmy_plugins.so "$HOME/.local/share/kimchi/plugins/video/"    # Linux
# macOS: target/release/libmy_plugins.dylib → ~/Library/Application Support/kimchi/plugins/video/
# Windows: target\release\my_plugins.dll → %APPDATA%\kimchi\plugins\video\
kimchi-cli plugins.rescan
```

Settings › Plugins shows kimchi's own folder (and opens it), what was found per format, and what
failed with the reason.

### The interface

The library exports `kimchi_plugin_entry`, returning the ABI version (1) and one table of
functions per plugin; `export_plugins!` writes it. Metadata crosses as JSON, values as small
tagged structs, frames as pointers with a stride; every table and context starts with its size,
so later versions can add to them without breaking plugins built today. kimchi refuses a library
built for an ABI it doesn't know, with a message saying which side to update. `kimchi_plugin::ffi`
documents every structure, for anyone writing a host of their own.
