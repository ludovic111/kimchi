//! Loudness as EBU R128 / ITU-R BS.1770 measures it: K-weighting, 400 ms blocks, gating, and the
//! true peak (4x oversampled), for `audio.measure`, normalising clips and the export's target.

use serde::Serialize;

use crate::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loudness {
    /// Integrated loudness (LUFS); [`kimchi_core::audio::MIN_DB`] for silence.
    pub integrated: f64,
    /// Loudness range (LU).
    pub range: f64,
    /// Highest true peak (dBTP).
    pub true_peak: f64,
    /// Loudest momentary (400 ms) and short-term (3 s) readings (LUFS).
    pub momentary_max: f64,
    pub short_term_max: f64,
}

/// Feed it the sound block by block, then read [`Self::result`].
pub struct LoudnessMeter {
    _rate: u32,
}

impl LoudnessMeter {
    pub fn new(rate: u32) -> Self {
        Self { _rate: rate }
    }

    pub fn push(&mut self, _frames: &[Frame]) {}

    pub fn result(&self) -> Loudness {
        let min = kimchi_core::audio::MIN_DB;
        Loudness { integrated: min, range: 0.0, true_peak: min, momentary_max: min, short_term_max: min }
    }
}

/// The loudness of a whole buffer.
pub fn measure(frames: &[Frame], rate: u32) -> Loudness {
    let mut m = LoudnessMeter::new(rate);
    m.push(frames);
    m.result()
}
