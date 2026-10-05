//! Loudness as EBU R128 / ITU-R BS.1770-4 measures it: K-weighting, 400 ms blocks, gating, and the
//! true peak (4x oversampled), for `audio.measure`, normalising clips and the export's target.
//!
//! - K-weighting: the standard's two filters (a high shelf around 1.7 kHz, a high-pass around
//!   38 Hz), designed from their analog prototypes so they are right at any sample rate (at 48 kHz
//!   they are the coefficients printed in BS.1770).
//! - Momentary loudness: 400 ms blocks every 100 ms (75 % overlap); short-term: 3 s windows.
//! - Integrated loudness: blocks above the absolute gate (-70 LUFS), then above the relative gate
//!   (10 LU under the loudness of those).
//! - Loudness range (EBU Tech 3342): short-term readings gated at -70 LUFS and 20 LU under their
//!   loudness; the spread between the 10th and the 95th percentiles.
//! - True peak: 4x oversampling with the polyphase interpolator of BS.1770-4 Annex 2.

use std::collections::VecDeque;

use serde::Serialize;

use crate::Frame;

const MIN: f64 = kimchi_core::audio::MIN_DB;

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

/// One second-order section (transposed direct form II, in f64 so long runs stay exact).
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages for `rate` (as libebur128 derives them).
fn k_weighting(rate: u32) -> [Biquad; 2] {
    let rate = rate as f64;
    // Stage 1: the head's acoustic effect, a high shelf of about +4 dB.
    let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    // Stage 2: the RLB high-pass.
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    [shelf, high_pass]
}

/// The 4x interpolator of BS.1770-4 Annex 2: 48 taps as four phases of 12.
const TRUE_PEAK_TAPS: [[f64; 12]; 4] = [
    [
        0.0017089843750, 0.0109863281250, -0.0196533203125, 0.0332031250000, -0.0594482421875, 0.1373291015625,
        0.9721679687500, -0.1022949218750, 0.0476074218750, -0.0266113281250, 0.0148925781250, -0.0083007812500,
    ],
    [
        -0.0291748046875, 0.0292968750000, -0.0517578125000, 0.0891113281250, -0.1665039062500, 0.4650878906250,
        0.7797851562500, -0.2003173828125, 0.1015625000000, -0.0582275390625, 0.0330810546875, -0.0189208984375,
    ],
    [
        -0.0189208984375, 0.0330810546875, -0.0582275390625, 0.1015625000000, -0.2003173828125, 0.7797851562500,
        0.4650878906250, -0.1665039062500, 0.0891113281250, -0.0517578125000, 0.0292968750000, -0.0291748046875,
    ],
    [
        -0.0083007812500, 0.0148925781250, -0.0266113281250, 0.0476074218750, -0.1022949218750, 0.9721679687500,
        0.1373291015625, -0.0594482421875, 0.0332031250000, -0.0196533203125, 0.0109863281250, 0.0017089843750,
    ],
];

/// Samples the true-peak interpolator lags behind its input: what it reports for sample `n`
/// lies between samples `n - TRUE_PEAK_DELAY` and `n - TRUE_PEAK_DELAY + 1`.
pub const TRUE_PEAK_DELAY: usize = 6;

/// The true peak of a stereo stream, sample by sample (4x oversampled).
#[derive(Debug, Clone, Default)]
pub struct TruePeak {
    /// The last 12 samples per channel, written twice (at `at` and `at + 12`) so the newest
    /// twelve are always `history[at..at + 12]`, newest first, without wrapping.
    history: [[f64; 24]; 2],
    at: usize,
}

impl TruePeak {
    pub fn new() -> Self {
        Self::default()
    }

    /// The highest absolute value of the reconstructed signal between the sample
    /// [`TRUE_PEAK_DELAY`] frames ago and the next one, over both channels.
    #[inline]
    pub fn push(&mut self, frame: Frame) -> f32 {
        self.at = if self.at == 0 { 11 } else { self.at - 1 };
        let mut peak = 0.0f64;
        for (c, &x) in frame.iter().enumerate() {
            let h = &mut self.history[c];
            let x = if x.is_finite() { x as f64 } else { 0.0 };
            h[self.at] = x;
            h[self.at + 12] = x;
            let recent: &[f64; 12] = h[self.at..self.at + 12].try_into().expect("twelve");
            for taps in &TRUE_PEAK_TAPS {
                let acc: f64 = taps.iter().zip(recent).map(|(t, v)| t * v).sum();
                peak = peak.max(acc.abs());
            }
        }
        peak as f32
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Loudness of a mean-square energy (already K-weighted and summed over the channels).
fn lufs(energy: f64) -> f64 {
    if energy <= 1e-15 { MIN } else { (-0.691 + 10.0 * energy.log10()).max(MIN) }
}

/// Feed it the sound block by block, then read [`Self::result`]. Also gives the momentary and
/// short-term loudness as it goes, for the meters.
pub struct LoudnessMeter {
    rate: u32,
    filters: [[Biquad; 2]; 2],
    /// Samples in 100 ms, and the weighted energy of the 100 ms being filled.
    step: usize,
    sum: f64,
    filled: usize,
    /// Energies of the last 30 complete 100 ms segments (3 s), newest last.
    recent: VecDeque<f64>,
    /// Every 400 ms block's energy (gating) and every 3 s window's (loudness range).
    blocks: Vec<f64>,
    windows: Vec<f64>,
    momentary_max: f64,
    short_term_max: f64,
    peak: TruePeak,
    true_peak: f32,
}

impl LoudnessMeter {
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1000);
        let k = k_weighting(rate);
        Self {
            rate,
            filters: [k, k],
            step: ((rate as f64 / 10.0).round() as usize).max(1),
            sum: 0.0,
            filled: 0,
            recent: VecDeque::with_capacity(31),
            blocks: vec![],
            windows: vec![],
            momentary_max: MIN,
            short_term_max: MIN,
            peak: TruePeak::new(),
            true_peak: 0.0,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&mut self, frames: &[Frame]) {
        for &frame in frames {
            self.true_peak = self.true_peak.max(self.peak.push(frame));
            let mut e = 0.0;
            for (c, &x) in frame.iter().enumerate() {
                let x = if x.is_finite() { x as f64 } else { 0.0 };
                let [a, b] = &mut self.filters[c];
                let y = b.run(a.run(x));
                e += y * y;
            }
            self.sum += e;
            self.filled += 1;
            if self.filled == self.step {
                self.segment();
            }
        }
    }

    /// A 100 ms segment is complete: a new momentary block (once there are four) and a new
    /// short-term window (once there are thirty).
    fn segment(&mut self) {
        let e = self.sum / self.step as f64;
        self.sum = 0.0;
        self.filled = 0;
        if self.recent.len() == 30 {
            self.recent.pop_front();
        }
        self.recent.push_back(e);
        if self.recent.len() >= 4 {
            let block = self.recent.iter().rev().take(4).sum::<f64>() / 4.0;
            self.blocks.push(block);
            self.momentary_max = self.momentary_max.max(lufs(block));
        }
        if self.recent.len() == 30 {
            let window = self.recent.iter().sum::<f64>() / 30.0;
            self.windows.push(window);
            self.short_term_max = self.short_term_max.max(lufs(window));
        }
    }

    /// Loudness of the last 400 ms (LUFS).
    pub fn momentary(&self) -> f64 {
        let n = self.recent.len().min(4);
        if n == 0 {
            return MIN;
        }
        lufs(self.recent.iter().rev().take(n).sum::<f64>() / 4.0)
    }

    /// Loudness of the last 3 s (LUFS).
    pub fn short_term(&self) -> f64 {
        if self.recent.is_empty() {
            return MIN;
        }
        lufs(self.recent.iter().sum::<f64>() / 30.0)
    }

    /// Integrated (gated) loudness so far.
    pub fn integrated(&self) -> f64 {
        let above: Vec<f64> = self.blocks.iter().copied().filter(|&e| lufs(e) > -70.0).collect();
        if above.is_empty() {
            return MIN;
        }
        let gate = lufs(above.iter().sum::<f64>() / above.len() as f64) - 10.0;
        let kept: Vec<f64> = above.into_iter().filter(|&e| lufs(e) > gate).collect();
        if kept.is_empty() {
            return MIN;
        }
        lufs(kept.iter().sum::<f64>() / kept.len() as f64)
    }

    /// Loudness range so far (EBU Tech 3342).
    pub fn range(&self) -> f64 {
        let above: Vec<f64> = self.windows.iter().copied().filter(|&e| lufs(e) > -70.0).collect();
        if above.is_empty() {
            return 0.0;
        }
        let gate = lufs(above.iter().sum::<f64>() / above.len() as f64) - 20.0;
        let mut kept: Vec<f64> = above.into_iter().map(lufs).filter(|&l| l > gate).collect();
        if kept.len() < 2 {
            return 0.0;
        }
        kept.sort_by(f64::total_cmp);
        let at = |p: f64| {
            let x = p * (kept.len() - 1) as f64;
            let (i, f) = (x.floor() as usize, x.fract());
            kept[i] + (kept[(i + 1).min(kept.len() - 1)] - kept[i]) * f
        };
        (at(0.95) - at(0.10)).max(0.0)
    }

    /// Highest true peak so far (dBTP).
    pub fn true_peak(&self) -> f64 {
        kimchi_core::audio::gain_to_db(self.true_peak as f64)
    }

    pub fn result(&self) -> Loudness {
        Loudness {
            integrated: self.integrated(),
            range: self.range(),
            true_peak: self.true_peak(),
            momentary_max: self.momentary_max,
            short_term_max: self.short_term_max,
        }
    }

    /// Starts over (a new play or a seek).
    pub fn reset(&mut self) {
        *self = Self::new(self.rate);
    }
}

/// The loudness of a whole buffer.
pub fn measure(frames: &[Frame], rate: u32) -> Loudness {
    let mut m = LoudnessMeter::new(rate);
    m.push(frames);
    m.result()
}

/// Gain (dB) that brings `measured` to `target` LUFS; 0 for silence.
pub fn gain_to(measured: &Loudness, target: f64) -> f64 {
    if measured.integrated <= MIN + 1e-9 { 0.0 } else { target - measured.integrated }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 kHz sine at `dbfs` (peak) on both channels, `seconds` long.
    fn sine(dbfs: f64, seconds: f64, rate: u32, hz: f64) -> Vec<Frame> {
        let a = 10f64.powf(dbfs / 20.0);
        (0..(seconds * rate as f64) as usize)
            .map(|i| {
                let v = (a * (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin()) as f32;
                [v, v]
            })
            .collect()
    }

    // EBU Tech 3341, test signals 1 to 5 (and Tech 3342 for the range): stereo 1 kHz sines.
    #[test]
    fn tech_3341_minimum_requirements() {
        for rate in [44_100, 48_000, 96_000] {
            // 1: -23 dBFS for 20 s reads -23.0 ±0.1 (momentary, short-term and integrated).
            let r = measure(&sine(-23.0, 20.0, rate, 1000.0), rate);
            assert!((r.integrated + 23.0).abs() < 0.1, "{rate}: {r:?}");
            assert!((r.momentary_max + 23.0).abs() < 0.1, "{rate}: {r:?}");
            assert!((r.short_term_max + 23.0).abs() < 0.1, "{rate}: {r:?}");
            // 2: -33 dBFS reads -33.0.
            let r = measure(&sine(-33.0, 20.0, rate, 1000.0), rate);
            assert!((r.integrated + 33.0).abs() < 0.1, "{rate}: {r:?}");
        }
        let rate = 48_000;
        // 3: -36 / -23 / -36 dBFS for 10 / 60 / 10 s: the relative gate leaves -23.
        let mut s = sine(-36.0, 10.0, rate, 1000.0);
        s.extend(sine(-23.0, 60.0, rate, 1000.0));
        s.extend(sine(-36.0, 10.0, rate, 1000.0));
        assert!((measure(&s, rate).integrated + 23.0).abs() < 0.1);
        // 4: -72 / -36 / -23 / -36 / -72 dBFS: the absolute gate drops the -72 dB parts.
        let mut s = sine(-72.0, 10.0, rate, 1000.0);
        s.extend(sine(-36.0, 10.0, rate, 1000.0));
        s.extend(sine(-23.0, 60.0, rate, 1000.0));
        s.extend(sine(-36.0, 10.0, rate, 1000.0));
        s.extend(sine(-72.0, 10.0, rate, 1000.0));
        assert!((measure(&s, rate).integrated + 23.0).abs() < 0.1);
        // 5: -26 / -20 / -26 dBFS for 20 / 20.1 / 20 s reads -23.0.
        let mut s = sine(-26.0, 20.0, rate, 1000.0);
        s.extend(sine(-20.0, 20.1, rate, 1000.0));
        s.extend(sine(-26.0, 20.0, rate, 1000.0));
        assert!((measure(&s, rate).integrated + 23.0).abs() < 0.1);
        // Tech 3342 test 1: -20 then -30 dBFS, 20 s each: a range of 10 LU ±1.
        let mut s = sine(-20.0, 20.0, rate, 1000.0);
        s.extend(sine(-30.0, 20.0, rate, 1000.0));
        let r = measure(&s, rate);
        assert!((r.range - 10.0).abs() < 1.0, "{r:?}");
        // Silence is the floor, not a NaN.
        let r = measure(&vec![[0.0; 2]; 48_000], rate);
        assert_eq!(r.integrated, MIN);
        assert_eq!(r.range, 0.0);
    }

    #[test]
    fn true_peak_finds_what_falls_between_samples() {
        let rate = 48_000;
        // A quarter of the rate, 45° out of phase: every sample is at 0.707 of the real peak.
        let s: Vec<Frame> = (0..4800)
            .map(|i| {
                let v = (0.5 * (std::f64::consts::FRAC_PI_2 * i as f64 + std::f64::consts::FRAC_PI_4).sin()) as f32;
                [v, v]
            })
            .collect();
        let sample_peak = s.iter().map(|f| f[0].abs()).fold(0.0, f32::max) as f64;
        let r = measure(&s, rate);
        assert!((kimchi_core::audio::gain_to_db(sample_peak) + 9.03).abs() < 0.1);
        // Tech 3341 allows +0.2 / -0.4 dB around the true value (-6.02 dBTP).
        assert!(r.true_peak > -6.02 - 0.4 && r.true_peak < -6.02 + 0.2, "{r:?}");
        // A 1 kHz sine at 0 dBFS reads about 0 dBTP.
        let r = measure(&sine(0.0, 1.0, rate, 997.0), rate);
        assert!(r.true_peak.abs() < 0.3, "{r:?}");
    }

    #[test]
    fn meters_follow_the_last_400_ms_and_3_s() {
        let rate = 48_000;
        let mut m = LoudnessMeter::new(rate);
        m.push(&sine(-23.0, 4.0, rate, 1000.0));
        assert!((m.momentary() + 23.0).abs() < 0.1);
        assert!((m.short_term() + 23.0).abs() < 0.1);
        m.push(&vec![[0.0; 2]; rate as usize]);
        assert_eq!(m.momentary(), MIN);
        // Two of the last three seconds still sound: 10·log10(2/3) under.
        assert!((m.short_term() + 23.0 + 1.76).abs() < 0.1, "{}", m.short_term());
        let r = m.result();
        assert!((r.integrated + 23.0).abs() < 0.3, "{r:?}");
        assert!((gain_to(&r, -14.0) - (-14.0 - r.integrated)).abs() < 1e-9);
        assert_eq!(gain_to(&measure(&[[0.0; 2]; 100], rate), -14.0), 0.0);
    }
}
