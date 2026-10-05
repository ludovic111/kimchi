//! The levels playing now, for every meter in the window and `audio.meters`.
//!
//! The preview's mixer writes [`Meters`] snapshots stamped with the timeline time they belong
//! to; the window reads the one for the time the speakers play (the playhead), keeps peak
//! holds, the clip lights until clicked, and the master's integrated loudness over the last
//! play (from the momentary readings, gated as EBU R128 gates its blocks).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Global};
use kimchi_audio::meter::{Level, Snapshot};
use kimchi_core::Id;
use kimchi_core::audio::MIN_DB;
use serde_json::{Value, json};

use super::widgets::Reading;

/// How long a peak hold lingers.
const HOLD: Duration = Duration::from_millis(1500);

/// What the window knows about the sound playing.
#[derive(Default)]
pub struct Live {
    /// The sound playing (or last played): its meters for what is heard now.
    pub source: Option<Arc<crate::preview::AudioBuffer>>,
    holds: HashMap<String, ([f64; 2], Instant)>,
    /// Meters that went over full scale since their light was last cleared.
    clipped: std::collections::HashSet<String>,
    /// Momentary loudness readings of the current (or last) play, for its integrated value.
    momentary: Vec<f64>,
    last_time: f64,
}

impl Global for Live {}

/// Where the meters come from: the preview calls this when it starts playing a stream.
pub fn set_source(cx: &mut App, source: Option<Arc<crate::preview::AudioBuffer>>) {
    let live = cx.default_global::<Live>();
    if source.is_some() {
        live.momentary.clear();
    }
    live.source = source;
}

/// The levels heard now, while playing.
pub fn snapshot(cx: &App, _playhead: f64, playing: bool) -> Option<Snapshot> {
    if !playing {
        return None;
    }
    cx.try_global::<Live>()?.source.as_ref()?.meters()
}

/// A meter's key: `track:<id>`, `bus:<id>`, `master`.
pub fn key_of(kind: &str, id: Option<Id>) -> String {
    match id {
        Some(id) => format!("{kind}:{id}"),
        None => kind.to_string(),
    }
}

/// A level as the meters draw it, with its hold, and whether its clip light is on. Updates the
/// holds and lights as a side effect (call it once per meter per frame).
pub fn reading(cx: &mut App, key: &str, level: Option<&Level>) -> (Vec<Reading>, bool) {
    let live = cx.default_global::<Live>();
    let now = Instant::now();
    let level = level.copied().unwrap_or_default();
    if level.clipped || level.peak_db.iter().any(|p| *p > 0.0) {
        live.clipped.insert(key.to_string());
    }
    let hold = live.holds.entry(key.to_string()).or_insert(([MIN_DB; 2], now));
    for ch in 0..2 {
        if level.peak_db[ch] >= hold.0[ch] || now.duration_since(hold.1) > HOLD {
            if level.peak_db[ch] >= hold.0[ch] {
                hold.1 = now;
            }
            hold.0[ch] = level.peak_db[ch];
        }
    }
    let hold = hold.0;
    let readings = (0..2).map(|ch| Reading { peak: level.peak_db[ch], rms: level.rms_db[ch], hold: hold[ch] }).collect();
    (readings, live.clipped.contains(key))
}

/// Turns a clip light off.
pub fn clear_clip(cx: &mut App, key: &str) {
    cx.default_global::<Live>().clipped.remove(key);
}

/// Adds the master's momentary loudness of a new snapshot to the play's integrated value.
pub fn note_loudness(cx: &mut App, s: &Snapshot) {
    let live = cx.default_global::<Live>();
    if (s.time - live.last_time).abs() < 1e-6 {
        return;
    }
    live.last_time = s.time;
    if s.momentary_lufs > -70.0 {
        live.momentary.push(s.momentary_lufs);
    }
}

/// The integrated loudness of the readings so far (`None` for silence): mean energy over the
/// blocks above -70 LUFS, then again over those within 10 LU of that.
pub fn integrated(cx: &App) -> Option<f64> {
    let m = &cx.try_global::<Live>()?.momentary;
    gated(m)
}

pub fn gated(m: &[f64]) -> Option<f64> {
    let mean = |v: &[f64]| -> Option<f64> {
        if v.is_empty() {
            return None;
        }
        let e: f64 = v.iter().map(|l| 10f64.powf((l + 0.691) / 10.0)).sum::<f64>() / v.len() as f64;
        Some(-0.691 + 10.0 * e.log10())
    };
    let abs: Vec<f64> = m.iter().copied().filter(|l| *l > -70.0).collect();
    let first = mean(&abs)?;
    let rel: Vec<f64> = abs.into_iter().filter(|l| *l > first - 10.0).collect();
    mean(&rel)
}

/// `audio.meters`' answer.
pub fn meters_json(cx: &App, playhead: f64, playing: bool) -> Value {
    let level = |l: &Level| {
        json!({
            "peakDb": l.peak_db.map(|v| (v * 10.0).round() / 10.0),
            "rmsDb": l.rms_db.map(|v| (v * 10.0).round() / 10.0),
            "clipped": l.clipped,
        })
    };
    match snapshot(cx, playhead, playing) {
        None => json!({ "playing": playing, "note": if playing { "Levels arrive a moment after playback starts." } else { "Nothing plays: start playback to read levels." }, "integratedLufs": integrated(cx) }),
        Some(s) => json!({
            "playing": true,
            "time": s.time,
            "master": level(&s.master),
            "tracks": s.tracks.iter().map(|(id, l)| (id.to_string(), level(l))).collect::<serde_json::Map<_, _>>(),
            "buses": s.buses.iter().map(|(id, l)| (id.to_string(), level(l))).collect::<serde_json::Map<_, _>>(),
            "momentaryLufs": s.momentary_lufs,
            "shortTermLufs": s.short_term_lufs,
            "integratedLufs": integrated(cx),
            "duckingDb": s.ducking_db.iter().map(|(id, d)| (id.to_string(), json!(d))).collect::<serde_json::Map<_, _>>(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrated_loudness_is_gated() {
        assert_eq!(gated(&[]), None);
        // A steady -20: -20.
        assert!((gated(&[-20.0; 10]).unwrap() + 20.0).abs() < 1e-9);
        // Quiet passages far below the rest don't pull it down.
        let mut v = vec![-14.0; 20];
        v.extend([-40.0; 20]);
        assert!((gated(&v).unwrap() + 14.0).abs() < 1e-6);
    }
}
