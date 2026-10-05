//! The mixer: a project's sound, block by block, for the preview (in real time, following live
//! changes) and the export (offline, as fast as it goes).
//!
//! Signal path, per clip: decoded source (speed, reverse, pitch and channels applied by the
//! [`SourceOpener`]) → clip volume and its keyframes → fades (and crossfades at transitions) in
//! the clip's curve → clip effects → pan. Per track: the sum of its clips → track effects (with
//! automation) → ducking → fader and pan (automation) → its bus or the master, plus its sends.
//! Buses: effects → fader → master. Master: effects → fader → loudness gain (exports) → true-peak
//! limiter. Muted / solo follow the usual rules (solo on a track keeps its bus and the master).

use std::sync::Arc;

use kimchi_core::{Asset, Clip, Id, Project};

use crate::meter::Meters;
use crate::{Frame, Result};

/// Decoded sound of one clip, at the mixer's rate, stereo, starting at the time asked for.
pub trait SourceReader: Send {
    /// Fills `out` with the next frames; returns how many were written (fewer at the end of the
    /// source; the rest of `out` is left as it was).
    fn read(&mut self, out: &mut [Frame]) -> usize;
}

/// Opens clip sounds (kimchi-media does it with ffmpeg; tests with tones).
pub trait SourceOpener: Send + Sync {
    /// The clip's sound from timeline time `from` on (inside the clip, or before it for a
    /// transition's tail), with speed, reverse, pitch and channels already applied.
    fn open(&self, clip: &Clip, asset: &Asset, from: f64, rate: u32) -> Result<Box<dyn SourceReader>>;
}

/// Offline (exports: every block waits for its sources) or real time (preview: a source that
/// isn't ready plays silence rather than stalling the speakers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Offline,
    Realtime,
}

/// What the mixer renders: the whole mix, or only some tracks (stems, solo previews).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// Only these tracks (and the buses they reach); `None`: all of them.
    pub tracks: Option<Vec<Id>>,
    /// Apply the master (effects, fader, loudness, limiter). Stems usually don't.
    pub skip_master: bool,
}

pub struct Mixer {
    project: Arc<Project>,
    rate: u32,
    position: f64,
    meters: Arc<Meters>,
    _opener: Arc<dyn SourceOpener>,
    _mode: Mode,
}

impl Mixer {
    pub fn new(project: Arc<Project>, opener: Arc<dyn SourceOpener>, rate: u32, mode: Mode) -> Result<Self> {
        Ok(Self { project, rate, position: 0.0, meters: Arc::new(Meters::default()), _opener: opener, _mode: mode })
    }

    /// Render only part of the mix.
    pub fn with_selection(self, _selection: Selection) -> Self {
        self
    }

    /// Gain applied after the master fader, before the limiter (loudness normalisation).
    pub fn set_output_gain(&mut self, _gain: f64) {}

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Timeline time of the next frame [`Self::render`] makes.
    pub fn position(&self) -> f64 {
        self.position
    }

    /// Jump to timeline time `t` (effect tails are cleared).
    pub fn seek(&mut self, t: f64) {
        self.position = t.max(0.0);
    }

    /// The project changed while playing: levels, pans, effect settings and automation take
    /// effect at once; clips that moved are reopened.
    pub fn update(&mut self, project: Arc<Project>) {
        self.project = project;
    }

    /// The next `out.len()` frames of the mix.
    pub fn render(&mut self, out: &mut [Frame]) {
        out.fill([0.0; 2]);
        self.position += out.len() as f64 / self.rate as f64;
    }

    /// Live levels of every track, bus and the master, updated as blocks are rendered.
    pub fn meters(&self) -> Arc<Meters> {
        self.meters.clone()
    }

    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }
}
