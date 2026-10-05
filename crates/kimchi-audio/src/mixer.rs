//! The mixer: a project's sound, block by block, for the preview (in real time, following live
//! changes) and the export (offline, as fast as it goes).
//!
//! Signal path, per clip: decoded source (speed, reverse and pitch applied by the
//! [`SourceOpener`]) → channels ([`Channels`]) → clip volume and its keyframes → fades (and
//! crossfades at transitions, see [`heard`]) in the clip's curve → clip effects → pan (ryolune's
//! law, [`crate::dsp::pan_gains`]). Per track: the sum of its clips → track effects (with
//! automation) → ducking → fader and pan (automation) → its bus or the master, plus its sends
//! (before or after the fader). Buses: effects → fader and pan → master. Master: effects →
//! fader → output gain (loudness, exports) → true-peak limiter.
//!
//! Muted and solo follow the usual rules: a soloed track keeps the buses it feeds, a soloed bus
//! keeps the tracks that feed it. A [`Selection`] renders part of the mix (stems), ignoring solo.
//!
//! Plugin delay compensation: a clip's and a track's effects are compensated by reading their
//! sources that much earlier, so tracks always line up; buses are delayed to the slowest one;
//! the master's effects and the limiter delay the whole output, which the mixer pre-rolls on a
//! seek, so [`Mixer::position`] is always the time of what comes out.
//!
//! Effect tails ring on after a clip ends (its chain keeps running until it falls silent), and
//! effect instances survive [`Mixer::update`] while their slot keeps its id.
//!
//! The mixer holds plugin editors, which belong to the thread that made them: create it on the
//! thread that renders with it.

use std::collections::HashMap;
use std::sync::Arc;

use kimchi_core::anim::number_at;
use kimchi_core::audio::{Channels, FadeCurve, db_to_gain};
use kimchi_core::{Asset, Clip, ClipContent, Id, MediaKind, Project, Track};
use ryolune_engine::plugin::{MAX_BLOCK, ProcessContext};

use crate::chain::Chain;
use crate::dsp::{DelayLine, Limiter, add, pan_gains};
use crate::loudness::LoudnessMeter;
use crate::meter::{Accumulator, Meters, Snapshot};
use crate::{Frame, Result};

/// Decoded sound of one clip, at the mixer's rate, stereo, starting at the time asked for.
pub trait SourceReader: Send {
    /// Fills `out` with the next frames; returns how many were written (fewer at the end of the
    /// source; the rest of `out` is left as it was). May wait for the decoder.
    fn read(&mut self, out: &mut [Frame]) -> usize;

    /// Frames a [`Self::read`] can return now without waiting (`usize::MAX` once the source has
    /// ended, or for sources that never wait). Real-time mixing reads no more than this.
    fn available(&self) -> usize {
        usize::MAX
    }

    /// Why the source stopped early (a file that can't be decoded), if it did.
    fn error(&self) -> Option<String> {
        None
    }
}

/// Opens clip sounds (kimchi-media does it with ffmpeg; tests with tones).
pub trait SourceOpener: Send + Sync {
    /// The clip's sound from timeline time `from` on (inside the clip, or before it for a
    /// transition's tail), with speed, reverse and pitch already applied (the mixer applies the
    /// clip's [`Channels`] and everything after). Must return quickly: real-time mixing opens
    /// clips while it plays (start any slow work on another thread).
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
    /// Only these tracks (and the buses they reach); `None`: all of them. Solo is ignored when
    /// tracks or a bus are chosen.
    pub tracks: Option<Vec<Id>>,
    /// Apply the master (effects, fader, loudness, limiter). Stems usually don't.
    pub skip_master: bool,
    /// Only what comes out of this bus (a bus's stem): everything routed or sent to it, through it.
    pub bus: Option<Id>,
}

/// A clip as the mixer plays it: around a transition on a cut, the outgoing clip plays on and
/// the incoming one starts early (as far as their media allow), crossfading over the
/// transition (equal power, unless the clip asks for another curve); a transition with no clip
/// before it fades the sound in.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    /// The clip, stretched over its transitions (keyframes stay where they were on the timeline).
    pub clip: Clip,
    pub fade_in: f64,
    pub fade_in_curve: FadeCurve,
    pub fade_out: f64,
    pub fade_out_curve: FadeCurve,
}

/// The asset a clip's sound comes from, when it makes any.
pub fn sound_of<'a>(project: &'a Project, clip: &Clip) -> Option<&'a Asset> {
    let ClipContent::Media { asset_id } = &clip.content else { return None };
    let asset = project.asset(*asset_id)?;
    let sounds = match asset.kind {
        MediaKind::Audio => true,
        MediaKind::Video => asset.meta.has_audio,
        MediaKind::Image => false,
    };
    (sounds && !clip.audio.muted && clip.duration > 0.0).then_some(asset)
}

/// A track's clips with sound as they play (see [`Heard`]), in timeline order.
pub fn heard(project: &Project, track: &Track) -> Vec<Heard> {
    let mut out: Vec<Option<Heard>> = track
        .clips
        .iter()
        .map(|c| {
            Some(Heard { clip: c.clone(), fade_in: c.fade_in, fade_in_curve: c.audio.fade_curve, fade_out: c.fade_out, fade_out_curve: c.audio.fade_curve })
        })
        .collect();
    let source_len = |c: &Clip| c.asset_id().and_then(|id| project.asset(id)).and_then(|a| a.duration()).unwrap_or(0.0);
    // Crossfades are equal power unless the clip chose a curve other than the default.
    let cross = |c: &Clip| if c.audio.fade_curve == FadeCurve::Linear { FadeCurve::EqualPower } else { c.audio.fade_curve };
    for span in kimchi_core::transition::spans(track) {
        let (len, half) = (span.duration(), span.duration() / 2.0);
        let Some(from) = span.from else {
            let to = out[span.to].as_mut().expect("present");
            to.fade_in = to.fade_in.max(len);
            continue;
        };
        let room = |h: &Heard| h.clip.room(source_len(&h.clip));
        let after = room(out[from].as_ref().expect("present")).1.min(half);
        let a = out[from].as_mut().expect("present");
        a.clip.duration += after;
        if a.clip.reverse {
            a.clip.in_point -= after * a.clip.speed;
        }
        a.fade_out = after + half;
        a.fade_out_curve = cross(&a.clip);
        let before = room(out[span.to].as_ref().expect("present")).0.min(half);
        let b = out[span.to].as_mut().expect("present");
        b.clip.start -= before;
        b.clip.duration += before;
        if !b.clip.reverse {
            b.clip.in_point -= before * b.clip.speed;
        }
        kimchi_core::anim::shift(&mut b.clip.keyframes, before);
        b.fade_in = before + half;
        b.fade_in_curve = cross(&b.clip);
    }
    let mut heard: Vec<Heard> = out
        .into_iter()
        .flatten()
        .filter(|h| sound_of(project, &h.clip).is_some())
        .map(|mut h| {
            // Fades fit the clip; when together they would overlap they shrink in proportion.
            let d = h.clip.duration.max(0.0);
            let (fi, fo) = (h.fade_in.clamp(0.0, d), h.fade_out.clamp(0.0, d));
            let k = if fi + fo > d && fi + fo > 0.0 { d / (fi + fo) } else { 1.0 };
            (h.fade_in, h.fade_out) = (fi * k, fo * k);
            h
        })
        .collect();
    heard.sort_by(|a, b| a.clip.start.total_cmp(&b.clip.start));
    heard
}

/// Seconds before a clip starts that its source is opened (so the decoder is ready in time).
const OPEN_AHEAD: f64 = 1.0;
/// Every clip edge gets this short ramp, fade or not, so a cut never clicks (ryolune's too).
const EDGE_RAMP: f64 = 0.002;
/// Clip gains, pans and automation are computed every this many frames and joined by lines.
const GRAIN: usize = 32;
/// The ducking detector holds a sound this long (bridges syllables).
const DUCK_HOLD: f64 = 0.12;

/// Where a track or a bus's sound goes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Out {
    Master,
    Bus(usize),
    Nowhere,
}

/// What ducks a track.
#[derive(Debug, Clone)]
struct DuckNode {
    keys: Vec<usize>,
    floor: f32,
    threshold: f32,
    attack: f32,
    release: f32,
}

#[derive(Debug, Clone)]
struct TrackNode {
    id: Id,
    name: String,
    audible: bool,
    out: Out,
    /// (bus index, gain, before the fader).
    sends: Vec<(usize, f32, bool)>,
    duck: Option<DuckNode>,
    mix: kimchi_core::TrackMix,
    /// Indexes in `Graph::clips`.
    clips: Vec<usize>,
}

#[derive(Debug, Clone)]
struct BusNode {
    id: Id,
    name: String,
    audible: bool,
    /// Its output reaches the master (a bus stem keeps only one).
    out: bool,
    mix: kimchi_core::TrackMix,
}

/// A clip with sound, ready to play.
#[derive(Debug, Clone)]
struct ClipNode {
    heard: Heard,
    asset: Asset,
    /// First and last frame (exclusive) on the timeline, stretched over transitions.
    start: i64,
    end: i64,
}

/// What the project's sound is made of (rebuilt on every change).
#[derive(Debug, Clone, Default)]
struct Graph {
    tracks: Vec<TrackNode>,
    buses: Vec<BusNode>,
    clips: Vec<ClipNode>,
    master: kimchi_core::audio::Master,
    skip_master: bool,
}

impl Graph {
    fn build(project: &Project, selection: &Selection, rate: u32) -> Self {
        let mixer = &project.mixer;
        let bus_index = |id: &Id| mixer.buses.iter().position(|b| b.id == *id);
        let feeds = |t: &Track| -> Vec<usize> {
            t.mix.output.iter().chain(t.mix.sends.iter().map(|s| &s.bus)).filter_map(bus_index).collect()
        };
        let solo = selection.tracks.is_none() && selection.bus.is_none() && (project.tracks.iter().any(|t| t.mix.solo) || mixer.buses.iter().any(|b| b.mix.solo));
        let chosen = |t: &Track| selection.tracks.as_ref().is_none_or(|ids| ids.contains(&t.id));
        let track_audible = |t: &Track| {
            chosen(t) && !t.muted && (!solo || t.mix.solo || feeds(t).iter().any(|&b| mixer.buses[b].mix.solo))
        };
        let mut g = Graph { master: mixer.master.clone(), skip_master: selection.skip_master, ..Default::default() };
        for b in &mixer.buses {
            let index = g.buses.len();
            let fed_by_solo = project.tracks.iter().any(|t| t.mix.solo && feeds(t).contains(&index));
            let reached = project.tracks.iter().any(|t| chosen(t) && feeds(t).contains(&index));
            let audible = !b.muted && reached && (!solo || b.mix.solo || fed_by_solo);
            g.buses.push(BusNode {
                id: b.id,
                name: b.name.clone(),
                audible,
                out: selection.bus.is_none_or(|only| only == b.id),
                mix: b.mix.clone(),
            });
        }
        for t in &project.tracks {
            let mut node = TrackNode {
                id: t.id,
                name: t.name.clone(),
                audible: track_audible(t),
                out: match t.mix.output.as_ref().and_then(bus_index) {
                    Some(b) => Out::Bus(b),
                    None if selection.bus.is_some() => Out::Nowhere,
                    None => Out::Master,
                },
                sends: t
                    .mix
                    .sends
                    .iter()
                    .filter_map(|s| Some((bus_index(&s.bus)?, db_to_gain(s.level_db) as f32, s.pre_fader)))
                    .collect(),
                duck: None,
                mix: t.mix.clone(),
                clips: vec![],
            };
            if !node.audible {
                node.out = Out::Nowhere;
                node.sends.clear();
            }
            for h in heard(project, t) {
                let Some(asset) = sound_of(project, &h.clip) else { continue };
                let start = (h.clip.start * rate as f64).round() as i64;
                let end = (h.clip.end() * rate as f64).round() as i64;
                if end <= start {
                    continue;
                }
                node.clips.push(g.clips.len());
                g.clips.push(ClipNode { asset: asset.clone(), heard: h, start, end });
            }
            g.tracks.push(node);
        }
        // Ducking: the keys are the named tracks, or every track that isn't ducked itself.
        let ducked: Vec<bool> = project.tracks.iter().map(|t| t.mix.duck.is_some()).collect();
        for (i, t) in project.tracks.iter().enumerate() {
            let Some(d) = &t.mix.duck else { continue };
            let keys: Vec<usize> = if d.under.is_empty() {
                (0..project.tracks.len()).filter(|&k| k != i && !ducked[k]).collect()
            } else {
                d.under.iter().filter_map(|id| project.tracks.iter().position(|o| o.id == *id)).filter(|&k| k != i).collect()
            };
            let coef = |secs: f64| (1.0 - (-1.0 / (secs.max(0.001) * rate as f64)).exp()) as f32;
            g.tracks[i].duck = Some(DuckNode {
                keys,
                floor: db_to_gain(d.amount_db) as f32,
                threshold: db_to_gain(d.threshold_db) as f32,
                attack: coef(d.attack),
                release: coef(d.release),
            });
        }
        g
    }
}

/// The identity of a clip's decoded sound: when any of it changes the source is opened again.
fn timing(c: &ClipNode) -> (String, i64, i64, [u64; 5], bool, bool) {
    let k = &c.heard.clip;
    (c.asset.path.clone(), c.start, c.end, [k.in_point.to_bits(), k.speed.to_bits(), k.audio.pitch.to_bits(), k.duration.to_bits(), k.start.to_bits()], k.reverse, k.audio.preserve_pitch)
}

/// A clip being played.
struct Voice {
    id: Id,
    /// Its track and clip in the graph.
    track: usize,
    clip: usize,
    timing: (String, i64, i64, [u64; 5], bool, bool),
    reader: Option<Box<dyn SourceReader>>,
    /// The timeline frame the reader gives next, and frames it still owes (real time: it
    /// wasn't ready, the mixer played silence, those frames are skipped when they come).
    next: i64,
    owed: usize,
    ended: bool,
    chain: Chain,
    /// Frames the clip's effects delay it by: its source is read that much earlier.
    lead: i64,
    /// The source frame the last block reached.
    cursor: i64,
    buf: Vec<Frame>,
}

struct TrackState {
    chain: Chain,
    buf: Vec<Frame>,
    /// Frames the track's effects delay it by: its clips are read that much earlier.
    lead: i64,
    /// Ducking: frames the keys still count as sounding, the gain now and its lowest since
    /// the last meter reading.
    hold: usize,
    duck_gain: f32,
    duck_low: f32,
    meter: Accumulator,
}

struct BusState {
    chain: Chain,
    buf: Vec<Frame>,
    delay: DelayLine,
    meter: Accumulator,
}

pub struct Mixer {
    project: Arc<Project>,
    rate: u32,
    mode: Mode,
    opener: Arc<dyn SourceOpener>,
    selection: Selection,
    graph: Graph,
    meters: Arc<Meters>,
    output_gain: f32,
    limiter_bypassed: bool,
    /// Timeline frame of the next internal frame (sources and tracks run `latency` frames
    /// ahead of the output).
    frame: i64,
    /// Output frames still to drop after a seek (the delay of the buses, master and limiter).
    preroll: usize,
    latency: usize,
    /// Clips being played, in the graph's clip order (so sums are always made in one order).
    voices: Vec<Voice>,
    /// The frame at which to look for clips to open next.
    next_open: i64,
    tracks: HashMap<Id, TrackState>,
    buses: HashMap<Id, BusState>,
    bus_max: usize,
    direct: Vec<Frame>,
    direct_delay: DelayLine,
    master_chain: Chain,
    master_buf: Vec<Frame>,
    limiter: Limiter,
    master_meter: Accumulator,
    loudness: LoudnessMeter,
    /// Output frames since the last meter snapshot, and the time it started at.
    meter_frames: usize,
    meter_time: f64,
    sample_time: i64,
    scratch: Vec<Frame>,
    problems: Vec<String>,
}

impl Mixer {
    pub fn new(project: Arc<Project>, opener: Arc<dyn SourceOpener>, rate: u32, mode: Mode) -> Result<Self> {
        if !(8_000..=384_000).contains(&rate) {
            return Err(format!("{rate} Hz isn't a sample rate the mixer can run at"));
        }
        let selection = Selection::default();
        let graph = Graph::build(&project, &selection, rate);
        let mut m = Self {
            project,
            rate,
            mode,
            opener,
            selection,
            graph,
            meters: Arc::new(Meters::default()),
            output_gain: 1.0,
            limiter_bypassed: false,
            frame: 0,
            preroll: 0,
            latency: 0,
            voices: vec![],
            next_open: i64::MIN,
            tracks: HashMap::new(),
            buses: HashMap::new(),
            bus_max: 0,
            direct: vec![[0.0; 2]; MAX_BLOCK],
            direct_delay: DelayLine::default(),
            master_chain: Chain::new(rate),
            master_buf: vec![[0.0; 2]; MAX_BLOCK],
            limiter: Limiter::new(rate, -1.0),
            master_meter: Accumulator::default(),
            loudness: LoudnessMeter::new(rate),
            meter_frames: 0,
            meter_time: 0.0,
            sample_time: 0,
            scratch: vec![[0.0; 2]; MAX_BLOCK],
            problems: vec![],
        };
        m.sync();
        m.seek(0.0);
        Ok(m)
    }

    /// Render only part of the mix.
    pub fn with_selection(mut self, selection: Selection) -> Self {
        let at = self.position();
        self.selection = selection;
        self.graph = Graph::build(&self.project, &self.selection, self.rate);
        self.sync();
        self.seek(at);
        self
    }

    /// Share meters with someone already holding them (the preview hands them to the window
    /// before the mixer exists).
    pub fn with_meters(mut self, meters: Arc<Meters>) -> Self {
        self.meters = meters;
        self
    }

    /// Gain applied after the master fader, before the limiter (loudness normalisation).
    pub fn set_output_gain(&mut self, gain: f64) {
        self.output_gain = if gain.is_finite() { gain.max(0.0) as f32 } else { 1.0 };
    }

    /// Turns the master's limiter off whatever the project says (exports measure the mix
    /// first, then limit after their loudness gain). Its delay stays.
    pub fn bypass_limiter(&mut self, bypass: bool) {
        self.limiter_bypassed = bypass;
        self.limiter.set_enabled(!bypass && self.graph.master.limiter && !self.graph.skip_master);
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Timeline time of the next frame [`Self::render`] makes.
    pub fn position(&self) -> f64 {
        (self.frame - self.latency as i64 + self.preroll as i64) as f64 / self.rate as f64
    }

    /// Frames between a source sample being read and it coming out (buses, master effects and
    /// the limiter); hidden by a pre-roll after every seek.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Jump to timeline time `t` (effect tails are cleared).
    pub fn seek(&mut self, t: f64) {
        let t = if t.is_finite() { t.max(0.0) } else { 0.0 };
        self.frame = (t * self.rate as f64).round() as i64;
        self.preroll = self.latency;
        self.voices.clear();
        self.next_open = i64::MIN;
        for s in self.tracks.values_mut() {
            s.chain.reset();
            s.hold = 0;
            s.duck_gain = 1.0;
            s.duck_low = 1.0;
        }
        for s in self.buses.values_mut() {
            s.chain.reset();
            s.delay.clear();
        }
        self.direct_delay.clear();
        self.master_chain.reset();
        self.limiter.reset();
        self.loudness.reset();
        self.meter_frames = 0;
        self.meter_time = t;
    }

    /// Waits, at most `timeout`, until the clips that sound at the current position have
    /// decoded sound ready, so real-time playback starts with its sound rather than a short
    /// silence while the decoders start.
    pub fn prime(&mut self, timeout: std::time::Duration) {
        let deadline = std::time::Instant::now() + timeout;
        let b = self.frame;
        self.open(b, MAX_BLOCK);
        let soon = b + self.rate as i64 / 10 + self.latency as i64;
        loop {
            let ready = self.voices.iter().filter(|v| v.next <= soon).all(|v| v.ended || v.reader.as_ref().is_none_or(|r| r.available() >= MAX_BLOCK * 8));
            if ready || std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The project changed while playing: levels, pans, effect settings and automation take
    /// effect at once; clips that moved are reopened; effect instances whose slot stayed keep
    /// their state and tails.
    pub fn update(&mut self, project: Arc<Project>) {
        self.project = project;
        self.graph = Graph::build(&self.project, &self.selection, self.rate);
        let before = self.latency;
        let at = self.position();
        self.sync();
        if self.latency != before {
            // The output's delay changed (a plugin with lookahead): start again from here.
            self.seek(at);
        }
    }

    /// Follows the graph: states for new tracks and buses, chains synced, voices of clips that
    /// changed dropped, delays sized.
    fn sync(&mut self) {
        let rate = self.rate;
        let g = &self.graph;
        self.tracks.retain(|id, _| g.tracks.iter().any(|t| t.id == *id));
        for t in &g.tracks {
            let s = self.tracks.entry(t.id).or_insert_with(|| TrackState {
                chain: Chain::new(rate),
                buf: vec![[0.0; 2]; MAX_BLOCK],
                lead: 0,
                hold: 0,
                duck_gain: 1.0,
                duck_low: 1.0,
                meter: Accumulator::default(),
            });
            s.chain.sync(&t.mix.effects, &t.name);
            s.chain.automate(&t.mix.keyframes);
            s.lead = s.chain.latency() as i64;
        }
        self.buses.retain(|id, _| g.buses.iter().any(|b| b.id == *id));
        for b in &g.buses {
            let s = self.buses.entry(b.id).or_insert_with(|| BusState {
                chain: Chain::new(rate),
                buf: vec![[0.0; 2]; MAX_BLOCK],
                delay: DelayLine::default(),
                meter: Accumulator::default(),
            });
            s.chain.sync(&b.mix.effects, &b.name);
            s.chain.automate(&b.mix.keyframes);
        }
        self.bus_max = g.buses.iter().filter(|b| b.audible).map(|b| self.buses[&b.id].chain.latency()).max().unwrap_or(0);
        for b in &g.buses {
            let s = self.buses.get_mut(&b.id).expect("synced");
            s.delay.resize(self.bus_max.saturating_sub(s.chain.latency()));
        }
        self.direct_delay.resize(self.bus_max);
        if g.skip_master {
            self.master_chain.sync(&[], "master");
        } else {
            self.master_chain.sync(&g.master.effects, "master");
            self.master_chain.automate(&g.master.keyframes);
        }
        self.limiter.set_ceiling(g.master.ceiling_db);
        self.limiter.set_enabled(g.master.limiter && !g.skip_master && !self.limiter_bypassed);
        self.latency = self.bus_max + self.master_chain.latency() + self.limiter.latency();
        // Voices: clips that are gone or whose sound changed stop; the rest follow their effects.
        let mut place: HashMap<Id, (usize, usize)> = HashMap::new();
        for (ti, t) in g.tracks.iter().enumerate() {
            for &ci in &t.clips {
                place.insert(g.clips[ci].heard.clip.id, (ti, ci));
            }
        }
        self.voices.retain(|v| place.get(&v.id).is_some_and(|&(_, ci)| timing(&g.clips[ci]) == v.timing));
        for v in &mut self.voices {
            (v.track, v.clip) = place[&v.id];
            let c = &g.clips[v.clip];
            v.chain.sync(&c.heard.clip.audio.effects, &c.heard.clip.name);
            v.lead = v.chain.latency() as i64;
        }
        self.voices.sort_by_key(|v| v.clip);
        self.next_open = i64::MIN;
        let mut problems: Vec<String> = vec![];
        for s in self.tracks.values() {
            problems.extend(s.chain.problems().map(String::from));
        }
        for s in self.buses.values() {
            problems.extend(s.chain.problems().map(String::from));
        }
        problems.extend(self.master_chain.problems().map(String::from));
        problems.sort();
        problems.dedup();
        self.problems = problems;
    }

    /// Effects that couldn't be loaded and sources that couldn't be read, in plain words.
    pub fn problems(&self) -> Vec<String> {
        let mut all = self.problems.clone();
        for v in &self.voices {
            all.extend(v.chain.problems().map(String::from));
        }
        all.sort();
        all.dedup();
        all
    }

    /// The next `out.len()` frames of the mix.
    pub fn render(&mut self, out: &mut [Frame]) {
        while self.preroll > 0 {
            let n = self.preroll.min(MAX_BLOCK);
            let mut scratch = std::mem::take(&mut self.scratch);
            self.block(&mut scratch[..n], false);
            self.scratch = scratch;
            self.preroll -= n;
        }
        let mut done = 0;
        while done < out.len() {
            let n = (out.len() - done).min(MAX_BLOCK);
            self.block(&mut out[done..done + n], true);
            done += n;
        }
    }

    /// Live levels of every track, bus and the master, updated as blocks are rendered.
    pub fn meters(&self) -> Arc<Meters> {
        self.meters.clone()
    }

    pub fn project(&self) -> &Arc<Project> {
        &self.project
    }

    /// Opens the clips that start soon (or play now) on audible tracks, and lets go of the
    /// ones whose sound and tails are over (or whose track went quiet). Looks every few blocks.
    fn open(&mut self, b: i64, n: usize) {
        if b < self.next_open {
            return;
        }
        let ahead = (OPEN_AHEAD * self.rate as f64) as i64;
        self.next_open = b + ahead / 8;
        let rate = self.rate;
        let mut keep = vec![false; self.voices.len()];
        let mut opened = vec![];
        for (ti, track) in self.graph.tracks.iter().enumerate().filter(|(_, t)| t.audible) {
            let track_lead = self.tracks.get(&track.id).map_or(0, |s| s.lead);
            for &ci in &track.clips {
                let c = &self.graph.clips[ci];
                let id = c.heard.clip.id;
                if let Some(i) = self.voices.iter().position(|v| v.id == id) {
                    let v = &self.voices[i];
                    keep[i] = v.cursor < c.end || !v.chain.rang_out();
                    continue;
                }
                let first = b + track_lead;
                if c.end <= first || c.start > first + n as i64 + ahead {
                    continue;
                }
                let mut chain = Chain::new(rate);
                chain.sync(&c.heard.clip.audio.effects, &c.heard.clip.name);
                let lead = chain.latency() as i64;
                let from = c.start.max(first + lead);
                let reader = match self.opener.open(&c.heard.clip, &c.asset, from as f64 / rate as f64, rate) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        tracing::warn!(clip = %c.heard.clip.name, "sound not opened: {e}");
                        let p = format!("{}: {e}", c.heard.clip.name);
                        if !self.problems.contains(&p) {
                            self.problems.push(p);
                        }
                        None
                    }
                };
                opened.push(Voice {
                    id,
                    track: ti,
                    clip: ci,
                    timing: timing(c),
                    ended: reader.is_none(),
                    reader,
                    next: from,
                    owed: 0,
                    chain,
                    lead,
                    cursor: from,
                    buf: vec![[0.0; 2]; MAX_BLOCK],
                });
            }
        }
        let mut k = keep.into_iter();
        self.voices.retain(|_| k.next().unwrap_or(false));
        if !opened.is_empty() {
            self.voices.extend(opened);
            self.voices.sort_by_key(|v| v.clip);
        }
    }

    fn context(&self, t: f64) -> ProcessContext {
        ProcessContext {
            playing: true,
            recording: false,
            tempo: 120.0,
            position_beats: t * 2.0,
            position_seconds: t,
            sample_time: self.sample_time,
            numerator: 4,
            denominator: 4,
            cycle: None,
            bar_start_beats: (t * 2.0 / 4.0).floor() * 4.0,
        }
    }

    /// One block of at most `MAX_BLOCK` frames. `emit`: false while pre-rolling.
    fn block(&mut self, out: &mut [Frame], emit: bool) {
        let n = out.len();
        let rate = self.rate as f64;
        let b = self.frame;
        let t = b as f64 / rate;
        self.open(b, n);
        let realtime = self.mode == Mode::Realtime;
        let ctx = self.context(t);

        // Tracks: their clips (read ahead by the track's own delay), then their effects.
        let graph = &self.graph;
        let mut leads = Vec::with_capacity(graph.tracks.len());
        for track in &graph.tracks {
            let state = self.tracks.get_mut(&track.id).expect("synced");
            state.buf[..n].fill([0.0; 2]);
            leads.push(state.lead);
        }
        for v in &mut self.voices {
            let track = &graph.tracks[v.track];
            if !track.audible {
                continue;
            }
            play_voice(v, &graph.clips[v.clip], b + leads[v.track], n, realtime, rate, &ctx);
            add(&mut self.tracks.get_mut(&track.id).expect("synced").buf[..n], &v.buf[..n], 1.0);
        }
        for track in graph.tracks.iter().filter(|t| t.audible) {
            let state = self.tracks.get_mut(&track.id).expect("synced");
            let automation_time = t + state.lead as f64 / rate;
            state.chain.process(&mut state.buf[..n], &ctx, Some((&track.mix.keyframes, automation_time)));
        }

        // Ducking keys: the keying tracks' sound at their fader's level.
        let mut keys: HashMap<usize, Vec<f32>> = HashMap::new();
        for (i, track) in graph.tracks.iter().enumerate() {
            let Some(d) = track.duck.as_ref().filter(|_| track.audible) else { continue };
            let mut key = vec![0.0f32; n];
            for &k in &d.keys {
                let kt = &graph.tracks[k];
                if !kt.audible {
                    continue;
                }
                let g = db_to_gain(kt.mix.gain_db_at(t)) as f32;
                for (x, f) in key.iter_mut().zip(&self.tracks[&kt.id].buf[..n]) {
                    *x = x.max(f[0].abs().max(f[1].abs()) * g);
                }
            }
            keys.insert(i, key);
        }

        // Ducking, sends, faders and pans, routing.
        let bus_ids: Vec<Id> = graph.buses.iter().map(|b| b.id).collect();
        for id in &bus_ids {
            self.buses.get_mut(id).expect("synced").buf[..n].fill([0.0; 2]);
        }
        let direct = &mut self.direct[..n];
        direct.fill([0.0; 2]);
        let hold = (DUCK_HOLD * rate) as usize;
        for (i, track) in graph.tracks.iter().enumerate() {
            if !track.audible {
                continue;
            }
            let state = self.tracks.get_mut(&track.id).expect("synced");
            if let (Some(d), Some(key)) = (&track.duck, keys.get(&i)) {
                for (f, &k) in state.buf[..n].iter_mut().zip(key) {
                    if k > d.threshold {
                        state.hold = hold;
                    } else {
                        state.hold = state.hold.saturating_sub(1);
                    }
                    let (target, coef) = if state.hold > 0 { (d.floor, d.attack) } else { (1.0, d.release) };
                    state.duck_gain += (target - state.duck_gain) * coef;
                    f[0] *= state.duck_gain;
                    f[1] *= state.duck_gain;
                    state.duck_low = state.duck_low.min(state.duck_gain);
                }
            }
            for &(bus, level, _) in track.sends.iter().filter(|s| s.2) {
                add(&mut self.buses.get_mut(&bus_ids[bus]).expect("synced").buf[..n], &state.buf[..n], level);
            }
            fader(&mut state.buf[..n], &track.mix, t, rate);
            state.meter.add(&state.buf[..n]);
            match track.out {
                Out::Master => add(direct, &state.buf[..n], 1.0),
                Out::Bus(bus) => add(&mut self.buses.get_mut(&bus_ids[bus]).expect("synced").buf[..n], &state.buf[..n], 1.0),
                Out::Nowhere => {}
            }
            for &(bus, level, _) in track.sends.iter().filter(|s| !s.2) {
                add(&mut self.buses.get_mut(&bus_ids[bus]).expect("synced").buf[..n], &state.buf[..n], level);
            }
        }

        // Buses, lined up with the slowest, then the tracks that go straight to the master.
        let master = &mut self.master_buf[..n];
        master.fill([0.0; 2]);
        let bus_time = t - self.bus_max as f64 / rate;
        for bus in &graph.buses {
            let s = self.buses.get_mut(&bus.id).expect("synced");
            if !bus.audible {
                continue;
            }
            let buf = &mut s.buf[..n];
            s.chain.process(buf, &ctx, Some((&bus.mix.keyframes, t)));
            s.delay.process(buf);
            fader(buf, &bus.mix, bus_time, rate);
            s.meter.add(buf);
            if bus.out {
                add(master, buf, 1.0);
            }
        }
        self.direct_delay.process(direct);
        add(master, direct, 1.0);

        // The master: effects, fader, loudness gain, limiter.
        if !graph.skip_master {
            let m = &graph.master;
            self.master_chain.process(master, &ctx, Some((&m.keyframes, bus_time)));
            let at = bus_time - self.master_chain.latency() as f64 / rate;
            let keys = &m.keyframes;
            let (min, max) = (kimchi_core::audio::MIN_DB, kimchi_core::audio::MAX_DB);
            let gain_at = |t: f64| db_to_gain(number_at(keys, "gainDb", t).unwrap_or(m.gain_db).clamp(min, max)) as f32;
            let out_gain = self.output_gain;
            ramp(master, |k| gain_at(at + k as f64 / rate) * out_gain);
        }
        self.limiter.process(master);
        crate::dsp::sanitize(master);
        out.copy_from_slice(master);
        self.frame += n as i64;
        self.sample_time += n as i64;
        for v in &self.voices {
            if let Some(e) = v.reader.as_ref().and_then(|r| r.error())
                && !self.problems.contains(&e)
            {
                tracing::warn!("{e}");
                self.problems.push(e);
            }
        }
        if emit {
            self.master_meter.add(out);
            // Momentary and short-term loudness are for people watching the meters.
            if realtime {
                self.loudness.push(out);
            }
            self.meter_frames += n;
            if self.meter_frames >= (self.rate / 30) as usize {
                self.snapshot();
            }
        }
    }

    fn snapshot(&mut self) {
        let unclip = self.meters.take_reset();
        let mut s = Snapshot { time: self.meter_time, ..Default::default() };
        for track in &self.graph.tracks {
            let st = self.tracks.get_mut(&track.id).expect("synced");
            if unclip {
                st.meter.unclip();
            }
            s.tracks.insert(track.id, st.meter.take());
            if track.duck.is_some() {
                s.ducking_db.insert(track.id, kimchi_core::audio::gain_to_db(st.duck_low as f64).min(0.0));
                st.duck_low = st.duck_gain;
            }
        }
        for bus in &self.graph.buses {
            let st = self.buses.get_mut(&bus.id).expect("synced");
            if unclip {
                st.meter.unclip();
            }
            s.buses.insert(bus.id, st.meter.take());
        }
        if unclip {
            self.master_meter.unclip();
        }
        s.master = self.master_meter.take();
        s.momentary_lufs = self.loudness.momentary();
        s.short_term_lufs = self.loudness.short_term();
        s.limiter_db = self.limiter.take_reduction_db();
        self.meters.push(s);
        self.meter_time += self.meter_frames as f64 / self.rate as f64;
        self.meter_frames = 0;
    }
}

/// Applies `gain(k)` to every frame `k`, computed every [`GRAIN`] frames and joined by lines.
fn ramp(block: &mut [Frame], gain: impl Fn(usize) -> f32) {
    let n = block.len();
    let mut k = 0;
    let mut g0 = gain(0);
    while k < n {
        let end = (k + GRAIN).min(n);
        let g1 = gain(end);
        let step = (g1 - g0) / (end - k) as f32;
        for (j, f) in block[k..end].iter_mut().enumerate() {
            let g = g0 + step * j as f32;
            f[0] *= g;
            f[1] *= g;
        }
        g0 = g1;
        k = end;
    }
}

/// Left and right gains of `pan(time)` joined by lines over the block, applied.
fn pan_ramp(block: &mut [Frame], gains: impl Fn(usize) -> [f32; 2]) {
    let n = block.len();
    let mut k = 0;
    let mut a = gains(0);
    while k < n {
        let end = (k + GRAIN).min(n);
        let z = gains(end);
        let len = (end - k) as f32;
        for (j, f) in block[k..end].iter_mut().enumerate() {
            let x = j as f32 / len;
            f[0] *= a[0] + (z[0] - a[0]) * x;
            f[1] *= a[1] + (z[1] - a[1]) * x;
        }
        a = z;
        k = end;
    }
}

/// A strip's fader and pan (automation applied) on a block starting at timeline time `t`.
fn fader(block: &mut [Frame], mix: &kimchi_core::TrackMix, t: f64, rate: f64) {
    if !mix.keyframes.contains_key("gainDb") && !mix.keyframes.contains_key("pan") {
        let g = db_to_gain(mix.gain_db.clamp(kimchi_core::audio::MIN_DB, kimchi_core::audio::MAX_DB)) as f32;
        let [l, r] = pan_gains(mix.pan);
        for f in block {
            f[0] *= g * l;
            f[1] *= g * r;
        }
        return;
    }
    pan_ramp(block, |k| {
        let time = t + k as f64 / rate;
        let g = db_to_gain(mix.gain_db_at(time)) as f32;
        let [l, r] = pan_gains(mix.pan_at(time));
        [g * l, g * r]
    });
}

/// One block of a clip's sound into `v.buf`, for the track's frames `[track_frame, + n)`: its
/// source read `lead` frames ahead (for its effects), channels, gain, fades, effects, pan.
fn play_voice(v: &mut Voice, c: &ClipNode, track_frame: i64, n: usize, realtime: bool, rate: f64, ctx: &ProcessContext) {
    let buf = &mut v.buf[..n];
    buf.fill([0.0; 2]);
    let b = track_frame + v.lead;
    let (lo, hi) = (c.start.max(b), c.end.min(b + n as i64));
    v.cursor = v.cursor.max(b + n as i64);
    if hi > lo {
        if let Some(reader) = v.reader.as_mut().filter(|_| !v.ended) {
            // Frames the reader is behind by are skipped when they come.
            if v.next < lo {
                v.owed += (lo - v.next) as usize;
                v.next = lo;
            }
            let start = v.next.max(lo);
            while v.owed > 0 {
                let mut skip = [[0.0f32; 2]; 256];
                let can = if realtime { v.owed.min(reader.available()) } else { v.owed };
                if can == 0 {
                    break;
                }
                let k = can.min(skip.len());
                let got = reader.read(&mut skip[..k]);
                v.owed -= got;
                if got < k {
                    v.ended = true;
                    break;
                }
            }
            if start < hi {
                let want = (hi - start) as usize;
                let at = (start - b) as usize;
                if v.owed > 0 || v.ended {
                    // Still catching up (or finished): this stretch is silent.
                    v.owed += want;
                } else {
                    let can = if realtime { want.min(reader.available()) } else { want };
                    let got = reader.read(&mut buf[at..at + can]);
                    if got < can {
                        v.ended = true;
                    }
                    v.owed += want - can;
                }
                v.next = hi;
            }
        }
        let clip = &c.heard.clip;
        let region = &mut buf[(lo - b) as usize..(hi - b) as usize];
        match clip.audio.channels {
            Channels::Stereo => {}
            Channels::Mono => region.iter_mut().for_each(|f| *f = [(f[0] + f[1]) * 0.5; 2]),
            Channels::Left => region.iter_mut().for_each(|f| *f = [f[0]; 2]),
            Channels::Right => region.iter_mut().for_each(|f| *f = [f[1]; 2]),
            Channels::Swap => region.iter_mut().for_each(|f| f.swap(0, 1)),
        }
        // Volume (keyframes), fades and the edge ramps.
        let h = &c.heard;
        let keyed = clip.keyframes.contains_key("volume");
        let ramp_frames = EDGE_RAMP * rate;
        let gain = |f: i64| -> f32 {
            let age = (f - c.start) as f64;
            let left = (c.end - f) as f64;
            let mut g = (age / ramp_frames).min(left / ramp_frames).clamp(0.0, 1.0);
            let (age_s, left_s) = (age / rate, left / rate);
            if h.fade_in > 0.0 && age_s < h.fade_in {
                g *= h.fade_in_curve.gain(age_s / h.fade_in);
            }
            if h.fade_out > 0.0 && left_s < h.fade_out {
                g *= h.fade_out_curve.gain(left_s / h.fade_out);
            }
            let volume = if keyed { clip.volume_at(f as f64 / rate) } else { clip.volume.clamp(0.0, 4.0) };
            (g * volume) as f32
        };
        ramp(region, |k| gain(lo + k as i64));
    }
    // The clip's effects (they ring on past its end), then its pan.
    v.chain.process(buf, ctx, None);
    let clip = &c.heard.clip;
    if clip.keyframes.contains_key("pan") {
        let t0 = track_frame as f64 / rate;
        pan_ramp(buf, |k| pan_gains(clip.pan_at(t0 + k as f64 / rate)));
    } else if clip.audio.pan != 0.0 {
        let [l, r] = pan_gains(clip.audio.pan);
        for f in buf.iter_mut() {
            f[0] *= l;
            f[1] *= r;
        }
    }
}

#[cfg(test)]
mod tests;
