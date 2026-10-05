//! The sound of a project as speech recognition hears it: the mixer's output, 16 kHz mono.

use kimchi_core::Project;

use crate::audio::Range;
use crate::export::SPEECH_SAMPLE_RATE;
use crate::{MediaResult, Tools};

/// The project's sound between `range` (the whole timeline by default), mixed as it exports
/// (volumes, fades, mutes, transitions, effects), as mono f32 samples at [`SPEECH_SAMPLE_RATE`].
/// Silence when nothing makes a sound.
pub async fn speech_samples(tools: &Tools, project: &Project, range: Option<(f64, f64)>) -> MediaResult<Vec<f32>> {
    let mut p = project.clone();
    // Whisper wants the words as they are: no loudness target, no limiter pumping on them.
    p.mixer.master.loudness = None;
    p.mixer.master.limiter = false;
    p.settings.sample_rate = SPEECH_SAMPLE_RATE;
    let end = project.duration();
    let span = range.map_or((0.0, end), |(a, b)| (a.max(0.0), b.min(end)));
    if span.1 - span.0 < 1e-3 {
        return Ok(vec![]);
    }
    let mix = crate::audio::render(tools, &p, &Range { span: Some(span), ..Default::default() }).await?;
    Ok(mix.into_iter().map(|[l, r]| (l + r) * 0.5).collect())
}
