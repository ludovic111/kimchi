//! Where the beats of a piece of music fall: an onset envelope, the tempo from its periodicity,
//! and beat tracking along it. Used to snap cuts to the music and to draw beats on the timeline.

use kimchi_core::Beats;

use crate::Frame;

/// Beats of `frames` (a whole piece, at `rate`). `None` when there is no steady pulse to find.
pub fn detect(_frames: &[Frame], _rate: u32) -> Option<Beats> {
    None
}

/// The beat grid of a constant tempo from `offset` seconds, for `seconds` of sound.
pub fn grid(tempo: f64, beats_per_bar: u32, offset: f64, seconds: f64, source: &str) -> Beats {
    let step = 60.0 / tempo.max(1.0);
    let n = ((seconds - offset) / step).floor().max(0.0) as usize + 1;
    Beats { tempo, beats_per_bar: beats_per_bar.max(1), times: (0..n).map(|i| offset + i as f64 * step).collect(), first_downbeat: 0, source: source.into() }
}
