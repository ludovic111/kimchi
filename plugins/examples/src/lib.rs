//! Example kimchi video plugins, written with the `kimchi-plugin` SDK. Each file is one plugin and
//! reads on its own: start from the one closest to what you want to make.
//!
//! | Plugin | Kind | Shows how to |
//! |---|---|---|
//! | [`Halftone`] | effect | sample a cell's centre, turn a screen by an angle, colour parameters |
//! | [`ChromaticAberration`] | effect | sample between pixels, scale distances by `ctx.scale` |
//! | [`Gradient`] | generator | draw from nothing, use points |
//! | [`RadialWipe`] | transition | mix two pictures by `ctx.progress` |
//!
//! kimchi links these in as stock plugins (through the same ABI as any library). As a library to
//! install: `cargo build -p kimchi-plugin-examples --release`, then put
//! `target/release/libkimchi_plugin_examples.so` (`.dylib` on macOS, `kimchi_plugin_examples.dll`
//! on Windows) in a folder with this crate's `plugin.toml` and run `kimchi-cli plugin.install`.

mod chromatic_aberration;
mod gradient;
mod halftone;
mod radial_wipe;

pub use chromatic_aberration::ChromaticAberration;
pub use gradient::Gradient;
pub use halftone::Halftone;
pub use radial_wipe::RadialWipe;

/// Who made these.
pub(crate) const VENDOR: &str = "kimchi";

kimchi_plugin::export_plugins!(Halftone, ChromaticAberration, Gradient, RadialWipe);

/// The library's entry, for hosts that link these plugins in (kimchi's stock plugins).
pub fn entry() -> *const kimchi_plugin::ffi::Entry {
    kimchi_plugin_entry()
}
