//! Sound: how clips and tracks are mixed, the buses and the master, and songs from ryolune.
//!
//! Every sample comes from kimchi's mixer (`kimchi-audio`), for the preview and the export
//! alike: ffmpeg only decodes a clip's sound and encodes the finished mix. Effects are slots in
//! ryolune's own format ([`Insert`]) and are played by ryolune's engine inside kimchi, so a chain
//! moves between the two apps unchanged: the same stock effects (`stock:Channel EQ`) and the
//! same CLAP, VST3, Audio Unit and ryolune native plugins.
//!
//! Levels are decibels (0 = as recorded); pans go from -1 (left) to 1 (right). Everything here
//! is left out of the project file while it is at its default, so projects that don't use it
//! read and write exactly as before (ryolune writes kimchi projects too, with only the old
//! fields: see its `lsuite::place_on_kimchi`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::anim::{Keyframes, number_at};
use crate::model::Id;

/// Quietest level a fader or send goes to; at or below it the sound is off.
pub const MIN_DB: f64 = -96.0;
/// Loudest a track, bus or the master can be pushed.
pub const MAX_DB: f64 = 12.0;
/// Most effects in one chain (ryolune's `MAX_INSERTS`).
pub const MAX_EFFECTS: usize = 8;
/// Most sends on one track.
pub const MAX_SENDS: usize = 4;
/// Most buses in a project.
pub const MAX_BUSES: usize = 16;
/// Pitch shift range, in semitones.
pub const MAX_PITCH: f64 = 24.0;

/// Gain factor for `db` decibels (0 at or below [`MIN_DB`]).
pub fn db_to_gain(db: f64) -> f64 {
    if db <= MIN_DB { 0.0 } else { 10f64.powf(db / 20.0) }
}

/// Decibels for a gain factor ([`MIN_DB`] for silence).
pub fn gain_to_db(gain: f64) -> f64 {
    if gain <= 0.0 { MIN_DB } else { (20.0 * gain.log10()).max(MIN_DB) }
}

/// One effect slot, exactly as ryolune stores an insert, so a chain copies between the apps.
/// `plugin` is a ryolune descriptor id: `stock:<name>`, `native:<id>`, `clap:<id>`,
/// `vst3:<class id>` or `au:<type>:<subtype>:<manufacturer>`; when it is empty the stock effect
/// named by `name` is meant. `params` are the plugin's own parameter ids with plain values
/// (dB, Hz, %, ms); `blob` is the plugin's saved state (base64), for external plugins.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Insert {
    pub name: String,
    /// `active`, `bypassed` or `empty`.
    pub state: String,
    #[serde(default)]
    pub meta: String,
    /// The slot's id: stable while the effect stays in its chain (keyframes name it).
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plugin: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<u32, f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blob: String,
}

impl Insert {
    /// A running effect with its default settings.
    pub fn new(id: impl Into<String>, plugin: &str, name: &str) -> Self {
        Self { name: name.into(), state: "active".into(), id: id.into(), plugin: plugin.into(), ..Default::default() }
    }

    /// The descriptor id of the plugin (`stock:<name>` when only a name is stored).
    pub fn plugin_id(&self) -> String {
        if self.plugin.is_empty() { format!("stock:{}", self.name) } else { self.plugin.clone() }
    }

    pub fn is_active(&self) -> bool {
        self.state == "active"
    }

    pub fn is_bypassed(&self) -> bool {
        self.state == "bypassed"
    }

    pub fn is_empty(&self) -> bool {
        self.state == "empty"
    }
}

/// The keyframe name of one effect parameter: `effects.<slot id>.<parameter id>`.
pub fn effect_key(slot: &str, param: u32) -> String {
    format!("effects.{slot}.{param}")
}

/// `(slot id, parameter id)` of an `effects.<slot>.<param>` keyframe name.
pub fn parse_effect_key(name: &str) -> Option<(&str, u32)> {
    let rest = name.strip_prefix("effects.")?;
    let (slot, param) = rest.rsplit_once('.')?;
    Some((slot, param.parse().ok()?))
}

/// The shape of a clip's fades (fade-outs mirror fade-ins) and of crossfades at transitions.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FadeCurve {
    /// A straight line: kimchi's fades until 0.8.
    #[default]
    Linear,
    /// A quarter sine: crossfades keep their loudness (ryolune's default).
    EqualPower,
    /// Slow start, fast finish: sounds even to the ear on long fades in.
    Exponential,
    /// Eased at both ends.
    SCurve,
}

impl FadeCurve {
    pub const ALL: [FadeCurve; 4] = [Self::Linear, Self::EqualPower, Self::Exponential, Self::SCurve];

    /// Gain at `x` from 0 (start of a fade-in) to 1 (full level).
    pub fn gain(self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Self::Linear => x,
            Self::EqualPower => (x * std::f64::consts::FRAC_PI_2).sin(),
            Self::Exponential => x * x * x,
            Self::SCurve => x * x * (3.0 - 2.0 * x),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::EqualPower => "equalPower",
            Self::Exponential => "exponential",
            Self::SCurve => "sCurve",
        }
    }
}

/// Which of a source's channels a clip plays.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Channels {
    /// As recorded (sources with more than two channels are folded down to stereo).
    #[default]
    Stereo,
    /// Both channels summed to the middle (one-sided recordings, phone videos).
    Mono,
    /// Only the left channel, in the middle (a lavalier on one side of a camera's input).
    Left,
    /// Only the right channel, in the middle.
    Right,
    /// Left and right swapped.
    Swap,
}

/// A clip's sound beyond its volume and fades (those stay on [`crate::Clip`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipAudio {
    /// -1 (left) to 1 (right); keyframable as `pan`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pan: f64,
    #[serde(default, skip_serializing_if = "is_default")]
    pub fade_curve: FadeCurve,
    #[serde(default, skip_serializing_if = "is_default")]
    pub channels: Channels,
    /// Pitch shift in semitones, without changing the speed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pitch: f64,
    /// A speed change keeps the pitch (true, the default) or plays like tape (false: faster is
    /// higher).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub preserve_pitch: bool,
    /// The clip's sound is off while its picture plays (a video clip's own sound).
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// Effects on this clip alone, before its track's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Insert>,
}

impl Default for ClipAudio {
    fn default() -> Self {
        Self { pan: 0.0, fade_curve: FadeCurve::Linear, channels: Channels::Stereo, pitch: 0.0, preserve_pitch: true, muted: false, effects: vec![] }
    }
}

impl ClipAudio {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A send: part of a track's sound also goes to a bus (a shared reverb, a dialogue group).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Send {
    pub bus: Id,
    #[serde(default)]
    pub level_db: f64,
    /// Taken before the track's fader and pan (true) or after them (false, the default).
    #[serde(default, skip_serializing_if = "is_false")]
    pub pre_fader: bool,
}

/// Automatic ducking: the track's level drops while other tracks speak (music under dialogue).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Duck {
    /// The tracks that push this one down; empty means every track that isn't ducked itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub under: Vec<Id>,
    /// How far down it goes, in dB (negative).
    #[serde(default = "duck_amount")]
    pub amount_db: f64,
    /// Level above which the keying tracks count as sounding, in dBFS.
    #[serde(default = "duck_threshold")]
    pub threshold_db: f64,
    /// Seconds to go down, and to come back up once they stop.
    #[serde(default = "duck_attack")]
    pub attack: f64,
    #[serde(default = "duck_release")]
    pub release: f64,
}

impl Default for Duck {
    fn default() -> Self {
        Self { under: vec![], amount_db: duck_amount(), threshold_db: duck_threshold(), attack: duck_attack(), release: duck_release() }
    }
}

fn duck_amount() -> f64 {
    -12.0
}
fn duck_threshold() -> f64 {
    -40.0
}
fn duck_attack() -> f64 {
    0.15
}
fn duck_release() -> f64 {
    0.6
}

fn check_level(what: &str, db: f64) -> Result<(), String> {
    if !db.is_finite() || db > MAX_DB + 1e-9 {
        return Err(format!("{what} must be a number up to {MAX_DB} dB"));
    }
    Ok(())
}

fn check_pan(pan: f64) -> Result<(), String> {
    if !(-1.0..=1.0).contains(&pan) {
        return Err("pan goes from -1 (left) to 1 (right)".into());
    }
    Ok(())
}

fn check_chain(effects: &[Insert]) -> Result<(), String> {
    if effects.len() > MAX_EFFECTS {
        return Err(format!("a chain holds at most {MAX_EFFECTS} effects"));
    }
    for (i, e) in effects.iter().enumerate() {
        if e.id.is_empty() {
            return Err(format!("effect {} has no slot id", i + 1));
        }
        if effects[..i].iter().any(|o| o.id == e.id) {
            return Err(format!("two effects share the slot id `{}`", e.id));
        }
        if !["active", "bypassed", "empty"].contains(&e.state.as_str()) {
            return Err(format!("effect state `{}` isn't active, bypassed or empty", e.state));
        }
        if e.params.values().any(|v| !v.is_finite()) {
            return Err(format!("{}: parameter values must be numbers", e.name));
        }
    }
    Ok(())
}

impl ClipAudio {
    /// Ranges and effect slots are sound.
    pub fn check(&self) -> Result<(), String> {
        check_pan(self.pan)?;
        if !self.pitch.is_finite() || self.pitch.abs() > MAX_PITCH {
            return Err(format!("pitch goes from -{MAX_PITCH} to {MAX_PITCH} semitones"));
        }
        check_chain(&self.effects)
    }
}

/// How a track (or a bus) sounds in the mix.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackMix {
    /// Fader, in dB.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain_db: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pan: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub solo: bool,
    /// Effects after the clips' own, before the fader.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Insert>,
    /// The bus it feeds (`None`: the master). Buses always feed the master.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<Send>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duck: Option<Duck>,
    /// Automation, at timeline times (seconds): `gainDb`, `pan` and
    /// `effects.<slot id>.<parameter id>` ([`effect_key`]).
    #[serde(default, skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
    /// Armed for recording a voice-over take.
    #[serde(default, skip_serializing_if = "is_false")]
    pub armed: bool,
}

impl TrackMix {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Ranges are sound, effect slots unique, and the buses it names exist.
    pub fn check(&self, buses: &[Bus]) -> Result<(), String> {
        check_level("gainDb", self.gain_db)?;
        check_pan(self.pan)?;
        check_chain(&self.effects)?;
        let known = |id: &Id| buses.iter().any(|b| b.id == *id);
        if let Some(o) = &self.output.filter(|o| !known(o)) {
            return Err(format!("there is no bus {o}"));
        }
        if self.sends.len() > MAX_SENDS {
            return Err(format!("a track has at most {MAX_SENDS} sends"));
        }
        for s in &self.sends {
            if !known(&s.bus) {
                return Err(format!("there is no bus {}", s.bus));
            }
            check_level("send level", s.level_db)?;
        }
        if let Some(d) = &self.duck {
            if !(d.amount_db.is_finite() && d.amount_db <= 0.0 && d.amount_db >= MIN_DB) {
                return Err("ducking amount is a negative number of dB".into());
            }
            if !(d.attack.is_finite() && d.release.is_finite() && d.attack >= 0.0 && d.release >= 0.0 && d.attack <= 10.0 && d.release <= 10.0) {
                return Err("ducking attack and release are 0 to 10 seconds".into());
            }
            if !d.threshold_db.is_finite() {
                return Err("ducking threshold must be a number".into());
            }
        }
        for name in self.keyframes.keys() {
            if name != "gainDb" && name != "pan" && parse_effect_key(name).is_none_or(|(slot, _)| !self.effects.iter().any(|e| e.id == slot)) {
                return Err(format!("tracks can't automate `{name}`: gainDb, pan or effects.<slot id>.<parameter id>"));
            }
        }
        Ok(())
    }

    /// Fader level at timeline time `t`, in dB (automation applied).
    pub fn gain_db_at(&self, t: f64) -> f64 {
        number_at(&self.keyframes, "gainDb", t).unwrap_or(self.gain_db).clamp(MIN_DB, MAX_DB)
    }

    /// Pan at timeline time `t` (automation applied).
    pub fn pan_at(&self, t: f64) -> f64 {
        number_at(&self.keyframes, "pan", t).unwrap_or(self.pan).clamp(-1.0, 1.0)
    }

    /// An effect parameter's value at timeline time `t`: its automation, else its stored value.
    pub fn param_at(&self, slot: &str, param: u32, t: f64) -> Option<f64> {
        number_at(&self.keyframes, &effect_key(slot, param), t)
            .or_else(|| self.effects.iter().find(|e| e.id == slot).and_then(|e| e.params.get(&param).copied()))
    }
}

/// A bus: tracks feed it (their `output`) or send to it, it runs its own effects and fader,
/// and it feeds the master.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bus {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// Its fader, pan, solo, effects and automation (`output` and `sends` are not used).
    #[serde(default, skip_serializing_if = "TrackMix::is_default")]
    pub mix: TrackMix,
}

/// The master: everything ends here before the speakers or the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Master {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain_db: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Insert>,
    /// Automation of `gainDb` and `effects.<slot>.<param>`, at timeline times.
    #[serde(default, skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
    /// A true-peak limiter at the very end keeps the mix from clipping.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub limiter: bool,
    /// The limiter's ceiling, in dBTP.
    #[serde(default = "ceiling", skip_serializing_if = "is_ceiling")]
    pub ceiling_db: f64,
    /// Exports are brought to this integrated loudness (LUFS, e.g. -14 for YouTube and
    /// streaming, -16 for podcasts, -23 for broadcast). `None`: as mixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness: Option<f64>,
}

impl Default for Master {
    fn default() -> Self {
        Self { gain_db: 0.0, effects: vec![], keyframes: Keyframes::new(), limiter: true, ceiling_db: ceiling(), loudness: None }
    }
}

fn ceiling() -> f64 {
    -1.0
}
fn is_ceiling(v: &f64) -> bool {
    *v == ceiling()
}

/// The project's mixer beyond its tracks: the buses and the master.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Mixer {
    #[serde(default, skip_serializing_if = "is_default")]
    pub master: Master,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<Bus>,
}

impl Mixer {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn bus(&self, id: Id) -> Option<&Bus> {
        self.buses.iter().find(|b| b.id == id)
    }

    /// Ranges are sound and bus ids unique.
    pub fn check(&self) -> Result<(), String> {
        let m = &self.master;
        check_level("master gainDb", m.gain_db)?;
        check_chain(&m.effects)?;
        if !(m.ceiling_db.is_finite() && (-24.0..=0.0).contains(&m.ceiling_db)) {
            return Err("the limiter's ceiling goes from -24 to 0 dBTP".into());
        }
        if m.loudness.is_some_and(|l| !(l.is_finite() && (-40.0..=-5.0).contains(&l))) {
            return Err("loudness targets go from -40 to -5 LUFS".into());
        }
        if self.buses.len() > MAX_BUSES {
            return Err(format!("a project has at most {MAX_BUSES} buses"));
        }
        for (i, b) in self.buses.iter().enumerate() {
            if self.buses[..i].iter().any(|o| o.id == b.id) {
                return Err(format!("two buses share the id {}", b.id));
            }
            let mut mix = b.mix.clone();
            mix.output = None;
            mix.sends.clear();
            mix.check(&[]).map_err(|e| format!("bus {}: {e}", b.name))?;
        }
        Ok(())
    }
}

/// An asset made from a ryolune song: kimchi plays the rendered file and renders the song again
/// when it has been saved since (`audio.refreshSongs`, and when the project opens).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SongRef {
    /// The `.ryolune` file.
    pub song: String,
    /// The song file's modification time when it was rendered (RFC 3339).
    #[serde(default)]
    pub song_modified: Option<String>,
    /// One ryolune track's stem (its id) instead of the whole mix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// The song's name and, for stems, the track's.
    #[serde(default)]
    pub title: String,
    /// The song's starting tempo (beats per minute) and time signature.
    #[serde(default)]
    pub tempo: f64,
    #[serde(default)]
    pub beats_per_bar: f64,
}

/// Where the beats of a piece of music fall, to cut on them and draw them on the timeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Beats {
    /// Beats per minute (the average, for music that drifts).
    pub tempo: f64,
    #[serde(default = "four")]
    pub beats_per_bar: u32,
    /// Seconds into the source of every beat, in order.
    pub times: Vec<f64>,
    /// Index in `times` of the first downbeat (the first beat of a bar).
    #[serde(default)]
    pub first_downbeat: usize,
    /// `detected` (kimchi listened to it) or `ryolune` (the song's own tempo map).
    #[serde(default)]
    pub source: String,
}

fn four() -> u32 {
    4
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_stay_out_of_the_file() {
        assert_eq!(serde_json::to_string(&ClipAudio::default()).unwrap(), "{}");
        assert_eq!(serde_json::to_string(&TrackMix::default()).unwrap(), "{}");
        assert_eq!(serde_json::to_string(&Mixer::default()).unwrap(), "{}");
        let back: Master = serde_json::from_str("{}").unwrap();
        assert!(back.limiter);
        assert_eq!(back.ceiling_db, -1.0);
        let back: ClipAudio = serde_json::from_str("{}").unwrap();
        assert!(back.preserve_pitch);
    }

    #[test]
    fn inserts_read_as_ryolune_writes_them() {
        let json = r#"{"name":"Channel EQ","state":"active","meta":"","id":"t1/0","plugin":"stock:Channel EQ","params":{"0":-3.5,"4":1200.0}}"#;
        let i: Insert = serde_json::from_str(json).unwrap();
        assert_eq!(i.plugin_id(), "stock:Channel EQ");
        assert_eq!(i.params[&4], 1200.0);
        assert_eq!(serde_json::to_string(&i).unwrap(), json);
        let bare: Insert = serde_json::from_str(r#"{"name":"Space","state":"bypassed"}"#).unwrap();
        assert_eq!(bare.plugin_id(), "stock:Space");
        assert!(bare.is_bypassed());
    }

    #[test]
    fn effect_keys_round_trip() {
        let k = effect_key("a1b2", 7);
        assert_eq!(parse_effect_key(&k), Some(("a1b2", 7)));
        assert_eq!(parse_effect_key("effects.x"), None);
        assert_eq!(parse_effect_key("gainDb"), None);
    }

    #[test]
    fn curves_start_silent_and_end_full() {
        for c in FadeCurve::ALL {
            assert_eq!(c.gain(0.0), 0.0);
            assert!((c.gain(1.0) - 1.0).abs() < 1e-12);
            assert!(c.gain(0.5) > 0.0 && c.gain(0.5) < 1.0);
        }
        assert!((db_to_gain(-6.0) - 0.501).abs() < 1e-3);
        assert!((gain_to_db(db_to_gain(-12.0)) + 12.0).abs() < 1e-9);
        assert_eq!(db_to_gain(MIN_DB), 0.0);
    }
}
