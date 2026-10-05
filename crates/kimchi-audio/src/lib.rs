//! kimchi's sound. Every sample of the preview and the export comes from [`mixer::Mixer`]:
//! ffmpeg only decodes each clip's sound (in `kimchi-media`, through [`mixer::SourceOpener`])
//! and encodes the finished mix. Effects and plugins are ryolune's ([`plugins`]), so a chain
//! sounds the same in both apps; ryolune songs are rendered by ryolune's own engine ([`song`]).
//!
//! The model (what is mixed and how) lives in `kimchi_core::audio`.

pub mod beats;
mod chain;
pub mod devices;
pub mod dsp;
pub mod loudness;
pub mod meter;
pub mod mixer;
pub mod plugins;
pub mod song;
pub mod testing;

/// The mixer's sample rate when the project doesn't say (`ProjectSettings::sample_rate`).
pub const SAMPLE_RATE: u32 = 48_000;

/// One stereo sample frame.
pub type Frame = [f32; 2];

pub type Result<T> = std::result::Result<T, String>;
