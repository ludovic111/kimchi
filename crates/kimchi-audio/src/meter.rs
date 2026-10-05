//! Levels for the window's meters and `audio.meters`: peak and RMS per channel for every track,
//! bus and the master, and the master's momentary and short-term loudness.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use kimchi_core::Id;
use parking_lot::Mutex;
use serde::Serialize;

use crate::Frame;

const MIN: f64 = kimchi_core::audio::MIN_DB;

/// A level reading, in dBFS ([`kimchi_core::audio::MIN_DB`] for silence).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Level {
    pub peak_db: [f64; 2],
    pub rms_db: [f64; 2],
    /// The meter went over 0 dBFS since it was last reset.
    pub clipped: bool,
}

impl Default for Level {
    fn default() -> Self {
        Self { peak_db: [MIN; 2], rms_db: [MIN; 2], clipped: false }
    }
}

impl Level {
    /// The louder side's peak.
    pub fn peak(&self) -> f64 {
        self.peak_db[0].max(self.peak_db[1])
    }
}

/// One snapshot of every meter, at timeline time `time`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub time: f64,
    pub tracks: HashMap<Id, Level>,
    pub buses: HashMap<Id, Level>,
    pub master: Level,
    /// The master's loudness over the last 400 ms and 3 s (LUFS).
    pub momentary_lufs: f64,
    pub short_term_lufs: f64,
    /// How far each ducked track is pulled down right now (dB, negative).
    pub ducking_db: HashMap<Id, f64>,
    /// How far the master's limiter is pulling the sound down (dB, 0 or negative).
    pub limiter_db: f64,
}

/// Shared between the mixer (writes) and the window (reads the reading for the time it hears).
#[derive(Debug, Default)]
pub struct Meters {
    recent: Mutex<std::collections::VecDeque<Snapshot>>,
    reset: AtomicBool,
}

impl Meters {
    /// The mixer's newest snapshot at or before timeline time `t` (what the speakers play now).
    pub fn at(&self, t: f64) -> Option<Snapshot> {
        let recent = self.recent.lock();
        recent.iter().rev().find(|s| s.time <= t + 1e-6).or(recent.front()).cloned()
    }

    /// The newest snapshot.
    pub fn latest(&self) -> Option<Snapshot> {
        self.recent.lock().back().cloned()
    }

    /// Adds a snapshot, keeping the last few seconds.
    pub fn push(&self, s: Snapshot) {
        let mut recent = self.recent.lock();
        // A seek starts the timeline over: older readings no longer belong before the new ones.
        while recent.back().is_some_and(|b| b.time > s.time + 1e-6) {
            recent.pop_back();
        }
        recent.push_back(s);
        while recent.len() > 512 {
            recent.pop_front();
        }
    }

    /// Clears every clip light (the window's click on a red light).
    pub fn reset_clipped(&self) {
        self.reset.store(true, Ordering::Relaxed);
        for s in self.recent.lock().iter_mut() {
            s.master.clipped = false;
            s.tracks.values_mut().chain(s.buses.values_mut()).for_each(|l| l.clipped = false);
        }
    }

    /// Forgets every reading (playback stopped).
    pub fn clear(&self) {
        self.recent.lock().clear();
    }

    /// The mixer asks whether the clip lights should go out.
    pub(crate) fn take_reset(&self) -> bool {
        self.reset.swap(false, Ordering::Relaxed)
    }
}

/// Collects one strip's level over a snapshot's stretch.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Accumulator {
    peak: [f32; 2],
    squares: [f64; 2],
    frames: usize,
    clipped: bool,
}

impl Accumulator {
    pub(crate) fn add(&mut self, block: &[Frame]) {
        for f in block {
            for (c, sample) in f.iter().enumerate() {
                let v = sample.abs();
                self.peak[c] = self.peak[c].max(v);
                self.squares[c] += (v as f64) * (v as f64);
            }
        }
        if self.peak[0] > 1.0 || self.peak[1] > 1.0 {
            self.clipped = true;
        }
        self.frames += block.len();
    }

    /// The level so far, and starts the next stretch (the clip light stays on).
    pub(crate) fn take(&mut self) -> Level {
        let n = self.frames.max(1) as f64;
        let db = kimchi_core::audio::gain_to_db;
        let level = Level {
            peak_db: [db(self.peak[0] as f64), db(self.peak[1] as f64)],
            rms_db: [db((self.squares[0] / n).sqrt()), db((self.squares[1] / n).sqrt())],
            clipped: self.clipped,
        };
        let clipped = self.clipped;
        *self = Self { clipped, ..Self::default() };
        level
    }

    pub(crate) fn unclip(&mut self) {
        self.clipped = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_follow_the_timeline() {
        let m = Meters::default();
        for t in [0.0, 0.1, 0.2] {
            m.push(Snapshot { time: t, ..Default::default() });
        }
        assert_eq!(m.at(0.15).unwrap().time, 0.1);
        // A seek back drops the readings after it.
        m.push(Snapshot { time: 0.05, ..Default::default() });
        assert_eq!(m.latest().unwrap().time, 0.05);
        assert_eq!(m.at(0.3).unwrap().time, 0.05);
        let mut a = Accumulator::default();
        a.add(&[[0.5, 2.0], [-0.5, 0.0]]);
        let l = a.take();
        assert!((l.peak_db[0] + 6.02).abs() < 0.01 && l.clipped);
        assert!((l.rms_db[0] + 6.02).abs() < 0.01);
        assert!(a.take().clipped, "the clip light stays on");
        a.unclip();
        assert!(!a.take().clipped);
    }
}
