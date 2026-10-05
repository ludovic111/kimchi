//! Sources for tests (here and in the crates above): sounds made from a description in the
//! asset's path instead of decoding a file, so the mixer can be checked sample by sample.
//!
//! - `tone:<hz>:<amplitude>`: a sine on both channels.
//! - `dc:<left>:<right>`: a constant.
//! - `step:<seconds>:<value>`: silence, then `value` from that source time on.
//! - `count`: each frame's source frame number divided by the rate (the source's own clock),
//!   to check what plays when.
//! - anything else: silence.
//!
//! Speed and reverse follow the clip ([`Clip::source_time`]); pitch is ignored.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kimchi_core::{Asset, Clip};

use crate::mixer::{SourceOpener, SourceReader};
use crate::{Frame, Result};

/// Opens the sounds described above. `delay`: frames each new source keeps the mixer waiting
/// for in real time (a slow decoder), counted down on every `available` call.
#[derive(Debug, Default)]
pub struct ToneOpener {
    pub delay: usize,
    /// Sources opened so far.
    pub opened: AtomicUsize,
}

impl ToneOpener {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Sources that aren't ready for their first `polls` real-time reads.
    pub fn slow(polls: usize) -> Arc<Self> {
        Arc::new(Self { delay: polls, opened: AtomicUsize::new(0) })
    }
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Tone(f64, f64),
    Dc(f32, f32),
    Step(f64, f32),
    Count,
    Silence,
}

struct Reader {
    kind: Kind,
    /// Source time of the next frame, and its change per frame (negative when reversed).
    at: f64,
    step: f64,
    /// Source length: frames past it are the end.
    len: f64,
    rate: f64,
    not_ready: AtomicUsize,
    /// A tone's sine and cosine at `at`.
    phasor: Option<(f64, f64)>,
}

impl SourceReader for Reader {
    fn read(&mut self, out: &mut [Frame]) -> usize {
        let mut n = 0;
        for f in out.iter_mut() {
            if self.at < -1e-9 || self.at >= self.len {
                break;
            }
            let t = self.at;
            *f = match self.kind {
                Kind::Tone(hz, a) => {
                    // A rotating phasor (cheap), started from the exact phase.
                    let (s, c) = match self.phasor {
                        Some(p) => p,
                        None => (std::f64::consts::TAU * hz * t).sin_cos(),
                    };
                    let (ds, dc) = (std::f64::consts::TAU * hz * self.step).sin_cos();
                    self.phasor = Some((s * dc + c * ds, c * dc - s * ds));
                    let v = (a * s) as f32;
                    [v, v]
                }
                Kind::Dc(l, r) => [l, r],
                Kind::Step(at, v) => if t + 1e-9 >= at { [v, v] } else { [0.0, 0.0] },
                Kind::Count => [(t * self.rate) as f32, (t * self.rate) as f32],
                Kind::Silence => [0.0, 0.0],
            };
            self.at += self.step;
            n += 1;
        }
        n
    }

    fn available(&self) -> usize {
        let left = self.not_ready.load(Ordering::Relaxed);
        if left > 0 {
            self.not_ready.store(left - 1, Ordering::Relaxed);
            0
        } else {
            usize::MAX
        }
    }
}

impl SourceOpener for ToneOpener {
    fn open(&self, clip: &Clip, asset: &Asset, from: f64, rate: u32) -> Result<Box<dyn SourceReader>> {
        self.opened.fetch_add(1, Ordering::Relaxed);
        let parts: Vec<&str> = asset.path.split(':').collect();
        let num = |i: usize| parts.get(i).and_then(|p| p.parse::<f64>().ok()).unwrap_or(0.0);
        let kind = match parts.first().copied() {
            Some("tone") => Kind::Tone(num(1), num(2)),
            Some("dc") => Kind::Dc(num(1) as f32, num(2) as f32),
            Some("step") => Kind::Step(num(1), num(2) as f32),
            Some("count") => Kind::Count,
            Some("missing") => return Err(format!("missing media file {}", asset.path)),
            _ => Kind::Silence,
        };
        let speed = clip.speed.max(1e-3);
        Ok(Box::new(Reader {
            kind,
            at: clip.source_time(from),
            step: (if clip.reverse { -speed } else { speed }) / rate as f64,
            len: asset.duration().unwrap_or(f64::INFINITY),
            rate: rate as f64,
            not_ready: AtomicUsize::new(self.delay),
            phasor: None,
        }))
    }
}

/// An audio asset whose "file" is one of the descriptions above, `seconds` long.
pub fn asset(path: &str, seconds: f64) -> Asset {
    serde_json::from_value(serde_json::json!({
        "id": kimchi_core::new_id(),
        "name": path,
        "kind": "audio",
        "path": path,
        "meta": { "duration": seconds, "width": null, "height": null, "fps": null, "has_video": false, "has_audio": true,
                  "video_codec": null, "audio_codec": "pcm", "size_bytes": 0 },
        "origin": { "type": "imported" },
        "created_at": "2026-10-05T00:00:00Z",
    }))
    .expect("a valid asset")
}

/// A project with one audio track per entry of `tracks`, each holding clips of the given
/// assets as `(asset, start, duration)`.
pub fn project(assets: Vec<Asset>, tracks: Vec<Vec<(usize, f64, f64)>>) -> kimchi_core::Project {
    let mut p = kimchi_core::Project::new("test", kimchi_core::ProjectSettings::default());
    p.tracks.clear();
    for (i, clips) in tracks.into_iter().enumerate() {
        let mut t = kimchi_core::Track::new(kimchi_core::TrackKind::Audio, format!("Track {}", i + 1));
        for (a, start, duration) in clips {
            let content = kimchi_core::ClipContent::Media { asset_id: assets[a].id };
            t.clips.push(Clip::new(format!("clip {}", t.clips.len() + 1), start, duration, content));
        }
        p.tracks.push(t);
    }
    p.assets = assets;
    p
}
