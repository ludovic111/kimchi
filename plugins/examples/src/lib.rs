//! Example kimchi video plugins, written with the `kimchi-plugin` SDK. Each file is one plugin and
//! reads on its own: start from the one closest to what you want to make.
//!
//! | Plugin | Kind | Shows how to |
//! |---|---|---|
//! | [`Glow`] | effect | work in linear light, blur at a lower resolution, scale by `ctx.scale` |
//! | [`Pixelate`] | effect | average blocks, use a choice parameter |
//! | [`RgbSplit`] | effect | sample between pixels, use an angle |
//! | [`FilmGrain`] | effect | animated, seeded randomness that matches in preview and export |
//! | [`Duotone`] | effect | read straight colour, use colour parameters |
//! | [`Gradient`] | generator | draw from nothing, use points |
//! | [`Checker`] | generator | patterns with an angle and an offset |
//! | [`Noise`] | generator | smooth noise, a seed parameter |
//! | [`RadialWipe`] | transition | mix two pictures by `ctx.progress` |
//!
//! Build the library with `cargo build -p kimchi-plugin-examples --release` and copy
//! `target/release/libkimchi_plugin_examples.so` (`.dylib` on macOS, `kimchi_plugin_examples.dll`
//! on Windows) into kimchi's plugin folder. kimchi also links these same plugins in as its
//! built-in ones, through the same ABI.

mod blur;
mod checker;
mod duotone;
mod glow;
mod gradient;
mod grain;
mod noise;
mod pixelate;
mod radial_wipe;
mod rgb_split;

pub use checker::Checker;
pub use duotone::Duotone;
pub use glow::Glow;
pub use gradient::Gradient;
pub use grain::FilmGrain;
pub use noise::Noise;
pub use pixelate::Pixelate;
pub use radial_wipe::RadialWipe;
pub use rgb_split::RgbSplit;

/// Who made these.
pub(crate) const VENDOR: &str = "kimchi";

kimchi_plugin::export_plugins!(Glow, Pixelate, RgbSplit, FilmGrain, Duotone, Gradient, Checker, Noise, RadialWipe);

/// The library's entry, for hosts that link these plugins in (kimchi's built-in plugins).
pub fn entry() -> *const kimchi_plugin::ffi::Entry {
    kimchi_plugin_entry()
}
