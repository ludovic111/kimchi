//! ryolune songs on kimchi's timeline: read a `.ryolune` file, render its mix or one track's stem
//! with ryolune's own engine (so it sounds exactly as in ryolune), and write a kimchi cut as a
//! ryolune session (`handoff.toRyolune` with `as: session`).
//!
//! A kimchi cut becomes one ryolune audio track per kimchi track with sound: each clip's sound
//! (with what ryolune can't do baked in: speed, reverse, pitch, channels, volume keyframes, clip
//! effects, pan) is a source placed at the clip's start, its fades become ryolune fades and its
//! volume the clip gain; the track's effect chain is copied as it is (same insert format), its
//! fader goes through ryolune's fader law and its pan, automation and ducking become ryolune
//! automation lanes. Buses become ryolune bus tracks, the master's effects and limiter go on
//! ryolune's Stereo Out, and markers land on their bars. The tempo is 120 in 4/4, or a ryolune
//! song's when the cut uses one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kimchi_core::anim::{Keyframes, number_at};
use kimchi_core::audio::{FadeCurve, MIN_DB, db_to_gain, gain_to_db};
use kimchi_core::{Beats, Id, Project};
use ryolune_engine::audio::{AudioBuffer, Library};
use ryolune_engine::automation::{AutomationLane, AutomationPoint, AutomationTarget, Interpolation};
use ryolune_engine::export::{Container, ExportOptions, SampleFormat};
use ryolune_engine::model::{self, ClipData, Session};
use serde::Serialize;

use crate::{Frame, Result};

/// What kimchi needs to know about a song.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongInfo {
    pub name: String,
    /// Starting tempo and time signature.
    pub tempo: f64,
    pub beats_per_bar: f64,
    /// Length in seconds (to the end of the last clip, without the effects' tails).
    pub seconds: f64,
    /// Tracks that make sound, as (ryolune track id, name).
    pub tracks: Vec<(String, String)>,
    /// Song markers, as (seconds, name).
    pub markers: Vec<(f64, String)>,
    /// Every beat in seconds, following the song's tempo changes.
    pub beats: Vec<f64>,
}

impl SongInfo {
    /// The song's beats for `Asset.beats` (its own tempo map, not a guess).
    pub fn beats(&self) -> Beats {
        let tempo = if self.beats.len() > 1 {
            60.0 * (self.beats.len() - 1) as f64 / (self.beats[self.beats.len() - 1] - self.beats[0]).max(1e-9)
        } else {
            self.tempo
        };
        Beats { tempo, beats_per_bar: self.beats_per_bar.round().max(1.0) as u32, times: self.beats.clone(), first_downbeat: 0, source: "ryolune".into() }
    }
}

fn describe(path: &Path, s: &Session) -> SongInfo {
    let bpb = s.beats_per_bar();
    let map = s.tempo_map();
    let seconds = s.bars_seconds(0.0, s.end_bar());
    let total_beats = map.beat(seconds).ceil().max(0.0) as usize;
    let name = Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
        path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Song".into())
    });
    SongInfo {
        name,
        tempo: s.transport.tempo,
        beats_per_bar: bpb,
        seconds,
        tracks: s
            .tracks
            .iter()
            .filter(|t| !t.is_bus() && s.clips.iter().any(|c| c.track_id == t.id))
            .map(|t| (t.id.clone(), t.name.clone()))
            .collect(),
        markers: s.markers.iter().map(|m| (map.seconds(m.bar * bpb), m.name.clone())).collect(),
        beats: (0..=total_beats).map(|k| map.seconds(k as f64)).filter(|t| *t <= seconds + 1e-9).collect(),
    }
}

/// Reads a song without rendering it.
pub fn info(path: &Path) -> Result<SongInfo> {
    let (s, _) = ryolune_engine::document::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(describe(path, &s))
}

/// Renders the song's mix (or the stem of `track`, an id or a name) to a WAV file at `rate`
/// (44.1, 48 or 96 kHz; others render at 48 kHz), with three seconds of tail for the effects.
pub fn render(path: &Path, track: Option<&str>, out: &Path, rate: u32) -> Result<SongInfo> {
    let (s, library) = ryolune_engine::document::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = describe(path, &s);
    let options = ExportOptions {
        sample_rate: if [44_100, 48_000, 96_000].contains(&rate) { rate } else { 48_000 },
        format: SampleFormat::Float32,
        dither: false,
        container: Container::Wav,
        ..ExportOptions::default()
    };
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    match track {
        None => {
            ryolune_engine::export::mix(&s, &library, out, &options)?;
        }
        Some(wanted) => {
            let t = s
                .tracks
                .iter()
                .find(|t| t.id == wanted)
                .or_else(|| s.tracks.iter().find(|t| t.name.eq_ignore_ascii_case(wanted)))
                .ok_or_else(|| {
                    let names: Vec<&str> = s.tracks.iter().map(|t| t.name.as_str()).collect();
                    format!("{} has no track `{wanted}`. Tracks: {}.", info.name, names.join(", "))
                })?;
            // ryolune writes stems into a new folder: one file, then moved where it belongs.
            let scratch = tempfile::Builder::new().prefix(".kimchi-stem-").tempdir_in(out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
            let dir = scratch.path().join("stem");
            let report = ryolune_engine::export::stems(&s, &library, &dir, &options, Some(std::slice::from_ref(&t.id)), true, false)?;
            let file = report.files.first().map(|f| dir.join(f.path.file_name().unwrap_or_default())).ok_or("ryolune wrote no stem")?;
            std::fs::rename(&file, out).or_else(|_| std::fs::copy(&file, out).map(|_| ())).map_err(|e| e.to_string())?;
        }
    }
    Ok(info)
}

/// One clip's sound for a session, at the session's rate, with everything ryolune can't do
/// already applied. `gain_db` and the fades are left to ryolune.
#[derive(Debug, Clone)]
pub struct ClipSound {
    pub frames: Vec<Frame>,
    /// Timeline seconds where it starts (transitions start a clip early).
    pub start: f64,
    pub gain_db: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub curve: FadeCurve,
}

/// Everything [`write_session`] needs besides the project.
#[derive(Debug, Clone, Default)]
pub struct SessionSounds {
    pub rate: u32,
    /// Each clip's sound, by clip id.
    pub clips: HashMap<Id, ClipSound>,
    /// How far each ducked track was pulled down over time: (seconds, dB) readings.
    pub ducking: HashMap<Id, Vec<(f64, f64)>>,
}

/// The ryolune fader position (0..1, 0.75 = 0 dB, 1 = +6 dB) for `db`: `model::fader_gain`
/// inverted.
pub fn fader_position(db: f64) -> f32 {
    if db <= MIN_DB {
        return 0.0;
    }
    let gain = db_to_gain(db);
    let v = if gain >= 1.0 { 0.75 + db / 24.0 } else { 0.75 * gain.powf(1.0 / 1.6) };
    v.clamp(0.0, 1.0) as f32
}

const COLORS: [&str; 8] =
    ["oklch(0.72 0.14 40)", "oklch(0.72 0.13 300)", "oklch(0.74 0.12 160)", "oklch(0.76 0.13 85)", "oklch(0.70 0.14 250)", "oklch(0.72 0.14 10)", "oklch(0.75 0.11 200)", "oklch(0.73 0.12 130)"];

fn insert(i: &kimchi_core::Insert) -> model::Insert {
    model::Insert {
        name: i.name.clone(),
        state: i.state.clone(),
        meta: i.meta.clone(),
        id: i.id.clone(),
        plugin: i.plugin.clone(),
        params: i.params.clone(),
        blob: i.blob.clone(),
    }
}

fn curve(c: FadeCurve) -> model::FadeCurve {
    match c {
        FadeCurve::Linear => model::FadeCurve::Linear,
        FadeCurve::Exponential => model::FadeCurve::Exponential,
        FadeCurve::EqualPower | FadeCurve::SCurve => model::FadeCurve::EqualPower,
    }
}

/// The tempo and beats per bar a session of `project` uses: a ryolune song's in it, else 120 in 4/4.
fn tempo_of(project: &Project) -> (f64, u32) {
    project
        .assets
        .iter()
        .find_map(|a| match &a.origin {
            kimchi_core::AssetOrigin::Song(s) if ryolune_engine::tempo::valid_bpm(s.tempo) => Some((s.tempo, s.beats_per_bar.round().clamp(1.0, 32.0) as u32)),
            _ => None,
        })
        .unwrap_or((120.0, 4))
}

/// An automation lane from `keys[name]` at timeline times, its values through `map`, sampled
/// finely where the keyframes ease (ryolune's lanes are straight lines between points).
fn lane(id: String, name: String, target: AutomationTarget, keys: &Keyframes, key: &str, beat: impl Fn(f64) -> f64, (min, max): (f64, f64), map: impl Fn(f64) -> f64) -> Option<AutomationLane> {
    let list = keys.get(key).filter(|l| !l.is_empty())?;
    let mut times: Vec<f64> = vec![];
    for (i, k) in list.iter().enumerate() {
        if i > 0 {
            let a = list[i - 1].time;
            let steps = ((k.time - a) / 0.05).ceil().clamp(1.0, 200.0) as usize;
            if !matches!(k.easing, kimchi_core::anim::Easing::Linear) {
                times.extend((1..steps).map(|s| a + (k.time - a) * s as f64 / steps as f64));
            }
        }
        times.push(k.time);
    }
    let mut points: Vec<AutomationPoint> = vec![];
    for t in times.into_iter().filter(|t| *t >= 0.0) {
        let b = beat(t);
        if points.last().is_some_and(|p| b <= p.beat + 1e-6) {
            continue;
        }
        let v = number_at(keys, key, t).map(&map)?.clamp(min, max);
        points.push(AutomationPoint { id: format!("{id}-{}", points.len() + 1), beat: b, value: v });
    }
    let first = points.first()?.value;
    Some(AutomationLane { id, name, target, min, max, manual_value: first, interpolation: Interpolation::Linear, enabled: true, points })
}

/// The session for `project` with `sounds` (see the module's notes), and its audio library.
pub fn session(project: &Project, sounds: &SessionSounds) -> Result<(Session, Library)> {
    let rate = sounds.rate;
    let (tempo, bpb) = tempo_of(project);
    let seconds_per_bar = 60.0 / tempo * bpb as f64;
    let bar = |t: f64| (t / seconds_per_bar).max(0.0);
    let beat = |t: f64| (t * tempo / 60.0).max(0.0);
    let mut s = ryolune_engine::store::empty();
    s.name = format!("{}.ryolune", project.name);
    s.tracks.clear();
    s.clips.clear();
    s.sources.clear();
    s.strips.clear();
    s.automation.clear();
    s.markers.clear();
    s.tempo_changes.clear();
    s.transport.tempo = tempo;
    s.transport.time_signature = model::TimeSignature { numerator: bpb, denominator: 4 };
    s.transport.position_beats = 0.0;
    s.transport.cycle = false;
    s.view.selected_track_id = None;
    let mut library: Library = HashMap::new();
    let mut lanes: Vec<AutomationLane> = vec![];
    let track_key = |k: usize| format!("kimchi-{}", k + 1);
    let bus_key = |id: &Id| format!("kimchi-bus-{}", id.simple());

    // Buses first (tracks name them), as ryolune bus tracks.
    for (i, b) in project.mixer.buses.iter().enumerate() {
        let key = bus_key(&b.id);
        s.tracks.push(model::Track {
            id: key.clone(),
            name: b.name.clone(),
            color: COLORS[(i + 3) % COLORS.len()].into(),
            armed: false,
            monitor: model::Monitor::Off,
            extra: HashMap::new(),
            kind: "bus".into(),
            volume: fader_position(b.mix.gain_db),
            pan: (b.mix.pan * 100.0) as f32,
            mute: b.muted,
            solo: b.mix.solo,
            output: None,
        });
        s.strips.insert(key.clone(), model::Strip { inserts: b.mix.effects.iter().map(insert).collect(), ..Default::default() });
        add_lanes(&mut lanes, &key, &b.name, &b.mix.keyframes, &b.mix.effects, &beat, None);
    }
    let mut n = 0;
    for track in &project.tracks {
        let heard: Vec<&kimchi_core::Clip> = track.clips.iter().filter(|c| sounds.clips.contains_key(&c.id)).collect();
        if heard.is_empty() {
            continue;
        }
        let key = track_key(n);
        let name = track.name.clone();
        s.tracks.push(model::Track {
            id: key.clone(),
            name: name.clone(),
            color: COLORS[n % COLORS.len()].into(),
            armed: false,
            monitor: model::Monitor::Off,
            extra: HashMap::new(),
            kind: "audio".into(),
            volume: fader_position(track.mix.gain_db),
            pan: (track.mix.pan * 100.0) as f32,
            mute: track.muted,
            solo: track.mix.solo,
            output: track.mix.output.as_ref().filter(|o| project.mixer.bus(**o).is_some()).map(bus_key),
        });
        let sends = track
            .mix
            .sends
            .iter()
            .filter(|x| project.mixer.bus(x.bus).is_some())
            .map(|x| model::Send { level_db: Some(x.level_db.clamp(-100.0, 0.0) as f32), name: project.mixer.bus(x.bus).map(|b| b.name.clone()).unwrap_or_default(), bus: Some(bus_key(&x.bus)) })
            .collect();
        s.strips.insert(key.clone(), model::Strip { inserts: track.mix.effects.iter().map(insert).collect(), sends, ..Default::default() });
        add_lanes(&mut lanes, &key, &name, &track.mix.keyframes, &track.mix.effects, &beat, sounds.ducking.get(&track.id).map(|d| (d.as_slice(), track.mix.gain_db)));
        for c in heard {
            let sound = &sounds.clips[&c.id];
            if sound.frames.is_empty() {
                continue;
            }
            let source = format!("src-{}", c.id.simple());
            let buffer = AudioBuffer::new(rate, sound.frames.clone()).map_err(|e| format!("{}: {e}", c.name))?;
            let seconds = buffer.duration();
            s.sources.insert(
                source.clone(),
                model::Source {
                    id: source.clone(),
                    name: c.name.clone(),
                    sample_rate: rate,
                    channels: 2,
                    file_name: Some(format!("{}.wav", c.name)),
                    duration_seconds: seconds,
                    origin: "file".into(),
                    seed: None,
                    wave_kind: None,
                },
            );
            library.insert(source.clone(), Arc::new(buffer));
            let (fade_in, fade_out) = model::clamp_fades(sound.fade_in, sound.fade_out, seconds);
            s.clips.push(model::Clip {
                id: format!("clip-{}", c.id.simple()),
                name: c.name.clone(),
                agent: false,
                track_id: key.clone(),
                start_bar: bar(sound.start),
                length_bars: (seconds / seconds_per_bar).max(1e-6),
                data: ClipData::Audio {
                    source_id: source,
                    offset_seconds: 0.0,
                    fade_in,
                    fade_out,
                    fade_curve: curve(sound.curve),
                    gain_db: sound.gain_db.clamp(model::CLIP_GAIN_MIN_DB as f64, model::CLIP_GAIN_MAX_DB as f64) as f32,
                },
            });
        }
        n += 1;
    }
    // The master: its effects, then kimchi's limiter as ryolune's (sample peaks, at the ceiling).
    let master = &project.mixer.master;
    let mut inserts: Vec<model::Insert> = master.effects.iter().map(insert).collect();
    if master.limiter && inserts.len() < model::MAX_INSERTS {
        let mut limiter = model::Insert::new("kimchi-limiter".into(), "stock:Limiter", "Limiter");
        if let Ok(p) = crate::plugins::param("stock:Limiter", "Ceiling") {
            limiter.params.insert(p.id, master.ceiling_db.clamp(p.min, p.max));
        }
        inserts.push(limiter);
    }
    s.strips.insert(model::MASTER.into(), model::Strip { inserts, ..Default::default() });
    s.master_volume = fader_position(master.gain_db);
    if let Some(l) = lane("kimchi-master-volume".into(), "Stereo Out volume".into(), AutomationTarget::MasterVolume, &master.keyframes, "gainDb", &beat, (0.0, 1.0), |db| fader_position(db) as f64) {
        lanes.push(l);
    }
    add_plugin_lanes(&mut lanes, model::MASTER, &master.keyframes, &master.effects, &beat);
    s.markers = project
        .markers
        .iter()
        .take(model::MAX_MARKERS)
        .map(|m| model::Marker { id: format!("marker-{}", m.id.simple()), bar: bar(m.time), name: m.label.chars().take(120).collect(), color: Some(m.color.clone()).filter(|c| !c.is_empty()) })
        .collect();
    s.markers.sort_by(|a, b| a.bar.total_cmp(&b.bar));
    s.automation = lanes;
    s.normalize();
    s.validate().map_err(|e| format!("the session isn't valid for ryolune: {e}"))?;
    Ok((s, library))
}

/// Track automation (fader, pan, effect parameters, and the ducking folded into the fader).
fn add_lanes(
    lanes: &mut Vec<AutomationLane>, key: &str, name: &str, keys: &Keyframes, effects: &[kimchi_core::Insert], beat: &impl Fn(f64) -> f64,
    ducking: Option<(&[(f64, f64)], f64)>,
) {
    match ducking.filter(|(d, _)| !d.is_empty()) {
        Some((readings, fader)) => {
            // The fader at each reading, pulled down by the ducking.
            let mut points: Vec<AutomationPoint> = vec![];
            for &(t, duck) in readings {
                let b = beat(t);
                if points.last().is_some_and(|p| b <= p.beat + 1e-6) {
                    continue;
                }
                let db = number_at(keys, "gainDb", t).unwrap_or(fader) + duck;
                let value = fader_position(db) as f64;
                // Only where it moves, to keep the lane light.
                if points.len() >= 2 && (points[points.len() - 1].value - value).abs() < 1e-4 && (points[points.len() - 2].value - value).abs() < 1e-4 {
                    points.last_mut().expect("two").beat = b;
                    continue;
                }
                points.push(AutomationPoint { id: format!("{key}-vol-{}", points.len() + 1), beat: b, value });
            }
            if let Some(first) = points.first().map(|p| p.value) {
                lanes.push(AutomationLane {
                    id: format!("{key}-volume"),
                    name: format!("{name} volume"),
                    target: AutomationTarget::TrackVolume { track_id: key.into() },
                    min: 0.0,
                    max: 1.0,
                    manual_value: first,
                    interpolation: Interpolation::Linear,
                    enabled: true,
                    points,
                });
            }
        }
        None => {
            let target = AutomationTarget::TrackVolume { track_id: key.into() };
            lanes.extend(lane(format!("{key}-volume"), format!("{name} volume"), target, keys, "gainDb", beat, (0.0, 1.0), |db| fader_position(db) as f64));
        }
    }
    let target = AutomationTarget::TrackPan { track_id: key.into() };
    lanes.extend(lane(format!("{key}-pan"), format!("{name} pan"), target, keys, "pan", beat, (-100.0, 100.0), |p| p * 100.0));
    add_plugin_lanes(lanes, key, keys, effects, beat);
}

fn add_plugin_lanes(lanes: &mut Vec<AutomationLane>, key: &str, keys: &Keyframes, effects: &[kimchi_core::Insert], beat: &impl Fn(f64) -> f64) {
    for name in keys.keys() {
        let Some((slot, param)) = kimchi_core::audio::parse_effect_key(name) else { continue };
        let Some(e) = effects.iter().find(|e| e.id == slot && !e.is_empty()) else { continue };
        let plugin = e.plugin_id();
        let Ok(info) = crate::plugins::params(&plugin).map(|ps| ps.into_iter().find(|p| p.id == param)) else { continue };
        let Some(info) = info.filter(|p| p.max > p.min) else { continue };
        let target = AutomationTarget::PluginParameter { track_id: key.into(), insert_id: e.id.clone(), plugin_id: plugin, parameter_id: param };
        lanes.extend(lane(format!("{key}-{slot}-{param}"), format!("{} {}", e.name, info.name), target, keys, name, beat, (info.min, info.max), |v| v));
    }
}

/// Writes the session for `project` (see [`session`]) to `path` with ryolune's own `save`, so
/// ryolune opens it as it is. Returns the file written.
pub fn write_session(project: &Project, sounds: &SessionSounds, path: &Path) -> Result<PathBuf> {
    let (s, library) = session(project, sounds)?;
    let path = if ryolune_engine::document::is_session_path(path) { path.to_path_buf() } else { path.with_extension(ryolune_engine::document::EXTENSION) };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    ryolune_engine::document::save(&s, &library, &path)?;
    Ok(path)
}

/// The clip gain (dB) for a kimchi clip volume, and whether its volume keyframes must be baked
/// into the sound instead.
pub fn clip_gain(clip: &kimchi_core::Clip) -> (f64, bool) {
    if clip.keyframes.contains_key("volume") {
        (0.0, true)
    } else {
        (gain_to_db(clip.volume.clamp(0.0, 4.0)), false)
    }
}

/// Counts what a session would hold (for errors before decoding everything): sources' decoded
/// bytes against ryolune's 1 GiB limit.
pub fn fits(seconds: f64, rate: u32) -> Result<()> {
    let bytes = seconds * rate as f64 * 8.0;
    if bytes > ryolune_engine::audio::MAX_LIBRARY_BYTES as f64 {
        return Err(format!("{:.0} minutes of sound is more than one ryolune session holds (about 45 minutes at 48 kHz); send a shorter range", seconds / 60.0));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
