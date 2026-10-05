//! Small building blocks of the mixer: ryolune's pan law, gains, delay lines and the true-peak
//! limiter at the end of the master.

use std::collections::VecDeque;

use crate::Frame;
use crate::loudness::{TRUE_PEAK_DELAY, TruePeak};

/// Left and right gains for `pan` (-1 left … 1 right), ryolune's law: the centre keeps both
/// sides at full level and a side fades out as the sound moves away from it (a square-root
/// balance, so a hard pan doesn't jump).
#[inline]
pub fn pan_gains(pan: f64) -> [f32; 2] {
    let p = pan.clamp(-1.0, 1.0) as f32;
    [(1.0 - p.max(0.0)).sqrt(), (1.0 + p.min(0.0)).sqrt()]
}

/// `into += from * gain`.
#[inline]
pub fn add(into: &mut [Frame], from: &[Frame], gain: f32) {
    for (a, b) in into.iter_mut().zip(from) {
        a[0] += b[0] * gain;
        a[1] += b[1] * gain;
    }
}

/// Peak absolute sample of a block.
pub fn peak(block: &[Frame]) -> f32 {
    block.iter().fold(0.0f32, |m, f| m.max(f[0].abs()).max(f[1].abs()))
}

/// Replaces anything that isn't a finite number (a misbehaving plugin) with silence.
pub fn sanitize(block: &mut [Frame]) {
    for f in block {
        for s in f {
            if !s.is_finite() {
                *s = 0.0;
            }
        }
    }
}

/// A fixed delay (plugin delay compensation).
#[derive(Debug, Clone, Default)]
pub struct DelayLine {
    frames: Vec<Frame>,
    at: usize,
}

impl DelayLine {
    pub fn new(len: usize) -> Self {
        Self { frames: vec![[0.0; 2]; len], at: 0 }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn process(&mut self, block: &mut [Frame]) {
        if self.frames.is_empty() {
            return;
        }
        for f in block {
            std::mem::swap(f, &mut self.frames[self.at]);
            self.at = (self.at + 1) % self.frames.len();
        }
    }

    pub fn clear(&mut self) {
        self.frames.fill([0.0; 2]);
    }

    /// Makes it `len` long, keeping what it holds when the length doesn't change.
    pub fn resize(&mut self, len: usize) {
        if len != self.frames.len() {
            *self = Self::new(len);
        }
    }
}

/// A true-peak limiter: looks ahead, so the gain is already down when a peak arrives, and
/// measures peaks between the samples (4x oversampled) so the output stays under `ceiling`
/// once converted to analog or encoded. Stereo-linked. Its delay never changes
/// ([`Limiter::latency`]), whether it limits or not.
pub struct Limiter {
    rate: u32,
    /// Lookahead window, in samples.
    window: usize,
    ceiling: f32,
    release: f32,
    enabled: bool,
    detector: TruePeak,
    /// Absolute values of the last TRUE_PEAK_DELAY + 2 input samples (to pair with the
    /// interpolator's output).
    recent: VecDeque<f32>,
    /// Input delayed so it meets its gain.
    delay: VecDeque<Frame>,
    /// Required gains of the window, and the running minimum (monotonic queue of (index, gain)).
    required: VecDeque<f32>,
    minima: VecDeque<(u64, f32)>,
    /// The last `window` minima and their sum (a moving average smooths the attack).
    smoothed: VecDeque<f32>,
    sum: f64,
    gain: f32,
    count: u64,
    /// The lowest gain since it was last read (meters).
    reduction: f32,
}

impl Limiter {
    pub fn new(rate: u32, ceiling_db: f64) -> Self {
        let window = ((rate as f64 * 0.0015).round() as usize).max(8);
        let mut l = Self {
            rate,
            window,
            ceiling: 1.0,
            release: 0.0,
            enabled: true,
            detector: TruePeak::new(),
            recent: VecDeque::new(),
            delay: VecDeque::new(),
            required: VecDeque::new(),
            minima: VecDeque::new(),
            smoothed: VecDeque::new(),
            sum: 0.0,
            gain: 1.0,
            count: 0,
            reduction: 1.0,
        };
        l.set_ceiling(ceiling_db);
        // A release of about 80 ms: quick enough not to pump on speech, slow enough not to
        // distort low notes.
        l.release = 1.0 - (-1.0 / (0.08 * rate as f64)).exp() as f32;
        l.reset();
        l
    }

    /// Ceiling in dBTP. A hair under it, so the reconstructed peak lands at or below.
    pub fn set_ceiling(&mut self, db: f64) {
        self.ceiling = kimchi_core::audio::db_to_gain(db.clamp(-24.0, 0.0) - 0.05) as f32;
    }

    /// Off: the sound passes untouched, with the same delay.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Frames between a sample going in and coming out.
    pub fn latency(&self) -> usize {
        TRUE_PEAK_DELAY + self.window - 1
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The deepest gain reduction since the last call, in dB (0 or negative).
    pub fn take_reduction_db(&mut self) -> f64 {
        let r = self.reduction;
        self.reduction = 1.0;
        kimchi_core::audio::gain_to_db(r as f64).min(0.0)
    }

    pub fn reset(&mut self) {
        self.detector.reset();
        self.recent = std::iter::repeat_n(0.0, TRUE_PEAK_DELAY + 2).collect();
        self.delay = std::iter::repeat_n([0.0; 2], self.latency()).collect();
        self.required = VecDeque::new();
        self.minima.clear();
        self.smoothed = std::iter::repeat_n(1.0, self.window).collect();
        self.sum = self.window as f64;
        self.gain = 1.0;
        self.count = 0;
    }

    pub fn process(&mut self, block: &mut [Frame]) {
        for f in block {
            let x = *f;
            // Peak around the sample TRUE_PEAK_DELAY frames back: both neighbours and what the
            // interpolator finds between them.
            let tp = self.detector.push(x);
            self.recent.pop_front();
            self.recent.push_back(x[0].abs().max(x[1].abs()));
            let n = self.recent.len();
            let peak = tp.max(self.recent[n - 1 - TRUE_PEAK_DELAY]).max(self.recent[n - TRUE_PEAK_DELAY]);
            let need = if self.enabled && peak > self.ceiling { self.ceiling / peak } else { 1.0 };
            // Sliding minimum over the window.
            let i = self.count;
            self.count += 1;
            while self.minima.back().is_some_and(|&(_, g)| g >= need) {
                self.minima.pop_back();
            }
            self.minima.push_back((i, need));
            while self.minima.front().is_some_and(|&(j, _)| j + self.window as u64 <= i) {
                self.minima.pop_front();
            }
            let min = self.minima.front().map_or(1.0, |m| m.1);
            // Moving average of the minima: falls smoothly, and is under every required gain
            // of the window when its peak comes out.
            self.sum += min as f64 - self.smoothed.pop_front().unwrap_or(1.0) as f64;
            self.smoothed.push_back(min);
            let target = (self.sum / self.window as f64) as f32;
            self.gain = if target < self.gain { target } else { self.gain + (target - self.gain) * self.release };
            self.gain = self.gain.min(1.0);
            self.reduction = self.reduction.min(self.gain);
            let out = self.delay.pop_front().unwrap_or([0.0; 2]);
            self.delay.push_back(x);
            *f = [out[0] * self.gain, out[1] * self.gain];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::measure;

    #[test]
    fn pan_law_keeps_the_centre_at_full_level() {
        assert_eq!(pan_gains(0.0), [1.0, 1.0]);
        assert_eq!(pan_gains(1.0), [0.0, 1.0]);
        assert_eq!(pan_gains(-1.0), [1.0, 0.0]);
        let [l, r] = pan_gains(0.5);
        assert!((l - 0.5f32.sqrt()).abs() < 1e-6 && r == 1.0);
    }

    #[test]
    fn delay_lines_delay() {
        let mut d = DelayLine::new(2);
        let mut b = [[1.0, 1.0], [2.0, 2.0], [3.0, 3.0]];
        d.process(&mut b);
        assert_eq!(b, [[0.0, 0.0], [0.0, 0.0], [1.0, 1.0]]);
    }

    #[test]
    fn the_limiter_keeps_true_peaks_under_its_ceiling() {
        let rate = 48_000;
        let mut lim = Limiter::new(rate, -1.0);
        // A loud sine with sudden bursts, at a frequency whose peaks fall between samples.
        let mut s: Vec<Frame> = (0..rate as usize)
            .map(|i| {
                let burst = if (i / 4000) % 2 == 1 { 3.0 } else { 0.6 };
                let v = (burst * (std::f64::consts::TAU * 11_025.0 * i as f64 / rate as f64 + 0.7).sin()) as f32;
                [v, -v * 0.8]
            })
            .collect();
        let quiet_in = s[100];
        lim.process(&mut s);
        let r = measure(&s, rate);
        assert!(r.true_peak <= -1.0 + 0.05, "{r:?}");
        // Quiet parts pass untouched, just later.
        let k = 100 + lim.latency();
        assert!((s[k][0] - quiet_in[0]).abs() < 1e-4, "{:?} vs {quiet_in:?}", s[k]);
        assert!(lim.take_reduction_db() < -6.0);
        // Turned off it only delays.
        let mut off = Limiter::new(rate, -1.0);
        off.set_enabled(false);
        let mut b = vec![[2.0f32, 2.0]; 200];
        off.process(&mut b);
        assert_eq!(b[off.latency()], [2.0, 2.0]);
        assert_eq!(b[off.latency() - 1], [0.0, 0.0]);
    }
}
