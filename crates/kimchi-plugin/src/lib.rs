//! The kimchi video plugin SDK.
//!
//! A kimchi plugin is a plain Rust type that implements [`Plugin`]: it says who it is
//! ([`Info`]: an effect, a generator or a transition), lists its parameters ([`Param`]: numbers
//! with units, whole numbers, switches, choices, colours, points, angles, text, files) and draws
//! frames in [`Plugin::render`]. Build it as a `cdylib`, put the library in kimchi's plugin folder,
//! and it shows up in the inspector's Plugins section, where its parameters get sliders,
//! switches, menus and colour wells, take keyframes, undo, save with the project, and render the
//! same in the preview and the export. The library is loaded through a small, versioned C ABI
//! ([`ffi`]) that [`export_plugins!`] writes for you.
//!
//! # A complete plugin
//!
//! ```
//! use kimchi_plugin::prelude::*;
//!
//! /// Lifts the shadows towards a colour.
//! pub struct Tint {
//!     color: [f32; 4],
//!     amount: f32,
//! }
//!
//! impl Plugin for Tint {
//!     const INFO: Info = Info::effect("com.example.tint", "Tint", "Example", "Color")
//!         .describe("Pushes the picture towards a colour.");
//!
//!     fn params() -> Vec<Param> {
//!         vec![
//!             color("Color", [1.0, 0.5, 0.2, 1.0]),
//!             number("Amount", 0.0, 100.0, 50.0).unit("%"),
//!         ]
//!     }
//!
//!     fn new(_setup: &Setup) -> Self {
//!         Self { color: [1.0, 0.5, 0.2, 1.0], amount: 0.5 }
//!     }
//!
//!     fn set_param(&mut self, index: usize, value: Value) {
//!         match index {
//!             0 => self.color = value.color(),
//!             1 => self.amount = value.number() as f32 / 100.0,
//!             _ => {}
//!         }
//!     }
//!
//!     fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
//!         let input = inputs[0];
//!         let (tint, k) = (self.color, self.amount);
//!         // Rows are drawn on all of kimchi's threads.
//!         ctx.rows(output, |y, row| {
//!             for (x, px) in row.iter_mut().enumerate() {
//!                 let c = input.straight(x, y);
//!                 let mixed = [0, 1, 2].map(|i| c[i] + (tint[i] - c[i]) * k);
//!                 *px = from_straight([mixed[0], mixed[1], mixed[2], c[3]]);
//!             }
//!         });
//!     }
//! }
//!
//! // In a cdylib, this exports the plugins (several types can be listed):
//! // kimchi_plugin::export_plugins!(Tint);
//!
//! // And a test runs it the way kimchi does, through the ABI:
//! use kimchi_plugin::testing::{Bench, Image};
//! let mut bench = Bench::<Tint>::new();
//! bench.set("Amount", 100.0);
//! let out = bench.effect(&Image::solid(4, 4, [0, 0, 0, 255]));
//! assert_eq!(out.pixel(0, 0), [255, 128, 51, 255]);
//! ```
//!
//! # Frames
//!
//! Frames are what kimchi's compositor works in: premultiplied RGBA, 8 bits per channel, sRGB
//! ([`frame`] explains, and has the conversions to straight colour and to linear light, and
//! bilinear sampling). `output` is the size of the inputs; for generators it starts transparent.
//! Sizes are in frame pixels, which are fewer in the preview than in the export: scale distances
//! by [`RenderCtx::scale`] (or [`RenderCtx::px`]) so the plugin looks the same at every size.
//! [`Param`]s of kind point are fractions of the frame, so they need no scaling.
//!
//! # Time and randomness
//!
//! [`RenderCtx`] has the time from the clip's start, the frame rate, the frame number, the
//! progress of a transition, and whether the frame is a quick preview. Noise and grain must be
//! the same each time a frame is drawn, so use [`random`] seeded with [`RenderCtx::seed`], never
//! the system's random numbers. Mark plugins whose picture moves on its own with
//! [`Info::animated`].
//!
//! # Rules
//!
//! - `render` may run on a different thread each time (never two at once on one instance):
//!   keep state in `self`, no thread-locals.
//! - Don't block or touch the network in `render`; reading a file named by a file parameter in
//!   `set_param` (and keeping what it holds) is fine.
//! - A panic is caught at the ABI: kimchi shows the picture unchanged, says so once, and stops
//!   calling the instance. Don't build plugins with `panic = "abort"`: then a panic would end kimchi.
//! - The plugin's `id` and its parameters' names are stored in projects: never change them.
//!   Add parameters at the end of the list.
//!
//! See `docs/PLUGINS.md` in kimchi's repository for building, installing and testing a plugin.

// The templates (`template/`) are compiled in this crate's tests, where they name it as an outside
// crate would.
extern crate self as kimchi_plugin;

pub mod ffi;
pub mod frame;
mod plugin;
pub mod random;
pub mod testing;

pub use frame::{Frame, FrameMut};
pub use plugin::*;

/// The ABI this SDK speaks, in [`ffi::Entry::abi_version`]. kimchi refuses versions it doesn't
/// know rather than guess at their layout.
pub const ABI_VERSION: u32 = 1;

/// The symbol kimchi looks up in a plugin library.
pub const ENTRY_SYMBOL: &str = "kimchi_plugin_entry";

/// What `plugin.guide` hands an agent: how to write a plugin, the recipe, the rules. The
/// templates follow it (see [`template`]).
pub const GUIDE: &str = include_str!("../GUIDE.md");

/// The starting code `plugin.new` writes, one per kind: a working plugin named `MyEffect`,
/// `MyGenerator` or `MyTransition` with the id `local.plugins.my-effect` (and so on), which
/// `plugin.new` renames. They are compiled and tested with this crate, so they always build.
pub mod template {
    pub const EFFECT: &str = include_str!("../template/effect.rs");
    pub const GENERATOR: &str = include_str!("../template/generator.rs");
    pub const TRANSITION: &str = include_str!("../template/transition.rs");

    /// The template of a kind (`effect`, `generator`, `transition`), with the names it uses:
    /// (code, type name, id, display name).
    pub fn of(kind: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
        Some(match kind {
            "effect" => (EFFECT, "MyEffect", "local.plugins.my-effect", "My effect"),
            "generator" => (GENERATOR, "MyGenerator", "local.plugins.my-generator", "My generator"),
            "transition" => (TRANSITION, "MyTransition", "local.plugins.my-transition", "My transition"),
            _ => return None,
        })
    }
}

// The templates, compiled and tested here.
#[cfg(test)]
#[path = "../template/effect.rs"]
mod template_effect;
#[cfg(test)]
#[path = "../template/generator.rs"]
mod template_generator;
#[cfg(test)]
#[path = "../template/transition.rs"]
mod template_transition;

#[cfg(test)]
mod guide_tests {
    /// The guide names everything the SDK offers a plugin, so an agent reading it misses nothing.
    #[test]
    fn the_guide_keeps_up_with_the_sdk() {
        let g = super::GUIDE;
        for word in ["number(", "integer(", "toggle(", "choice(", "color(", "point(", "angle(", "text(", "file(", "Info::effect", "Info::generator", "Info::transition", ".animated()", "export_plugins!", "ctx.px(", "rows(", "parallel(", "seed(", "Bench", "plugin.new", "plugin.writeSource", "plugin.build", "plugin.publishLocal", "plugin.toolchain", "panic = \"abort\"", "plugin.toml", "~/.lsuite/plugins/kimchi/"] {
            assert!(g.contains(word), "GUIDE.md doesn't mention {word}");
        }
        // Every parameter kind has its constructor in the guide.
        for kind in ["Number", "Integer", "Toggle", "Choice", "Color", "Point", "Angle", "Text", "File"] {
            let k: super::ParamKind = serde_json::from_value(serde_json::json!(kind.to_lowercase())).unwrap();
            let ctor = format!("{}(", kind.to_lowercase());
            assert!(g.contains(&ctor), "{k:?}: {ctor}");
        }
        for kind in ["effect", "generator", "transition"] {
            let (code, ty, id, name) = super::template::of(kind).unwrap();
            assert!(code.contains(ty) && code.contains(id) && code.contains(name), "{kind} template names");
            assert!(!code.contains("export_plugins!"), "plugin.new adds the export line");
        }
    }
}

/// Everything a plugin usually needs.
pub mod prelude {
    pub use crate::frame::{byte_to_linear, from_float, from_linear, from_straight, linear, linear_to_byte, linear_to_srgb, luma, mix, smoothstep, srgb_to_linear, straight, to_byte};
    pub use crate::random::{self, Rng};
    pub use crate::{Frame, FrameMut, Info, Kind, Param, ParamKind, Plugin, RenderCtx, Setup, Value, angle, choice, color, file, integer, number, point, text, toggle};
}

/// Number of types given, for [`export_plugins!`].
#[doc(hidden)]
#[macro_export]
macro_rules! __count {
    ($($plugin:ty),* $(,)?) => {
        <[&str]>::len(&[$(stringify!($plugin)),*])
    };
}

/// Exports the listed plugin types from a `cdylib` under the symbol kimchi looks for
/// (`kimchi_plugin_entry`). Use it once per library.
///
/// ```ignore
/// kimchi_plugin::export_plugins!(Glow, Pixelate, RadialWipe);
/// ```
#[macro_export]
macro_rules! export_plugins {
    ($($plugin:ty),+ $(,)?) => {
        #[doc(hidden)]
        pub mod __kimchi_plugin_export {
            #[allow(unused_imports)]
            use super::*;
            pub static TABLES: [$crate::ffi::PluginVTable; $crate::__count!($($plugin),+)] = [$($crate::ffi::vtable::<$plugin>()),+];
            pub unsafe extern "C" fn plugin(index: u32) -> *const $crate::ffi::PluginVTable {
                TABLES.get(index as usize).map_or(::std::ptr::null(), |t| t as *const _)
            }
            pub static ENTRY: $crate::ffi::Entry = $crate::ffi::Entry { abi_version: $crate::ABI_VERSION, plugin_count: TABLES.len() as u32, plugin };
        }

        /// The library's plugins, for kimchi (see `kimchi_plugin::ffi`).
        #[unsafe(no_mangle)]
        pub extern "C" fn kimchi_plugin_entry() -> *const $crate::ffi::Entry {
            &__kimchi_plugin_export::ENTRY
        }
    };
}
