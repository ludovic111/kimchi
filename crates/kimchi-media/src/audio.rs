//! The project's sound through kimchi-audio's mixer, with ffmpeg decoding each clip
//! ([`FfmpegOpener`]): offline mixes (exports, stems, loudness, speech), scrub snippets, an
//! asset's samples (beat detection) and the whole audio timeline as a ryolune session.

use std::path::{Path, PathBuf};

use kimchi_audio::loudness::Loudness;
use kimchi_audio::mixer::Selection;
use kimchi_audio::{Frame, SAMPLE_RATE};
use kimchi_core::{Asset, Id, Project};

use crate::{MediaError, MediaResult, Tools};

/// Opens clip sounds with ffmpeg: one raw stereo f32 stream per clip, with speed, reverse,
/// pitch and channels applied.
pub struct FfmpegOpener {
    pub tools: Tools,
}

/// What to measure or mix.
#[derive(Debug, Clone, Default)]
pub struct Range {
    /// Timeline seconds (the whole timeline by default).
    pub span: Option<(f64, f64)>,
    pub selection: Selection,
}

fn not_yet(what: &str) -> MediaError {
    MediaError::Unsupported(format!("{what} isn't built yet"))
}

/// The mix of `range`, offline, at the project's sample rate.
pub async fn render(tools: &Tools, project: &Project, range: &Range) -> MediaResult<Vec<Frame>> {
    let _ = (tools, project, range);
    Err(not_yet("mixing"))
}

/// Loudness of the mix of `range` (EBU R128).
pub async fn measure(tools: &Tools, project: &Project, range: &Range) -> MediaResult<Loudness> {
    let _ = (tools, project, range);
    Err(not_yet("measuring loudness"))
}

/// Loudness of one clip on its own: its source through its own effects, at volume 1 (what
/// `audio.normalize` sets the volume from).
pub async fn clip_loudness(tools: &Tools, project: &Project, clip: Id) -> MediaResult<Loudness> {
    let _ = (tools, project, clip);
    Err(not_yet("measuring a clip"))
}

/// A short piece of the mix from timeline time `t`, to hear while scrubbing.
pub async fn scrub(tools: &Tools, project: &Project, t: f64, seconds: f64) -> MediaResult<Vec<Frame>> {
    let _ = (tools, project, t, seconds);
    Err(not_yet("scrubbing"))
}

/// A whole asset's sound, stereo at `rate` (beat detection, waveforms).
pub async fn asset_samples(tools: &Tools, asset: &Asset, rate: u32) -> MediaResult<Vec<Frame>> {
    let _ = (tools, asset, rate);
    Err(not_yet("decoding sound"))
}

/// Writes the project's audio timeline as a ryolune session at `path`: one ryolune audio track
/// per kimchi track with sound (its clips, fades, gains, effects, fader and pan), the markers,
/// and the cut's length. Returns the file written.
pub async fn write_ryolune_session(tools: &Tools, project: &Project, path: &Path) -> MediaResult<PathBuf> {
    let _ = (tools, project, path);
    Err(not_yet("writing a ryolune session"))
}

/// The mixer's sample rate for a project.
pub fn rate(project: &Project) -> u32 {
    match project.settings.sample_rate {
        8_000..=192_000 => project.settings.sample_rate,
        _ => SAMPLE_RATE,
    }
}
