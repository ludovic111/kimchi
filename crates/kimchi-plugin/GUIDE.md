# Writing a kimchi plugin

kimchi plugins are Rust libraries built with the `kimchi-plugin` SDK. One library holds one or
more plugins of three kinds:

- **effect**: one picture in, one out (a look, a distortion, a stylisation). It goes on clips
  (`clip.addPlugin`).
- **generator**: draws a picture from nothing (patterns, gradients, noise). Put it on a solid
  clip (`clip.addSolid`, then `clip.addPlugin`).
- **transition**: the outgoing and the incoming picture in, one out, as `ctx.progress` goes from 0
  to 1 (`transition.set plugin=…`).

Pictures are premultiplied RGBA, 8 bits per channel, sRGB, the same size in and out. A plugin
draws `output` from `inputs` in `render`, which kimchi calls for every frame of the preview and of
the export, from its compositor's threads (never two calls at once on one instance).

## The recipe

1. `plugin.guide` (this text) and `plugin.toolchain`. When Rust is missing, tell the person and
   offer the install (`installHint`): never install anything without their yes.
2. `plugin.new {name, kind}`: a crate in `~/.lsuite/plugins-src/kimchi/<name>/` with
   `Cargo.toml`, `plugin.toml` and `src/lib.rs` (a working plugin of that kind to change).
3. Write the code: `plugin.writeSource {name, path: "src/lib.rs", contents}` (or your own file
   tools, inside the crate only).
4. `plugin.build {name}` until it is green. Errors come back as `{file, line, column, message}`:
   fix them one by one. Warnings are fine.
5. `plugin.publishLocal {name}`: the library and `plugin.toml` become a bundle, installed in
   `~/.lsuite/plugins/kimchi/<id>/` and loaded at once (no restart).
6. Try it: `clip.addPlugin {clipIds, plugin: "<name>"}` (or `transition.set` for a transition),
   then `project.renderFrame` and look at the picture. Adjust, build, publish again: the new
   build replaces the old one in the running app.

## The SDK in one page

```rust
use kimchi_plugin::prelude::*;

pub struct Tint { amount: f32 }

impl Plugin for Tint {
    // Reverse-DNS id: stored in projects, never change it.
    const INFO: Info = Info::effect("local.plugins.tint", "Tint", "Me", "Color")
        .describe("One short sentence.");
    fn params() -> Vec<Param> { vec![number("Amount", 0.0, 100.0, 50.0).unit("%")] }
    fn new(_setup: &Setup) -> Self { Self { amount: 0.5 } }
    fn set_param(&mut self, index: usize, value: Value) {
        if index == 0 { self.amount = value.number() as f32 / 100.0 }
    }
    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let c = input.straight(x, y);
                *px = from_straight([c[0], c[1] * (1.0 - self.amount), c[2], c[3]]);
            }
        });
    }
}

kimchi_plugin::export_plugins!(Tint);
```

- `Info::effect(id, name, vendor, category)`, `Info::generator(…)`, `Info::transition(id, name,
  vendor)`; `.describe("…")`, `.version("1.2.0")`, and `.animated()` when the picture changes
  with time on its own (grain, flicker): otherwise kimchi keeps the result of a still picture.
- Parameters, in a fixed order (`set_param` gets their index): `number(name, min, max, default)`,
  `integer(…)`, `toggle(name, default)`, `choice(name, &["A", "B"], default_index)`,
  `color(name, [r, g, b, a])` (straight sRGB 0…1), `point(name, [x, y])` (fractions of the
  picture), `angle(name, degrees)`, `text(name, default)`, `file(name, &["png"])`. Refine with
  `.unit("px")`, `.hint("tooltip")`, `.group("Advanced")`, `.label("Shown name")`,
  `.range(min, max)`. Numbers, integers, angles, colours and points take keyframes in kimchi
  (`plugins.<slot>.<name>`). Never rename a parameter or change the order once the plugin is
  used: add new ones at the end.
- `Value`: `.number()`, `.integer()`, `.toggle()`, `.choice()`, `.color()`, `.point()`,
  `.text()`. Values always come checked: of the parameter's kind and in its range.
- `Frame` (input): `.width()`, `.height()`, `.pixel(x, y)` (premultiplied bytes),
  `.straight(x, y)` and `.linear(x, y)` (floats 0…1), `.sample(x, y)` (bilinear, edges
  clamped), `.sample_or_clear(x, y)` (transparent outside). `FrameMut` (output): `.set(…)`,
  `.set_straight(…)`, `.set_linear(…)`, `.row_mut(y)`, `.fill(px)`, `.copy_from(&frame)`.
- Colour helpers: `straight(px)`, `from_straight(c)`, `linear(px)`, `from_linear(c)`,
  `from_float(c)` (premultiplied floats), `luma(c)`, `mix(a, b, t)`, `smoothstep(e0, e1, x)`.
- `RenderCtx`: `time` (seconds from the clip's start), `fps`, `frame()`, `progress`
  (transitions), `draft` (a quick preview frame: draw cheaper if you can), `scale` and
  `px(project_pixels)`: sizes in project pixels must go through `ctx.px(…)` so the effect looks
  the same in the small preview and the full-size export. `rows(output, |y, row| …)` and
  `parallel(n, |i| …)` spread work over kimchi's threads; `seed(salt)` and the `random` module
  give randomness that is the same for the same frame (never use the system's random numbers).
- `kimchi_plugin::testing::{Bench, Image}` runs a plugin through the real ABI in a unit test:
  `Bench::<Tint>::new().set("Amount", 100.0).effect(&Image::solid(4, 4, [0, 0, 0, 255]))`.

## Rules

- Don't panic on purpose, don't block, don't touch the network or the clock in `render`. A
  panic is caught: kimchi switches the plugin off (Settings, Plugins: switch it on again after a
  fix) and draws the picture without it. Never set `panic = "abort"`.
- `render` may run on a different thread each time: keep state in `self`.
- The plugin's `id`, its parameters' names and their order are stored in projects.
- One `export_plugins!(A, B, …)` per library.
- Keep `plugin.toml`'s `id`, `name`, `version`, `kind` and `description` true; `abi` is the SDK's
  ABI version and `[library]` names the built file on each system.

## Files

- Bundle: a folder with `plugin.toml` and the library (`.dylib` on macOS, `.so` on Linux, `.dll`
  on Windows). Installed in `~/.lsuite/plugins/kimchi/<id>/` (`LSUITE_HOME` replaces
  `~/.lsuite`).
- Sources: `~/.lsuite/plugins-src/kimchi/<name>/`. The crate's `Cargo.toml` takes
  `kimchi-plugin` from github.com/ludovic111/kimchi at the tag of the kimchi it was made in
  (`KIMCHI_PLUGIN_SDK=/path/to/kimchi/crates/kimchi-plugin` makes `plugin.new` use a local copy
  instead).
