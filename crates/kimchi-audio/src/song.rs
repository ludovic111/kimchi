//! ryolune songs on kimchi's timeline: read a `.ryolune` file, render its mix or one track's stem
//! with ryolune's own engine (so it sounds exactly as in ryolune), and write a kimchi cut as a
//! ryolune session (`handoff.toRyolune` with `as: session`).

use std::path::Path;

use serde::Serialize;

use crate::Result;

/// What kimchi needs to know about a song.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongInfo {
    pub name: String,
    /// Starting tempo and time signature.
    pub tempo: f64,
    pub beats_per_bar: f64,
    /// Length in seconds (to the end of the last clip, without the effects' tails).
    pub seconds: f64,
    /// Tracks that make sound, as (ryolune track id, name).
    pub tracks: Vec<(String, String)>,
    /// Song markers, as (seconds, name).
    pub markers: Vec<(f64, String)>,
    /// Every beat in seconds, following the song's tempo changes.
    pub beats: Vec<f64>,
}

/// Reads a song without rendering it.
pub fn info(path: &Path) -> Result<SongInfo> {
    Err(format!("Reading {} isn't built yet.", path.display()))
}

/// Renders the song's mix (or the stem of `track`) to a WAV file at `rate`.
pub fn render(path: &Path, track: Option<&str>, out: &Path, rate: u32) -> Result<SongInfo> {
    let _ = (track, out, rate);
    Err(format!("Rendering {} isn't built yet.", path.display()))
}
