//! Where the beats of a piece of music fall: an onset envelope, the tempo from its periodicity,
//! and beat tracking along it. Used to snap cuts to the music and to draw beats on the timeline.
//!
//! 1. Onsets: the sound is folded to mono and brought down to about 12 kHz; the spectral flux
//!    (how much each band of a short-time spectrum grows from one 10 ms step to the next, on a
//!    log scale) peaks where notes and hits start. A moving average is taken off, so a loud
//!    passage doesn't count as an onset by itself.
//! 2. Tempo: the envelope's autocorrelation, read as a comb (a period, its multiples and its
//!    halves together) between 50 and 220 beats per minute, with a soft preference for the
//!    usual range (a log-normal around 120) to settle octave doubts the way listeners do.
//! 3. Beats: dynamic programming (Ellis 2007) finds the train of onsets that best fits the tempo
//!    while letting it drift, so a rubato or a gradual change is followed.
//! 4. Downbeats: of the `beats_per_bar` ways to group the beats, the one whose first beats carry
//!    the most low-frequency onset (kicks and bass notes land on the one).

use kimchi_core::Beats;

use crate::Frame;

/// Steps of the onset envelope per second (about 10 ms).
const STEP_SECONDS: f64 = 0.0107;
const FFT: usize = 1024;
const MIN_BPM: f64 = 50.0;
const MAX_BPM: f64 = 220.0;

/// Beats of `frames` (a whole piece, at `rate`). `None` when there is no steady pulse to find.
pub fn detect(frames: &[Frame], rate: u32) -> Option<Beats> {
    detect_with(frames, rate, 4)
}

/// [`detect`] for music counted in `beats_per_bar`.
pub fn detect_with(frames: &[Frame], rate: u32, beats_per_bar: u32) -> Option<Beats> {
    let env = Envelope::of(frames, rate)?;
    if env.flux.len() < (4.0 / env.step) as usize {
        return None; // under four seconds: no tempo to speak of
    }
    let mut period = tempo(&env.flux, env.step)?;
    let mut beats = track(&env.flux, period);
    // Every other beat much weaker (hats or a shaker between the beats): those were
    // subdivisions, the beat is half as fast.
    if 60.0 / (2.0 * period * env.step) >= MIN_BPM && beats.len() >= 8 {
        // Loudness of the hits, not their (compressed) spectral change.
        let at = |i: &usize| (i.saturating_sub(1)..=*i + 1).filter_map(|k| env.strength.get(k)).copied().fold(0.0, f64::max);
        let even: f64 = beats.iter().step_by(2).map(at).sum::<f64>() / beats.len().div_ceil(2) as f64;
        let odd: f64 = beats.iter().skip(1).step_by(2).map(at).sum::<f64>() / (beats.len() / 2) as f64;
        if even.min(odd) < 0.45 * even.max(odd) {
            period *= 2.0;
            beats = track(&env.flux, period);
        }
    }
    if beats.len() < 4 {
        return None;
    }
    // Music that speeds up or slows down: track again with the tempo of each stretch.
    let local = local_periods(&env.flux, period, env.step);
    if local.iter().any(|p| (p / period - 1.0).abs() > 0.02) {
        beats = track_with(&env.flux, |t| local[t]);
    }
    let times: Vec<f64> = beats.iter().map(|&i| i as f64 * env.step + env.offset).collect();
    let span = times[times.len() - 1] - times[0];
    let tempo = 60.0 * (times.len() - 1) as f64 / span.max(1e-9);
    let bpb = beats_per_bar.max(1);
    let first_downbeat = downbeat(&env.low, &beats, bpb as usize);
    Some(Beats { tempo, beats_per_bar: bpb, times, first_downbeat, source: "detected".into() })
}

/// The beat grid of a constant tempo from `offset` seconds, for `seconds` of sound.
pub fn grid(tempo: f64, beats_per_bar: u32, offset: f64, seconds: f64, source: &str) -> Beats {
    let step = 60.0 / tempo.max(1.0);
    let n = ((seconds - offset) / step).floor().max(0.0) as usize + 1;
    Beats { tempo, beats_per_bar: beats_per_bar.max(1), times: (0..n).map(|i| offset + i as f64 * step).collect(), first_downbeat: 0, source: source.into() }
}

/// The onset envelope: all bands on a log scale (finding onsets), and on a linear one (how
/// loud each hit is), and the low bands (under ~200 Hz) for downbeats.
struct Envelope {
    flux: Vec<f64>,
    strength: Vec<f64>,
    low: Vec<f64>,
    /// Seconds per value.
    step: f64,
    /// Seconds from the start of a value's spectrum to the onset it shows (an onset counts as
    /// it enters the window).
    offset: f64,
}

impl Envelope {
    fn of(frames: &[Frame], rate: u32) -> Option<Self> {
        if frames.is_empty() || rate < 4_000 {
            return None;
        }
        // Mono, low-passed and decimated to about 12 kHz (a 4th-order low-pass at 5 kHz).
        let factor = ((rate as f64 / 12_000.0).round() as usize).max(1);
        let sr = rate as f64 / factor as f64;
        let mut lp = [Lowpass::new(5_000.0_f64.min(sr * 0.45), rate as f64), Lowpass::new(5_000.0_f64.min(sr * 0.45), rate as f64)];
        let mut mono = Vec::with_capacity(frames.len() / factor + 1);
        for (i, f) in frames.iter().enumerate() {
            let x = 0.5 * (f[0] as f64 + f[1] as f64);
            let first = lp[0].run(if x.is_finite() { x } else { 0.0 });
            let y = lp[1].run(first);
            if i % factor == 0 {
                mono.push(y);
            }
        }
        let hop = ((STEP_SECONDS * sr).round() as usize).max(1);
        let step = hop as f64 / sr;
        let window: Vec<f64> = (0..FFT).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / FFT as f64).cos()).collect();
        let bins = FFT / 2;
        let low_bins = ((200.0 / sr * FFT as f64).ceil() as usize).clamp(2, bins);
        let mut previous = vec![0.0f64; bins];
        let mut previous_mag = vec![0.0f64; bins];
        let mut buf = vec![(0.0f64, 0.0f64); FFT];
        let (mut flux, mut strength, mut low) = (vec![], vec![], vec![]);
        let mut magnitude = 0.0;
        let mut start = 0;
        while start + FFT <= mono.len().max(FFT) {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = (mono.get(start + i).copied().unwrap_or(0.0) * window[i], 0.0);
            }
            fft(&mut buf);
            let (mut all, mut loud, mut lows) = (0.0, 0.0, 0.0);
            for k in 1..bins {
                let mag = (buf[k].0 * buf[k].0 + buf[k].1 * buf[k].1).sqrt();
                let m = (1.0 + 1000.0 * mag).ln();
                all += (m - previous[k]).max(0.0);
                let rise = (mag - previous_mag[k]).max(0.0);
                loud += rise;
                if k < low_bins {
                    lows += rise;
                }
                magnitude += mag;
                previous[k] = m;
                previous_mag[k] = mag;
            }
            flux.push(all);
            strength.push(loud);
            low.push(lows);
            start += hop;
            if start + FFT > mono.len() {
                break;
            }
        }
        // The first value compares against silence: not an onset.
        flux[0] = 0.0;
        strength[0] = 0.0;
        low[0] = 0.0;
        // Onsets must stand out of the sound itself: a held note or a drone has none.
        if strength.iter().sum::<f64>() < 0.01 * magnitude {
            return None;
        }
        let window = (0.4 / step) as usize;
        Some(Self { flux: whiten(&flux, window), strength, low, step, offset: FFT as f64 / sr - step })
    }
}

/// Takes a moving average off, keeps what rises above it, scales to unit deviation.
fn whiten(x: &[f64], window: usize) -> Vec<f64> {
    let n = x.len();
    let half = window.max(1) / 2;
    let mut prefix = vec![0.0; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + x[i];
    }
    let mut out: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(n));
            (x[i] - (prefix[b] - prefix[a]) / (b - a) as f64).max(0.0)
        })
        .collect();
    let mean = out.iter().sum::<f64>() / n.max(1) as f64;
    let sd = (out.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n.max(1) as f64).sqrt();
    if sd > 1e-12 {
        out.iter_mut().for_each(|v| *v /= sd);
    }
    out
}

/// The beat period (in envelope steps, fractional) of the onset envelope.
fn tempo(env: &[f64], step: f64) -> Option<f64> {
    let to_lag = |bpm: f64| 60.0 / bpm / step;
    let (lo, hi) = (to_lag(MAX_BPM).floor() as usize, to_lag(MIN_BPM).ceil() as usize);
    let max_lag = (4 * hi).min(env.len() / 2);
    if max_lag <= lo + 2 {
        return None;
    }
    // Autocorrelation of the envelope less its mean (so a steady texture scores nothing),
    // normalised by the overlap.
    let n = env.len();
    let mean = env.iter().sum::<f64>() / n as f64;
    let centred: Vec<f64> = env.iter().map(|v| v - mean).collect();
    let acf: Vec<f64> = (0..=max_lag).map(|l| centred[..n - l].iter().zip(&centred[l..]).map(|(a, b)| a * b).sum::<f64>() / (n - l) as f64).collect();
    if acf[0] <= 1e-12 {
        return None;
    }
    let at = |l: f64| -> f64 {
        let i = l.floor() as usize;
        if i + 1 >= acf.len() {
            return 0.0;
        }
        let f = l - i as f64;
        acf[i] * (1.0 - f) + acf[i + 1] * f
    };
    let score = |l: f64| -> f64 {
        // The period, its multiples and its half: a comb that prefers the fundamental.
        let comb = at(l) + 0.5 * at(2.0 * l) + 0.33 * at(3.0 * l) + 0.25 * at(4.0 * l) + 0.25 * at(l / 2.0);
        let bpm = 60.0 / (l * step);
        let octaves = (bpm / 120.0).log2();
        comb * (-0.5 * (octaves / 0.9).powi(2)).exp()
    };
    let (mut best, mut best_score) = (0.0, f64::MIN);
    let mut l = lo as f64;
    while l <= hi as f64 {
        let s = score(l);
        if s > best_score {
            (best, best_score) = (l, s);
        }
        l += 0.25;
    }
    // A pulse at all: its periodic part stands out from the envelope's own energy.
    if best_score <= 0.1 * acf[0] {
        return None;
    }
    // Finer around the peak.
    let mut l = best - 0.25;
    while l <= best + 0.25 {
        let s = score(l);
        if s > best_score {
            (best, best_score) = (l, s);
        }
        l += 0.01;
    }
    Some(best)
}

/// The beat period at every step: the envelope's autocorrelation over eight seconds around it,
/// read within a quarter of the whole piece's `period`, smoothed.
fn local_periods(env: &[f64], period: f64, step: f64) -> Vec<f64> {
    let n = env.len();
    let half = (4.0 / step) as usize;
    let every = (1.0 / step).max(1.0) as usize;
    let (lo, hi) = (period / 1.25, period * 1.25);
    let mut points: Vec<(usize, f64)> = vec![];
    let mut centre = 0;
    while centre < n {
        let (a, b) = (centre.saturating_sub(half), (centre + half).min(n));
        let w = &env[a..b];
        if w.len() as f64 > 3.0 * hi {
            let mean = w.iter().sum::<f64>() / w.len() as f64;
            let acf = |lag: f64| {
                let l = lag.round() as usize;
                let comb = |l: usize| w[..w.len() - l].iter().zip(&w[l..]).map(|(x, y)| (x - mean) * (y - mean)).sum::<f64>() / (w.len() - l) as f64;
                comb(l) + 0.5 * if 2 * l < w.len() { comb(2 * l) } else { 0.0 }
            };
            let mut best = (period, f64::MIN);
            let mut lag = lo;
            while lag <= hi {
                let s = acf(lag);
                if s > best.1 {
                    best = (lag, s);
                }
                lag += 0.25;
            }
            points.push((centre, best.0));
        }
        centre += every;
    }
    if points.is_empty() {
        return vec![period; n];
    }
    // A median over five readings, then a line between them.
    let smooth: Vec<(usize, f64)> = (0..points.len())
        .map(|i| {
            let mut near: Vec<f64> = points[i.saturating_sub(2)..(i + 3).min(points.len())].iter().map(|p| p.1).collect();
            near.sort_by(f64::total_cmp);
            (points[i].0, near[near.len() / 2])
        })
        .collect();
    (0..n)
        .map(|t| {
            let k = smooth.partition_point(|p| p.0 <= t);
            match (k.checked_sub(1).map(|i| smooth[i]), smooth.get(k)) {
                (Some(a), Some(b)) => a.1 + (b.1 - a.1) * (t - a.0) as f64 / (b.0 - a.0).max(1) as f64,
                (Some(a), None) => a.1,
                (None, Some(b)) => b.1,
                (None, None) => period,
            }
        })
        .collect()
}

/// Dynamic-programming beat tracking along `env` at about `period` steps a beat; the indexes of
/// the beats.
fn track(env: &[f64], period: f64) -> Vec<usize> {
    track_with(env, |_| period)
}

/// [`track`] with a period that may change along the way.
fn track_with(env: &[f64], period_at: impl Fn(usize) -> f64) -> Vec<usize> {
    let n = env.len();
    let tightness = 100.0;
    let mut score = vec![0.0f64; n];
    let mut from = vec![usize::MAX; n];
    for t in 0..n {
        let period = period_at(t);
        let (near, far) = ((period * 0.5).round() as usize, (period * 2.0).round() as usize);
        let mut best = 0.0;
        let mut arg = usize::MAX;
        if t >= near.max(1) {
            for (prev, previous_score) in score.iter().enumerate().take(t - near.max(1) + 1).skip(t.saturating_sub(far)) {
                let gap = (t - prev) as f64;
                let penalty = tightness * (gap / period).ln().powi(2);
                let s = previous_score - penalty;
                if arg == usize::MAX || s > best {
                    (best, arg) = (s, prev);
                }
            }
        }
        score[t] = env[t] + best.max(0.0);
        from[t] = if best > 0.0 { arg } else { usize::MAX };
    }
    // End on the best-scoring step of the last beat period.
    let tail = (n.saturating_sub(period_at(n.saturating_sub(1)).ceil() as usize))..n;
    let Some(mut t) = tail.max_by(|&a, &b| score[a].total_cmp(&score[b])) else { return vec![] };
    let mut beats = vec![t];
    while from[t] != usize::MAX {
        t = from[t];
        beats.push(t);
    }
    beats.reverse();
    // Leading steps of silence before the music don't make beats.
    let threshold = 0.1;
    while beats.len() > 1 && env[beats[0]] < threshold && env.get(beats[1]).is_some_and(|v| *v >= threshold) {
        beats.remove(0);
    }
    beats
}

/// Index (0..beats_per_bar) of the first beat that starts a bar: the grouping whose first beats
/// carry the most low-frequency onset.
fn downbeat(low: &[f64], beats: &[usize], bpb: usize) -> usize {
    if bpb <= 1 {
        return 0;
    }
    let strength = |i: usize| {
        // Around the beat (a step either side: the tracker may sit a step off).
        let b = beats[i];
        (b.saturating_sub(1)..=(b + 1).min(low.len().saturating_sub(1))).map(|k| low[k]).fold(0.0, f64::max)
    };
    (0..bpb.min(beats.len()))
        .max_by(|&a, &b| {
            let sum = |phase: usize| (phase..beats.len()).step_by(bpb).map(strength).sum::<f64>() / beats.len().div_ceil(bpb) as f64;
            sum(a).total_cmp(&sum(b))
        })
        .unwrap_or(0)
}

/// A second-order Butterworth low-pass.
#[derive(Clone, Copy)]
struct Lowpass {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Lowpass {
    fn new(cutoff: f64, rate: f64) -> Self {
        let k = (std::f64::consts::PI * cutoff / rate).tan();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let norm = 1.0 / (1.0 + k / q + k * k);
        let b0 = k * k * norm;
        Self { b: [b0, 2.0 * b0, b0], a: [2.0 * (k * k - 1.0) * norm, (1.0 - k / q + k * k) * norm], z: [0.0; 2] }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// In-place radix-2 FFT of (re, im) pairs; the length is a power of two.
fn fft(x: &mut [(f64, f64)]) {
    let n = x.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            x.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f64::consts::TAU / len as f64;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ar, ai) = x[start + k];
                let (br, bi) = x[start + k + len / 2];
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                x[start + k] = (ar + tr, ai + ti);
                x[start + k + len / 2] = (ar - tr, ai - ti);
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A drum pattern: kicks on the first beat of each bar, snares (noise) on the others, and
    /// quieter hats on the off-beats, `swing` delaying them (0.5 = straight, 0.66 = triplet
    /// swing). `bpm(t)` is the tempo at time t. Starts on a downbeat after `lead` seconds.
    fn pattern(seconds: f64, bpm: impl Fn(f64) -> f64, swing: f64, lead: f64) -> (Vec<Frame>, Vec<f64>) {
        let n = (seconds * RATE as f64) as usize;
        let mut out = vec![[0.0f32; 2]; n];
        let mut seed = 0x1234_5678u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / 8_388_608.0 - 1.0
        };
        let mut beats = vec![];
        let mut t = lead;
        let mut k = 0;
        while t < seconds - 0.5 {
            beats.push(t);
            let period = 60.0 / bpm(t);
            let hit = |out: &mut Vec<Frame>, at: f64, kind: u8, noise: &mut dyn FnMut() -> f32| {
                let start = (at * RATE as f64) as usize;
                for i in 0..(0.12 * RATE as f64) as usize {
                    let Some(f) = out.get_mut(start + i) else { break };
                    let x = i as f32 / RATE as f32;
                    let v = match kind {
                        0 => 0.9 * (std::f32::consts::TAU * (55.0 + 90.0 * (-x * 30.0).exp()) * x).sin() * (-x * 18.0).exp(),
                        1 => 0.5 * noise() * (-x * 35.0).exp(),
                        _ => 0.15 * noise() * (-x * 90.0).exp(),
                    };
                    f[0] += v;
                    f[1] += v;
                }
            };
            hit(&mut out, t, if k % 4 == 0 { 0 } else { 1 }, &mut noise);
            hit(&mut out, t + swing * period, 2, &mut noise);
            t += period;
            k += 1;
        }
        (out, beats)
    }

    fn check(found: &Beats, beats: &[f64], tempo: f64) {
        assert!((found.tempo - tempo).abs() / tempo < 0.02, "tempo {} vs {tempo}", found.tempo);
        // Most beats within 40 ms of a true one.
        let near = found.times.iter().filter(|t| beats.iter().any(|b| (*b - **t).abs() < 0.04)).count();
        assert!(near as f64 >= 0.9 * found.times.len() as f64, "{near} of {} beats are on the pulse", found.times.len());
        // The downbeat: the first beat of a bar lands on a kick.
        let d = found.times[found.first_downbeat];
        let index = beats.iter().position(|b| (b - d).abs() < 0.04).expect("on a beat");
        assert_eq!(index % 4, 0, "downbeat at beat {index}");
    }

    #[test]
    fn finds_steady_tempos_and_their_downbeats() {
        for bpm in [72.0, 96.0, 120.0, 150.0] {
            let (sound, beats) = pattern(24.0, |_| bpm, 0.5, 0.3);
            let found = detect(&sound, RATE).unwrap_or_else(|| panic!("{bpm}: no beats"));
            check(&found, &beats, bpm);
            assert_eq!(found.source, "detected");
        }
    }

    #[test]
    fn follows_swing_and_a_tempo_drift() {
        let (sound, beats) = pattern(24.0, |_| 110.0, 0.66, 0.5);
        check(&detect(&sound, RATE).unwrap(), &beats, 110.0);
        // 100 to 125 beats per minute over 40 s: every beat followed, the average tempo between.
        let (sound, beats) = pattern(40.0, |t| 100.0 + 25.0 * t / 40.0, 0.5, 0.2);
        let found = detect(&sound, RATE).unwrap();
        let mean = 60.0 * (beats.len() - 1) as f64 / (beats[beats.len() - 1] - beats[0]);
        check(&found, &beats, mean);
    }

    #[test]
    fn no_pulse_no_beats() {
        assert!(detect(&vec![[0.0; 2]; RATE as usize * 10], RATE).is_none());
        // A steady tone has no onsets after its start.
        let tone: Vec<Frame> = (0..RATE as usize * 10).map(|i| [(i as f32 * 0.05).sin() * 0.3; 2]).collect();
        assert!(detect(&tone, RATE).is_none_or(|b| b.times.len() < 4));
        assert!(detect(&[[0.0; 2]; 100], RATE).is_none());
        let g = grid(120.0, 4, 0.25, 2.0, "ryolune");
        assert_eq!(g.times, [0.25, 0.75, 1.25, 1.75]);
    }

    #[test]
    fn the_fft_is_right() {
        let mut x: Vec<(f64, f64)> = (0..8).map(|i| ((std::f64::consts::TAU * i as f64 / 8.0).cos(), 0.0)).collect();
        fft(&mut x);
        assert!((x[1].0 - 4.0).abs() < 1e-9 && (x[7].0 - 4.0).abs() < 1e-9 && x[0].0.abs() < 1e-9);
    }
}
