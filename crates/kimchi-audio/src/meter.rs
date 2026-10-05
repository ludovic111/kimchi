//! Levels for the window's meters and `audio.meters`: peak and RMS per channel for every track,
//! bus and the master, and the master's momentary and short-term loudness.

use std::collections::HashMap;

use kimchi_core::Id;
use parking_lot::Mutex;
use serde::Serialize;

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
        let min = kimchi_core::audio::MIN_DB;
        Self { peak_db: [min; 2], rms_db: [min; 2], clipped: false }
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
}

/// Shared between the mixer (writes) and the window (reads the reading for the time it hears).
#[derive(Debug, Default)]
pub struct Meters {
    recent: Mutex<std::collections::VecDeque<Snapshot>>,
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
        recent.push_back(s);
        while recent.len() > 512 {
            recent.pop_front();
        }
    }
}
