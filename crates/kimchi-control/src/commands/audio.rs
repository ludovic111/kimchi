//! `audio.*`: the mix, for people and agents alike.
//!
//! Every edit is one undo step through the project model (`UpdateTrack { mix }`,
//! `UpdateClip { audio }`, `SetMixer`); slider and fader drags send a coalesce key. Effects are
//! slots in ryolune's insert format, so a chain moves between kimchi and ryolune unchanged;
//! their parameters are named and valued as ryolune shows them ("-6.0 dB", "Hall"), and typed
//! values are read the same way. Analysis (loudness, beats) and ryolune songs go through
//! `kimchi-audio` and `kimchi_media::audio`. The live commands (meters, devices, recording,
//! the mixer view) are carried out by the window.
//!
//! Wherever a command takes a `target`, it is a track, a bus or a clip by id or name, or
//! `master`; `track:`, `bus:` and `clip:` prefixes settle names that clash.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kimchi_audio::plugins::{self, ParamInfo};
use kimchi_core::audio::{
    Beats, Bus, Channels, Duck, FadeCurve, Insert, MAX_BUSES, MAX_DB, MAX_EFFECTS, MIN_DB, Mixer, Send, SongRef, TrackMix, db_to_gain, effect_key,
    gain_to_db, parse_effect_key,
};
use kimchi_core::{Asset, AssetOrigin, ClipPatch, Easing, Edit, Id, KeyValue, Keyframe, Keyframes, MediaKind, Project, TrackKind, TrackPatch, new_id};
use serde_json::{Map, Value, json};

use crate::commands::clip::apply_all;
use crate::commands::project::round;
use crate::registry::{Args, Ctx, closest};
use crate::resolve;
use crate::session::{CmdResult, Session, err};

/// Loudness targets people know by name (LUFS).
pub const LOUDNESS_PRESETS: &[(&str, f64)] = &[("youtube", -14.0), ("streaming", -14.0), ("spotify", -14.0), ("podcast", -16.0), ("apple", -16.0), ("broadcast", -23.0), ("ebu", -23.0), ("tv", -23.0)];

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "audio.overview" => Ok(overview(&s.project()?)),
        "audio.setTrack" => set_track(s, cx, &a),
        "audio.setClip" => set_clip(s, cx, &a),
        "audio.addBus" => add_bus(s, cx, &a),
        "audio.removeBus" => {
            let p = s.project()?;
            let id = bus(&p, a.str("busId")?)?;
            let mut mixer = p.mixer.clone();
            mixer.buses.retain(|b| b.id != id);
            s.apply(cx.label(), cx.source, &Edit::SetMixer { mixer }, None)?;
            Ok(json!({ "removed": id }))
        }
        "audio.setBus" => set_bus(s, cx, &a),
        "audio.setSend" | "audio.removeSend" => set_send(s, cx, &a),
        "audio.setMaster" => set_master(s, cx, &a),
        "audio.effects" => {
            let category = a.opt_str("category").map(str::to_lowercase);
            let list: Vec<Value> = plugins::effects(a.opt_str("query"))
                .into_iter()
                .filter(|e| category.as_ref().is_none_or(|c| e.category.to_lowercase() == *c))
                .map(|e| json!(e))
                .collect();
            Ok(json!(list))
        }
        "audio.effectParams" => effect_params(s, &a),
        "audio.rescanPlugins" => {
            let n = tokio::task::spawn_blocking(plugins::rescan).await.map_err(err)??;
            Ok(json!({ "effects": n }))
        }
        "audio.addEffect" => add_effect(s, cx, &a),
        "audio.removeEffect" | "audio.moveEffect" | "audio.setEffect" | "audio.applyPreset" => change_effect(s, cx, &a),
        "audio.copyEffects" => copy_effects(s, cx, &a),
        "audio.effectPresets" => {
            let only = a.opt_str("effect").map(plugins::effect).transpose()?;
            Ok(json!(
                ryolune_engine::stock::FACTORY_PRESETS
                    .iter()
                    .filter(|(name, _, _)| only.as_ref().is_none_or(|e| e.name == *name))
                    .map(|(name, preset, values)| {
                        let infos = plugins::params(&format!("stock:{name}")).unwrap_or_default();
                        let values: Map<String, Value> = values.iter().map(|(id, v)| (param_name(&infos, *id), json!(param_text(&infos, *id, *v)))).collect();
                        json!({ "effect": name, "preset": preset, "values": values })
                    })
                    .collect::<Vec<_>>()
            ))
        }
        "audio.setAutomation" | "audio.addAutomationKey" | "audio.removeAutomationKey" => automation(s, cx, &a),
        "audio.measure" => measure(s, &a).await,
        "audio.normalize" => normalize(s, cx, &a).await,
        "audio.detectBeats" => {
            let p = s.project()?;
            let asset = match (a.opt_str("assetId"), a.opt_str("clipId")) {
                (Some(k), _) => resolve::asset(&p, k)?,
                (None, Some(k)) => {
                    let id = resolve::clip(&p, k)?;
                    p.clip(id).and_then(|c| c.asset_id()).ok_or("That clip has no media, so no music to listen to.")?
                }
                (None, None) => return Err("Give assetId (a media item) or clipId (a clip of the music).".into()),
            };
            let beats = detect_beats(s, &p, asset).await?;
            Ok(beats_json(&beats))
        }
        "audio.beatCut" => beat_cut(s, cx, &a).await,
        "audio.autoDuck" => auto_duck(s, cx, &a),
        "audio.importSong" => import_song(s, cx, &a).await,
        "audio.refreshSongs" => refresh_songs(s, &a).await,
        "audio.openInRyolune" => open_in_ryolune(s, &a).await,
        "audio.scrub" => {
            let on = a.bool_or("on", true);
            s.update_settings(|st| st.audio.scrub = on)?;
            Ok(json!({ "scrub": on }))
        }
        "audio.meters" | "audio.devices" => s.ui_call(cx.spec.name, Value::Object(a.0)).await,
        "audio.record" => {
            let action = a.str("action")?;
            if !["start", "stop", "cancel", "status"].contains(&action) {
                return Err(format!("action is start, stop, cancel or status, not `{action}`."));
            }
            let mut params = a.0.clone();
            if let Some(k) = a.opt_str("trackId") {
                let p = s.project()?;
                let id = resolve::track(&p, k)?;
                if p.track(id).is_some_and(|t| t.kind != TrackKind::Audio) {
                    return Err("Voice-overs are recorded onto audio tracks.".into());
                }
                params.insert("trackId".into(), json!(id));
            }
            s.ui_call(cx.spec.name, Value::Object(params)).await
        }
        "audio.showMixer" => {
            let mut params = a.0.clone();
            if let Some(layout) = a.opt_str("layout")
                && !["replace", "beside"].contains(&layout)
            {
                return Err(format!("layout is replace or beside, not `{layout}`."));
            }
            if let Some(k) = a.opt_str("target") {
                let p = s.project()?;
                let t = target(&p, k)?;
                let slot = a.get("slot").ok_or("Give slot with target: the effect whose panel to open.")?;
                let chain = t.chain(&p)?;
                let i = slot_index(&chain, slot)?;
                params.insert("target".into(), t.to_json());
                params.insert("slot".into(), json!(chain[i].id));
            }
            s.ui_call(cx.spec.name, Value::Object(params)).await
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

// ---- targets ------------------------------------------------------------------------------

/// What an effect chain or automation belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Track(Id),
    Bus(Id),
    Master,
    Clip(Id),
}

impl Target {
    /// `{ "kind": "track", "id": … }`, as the window receives it.
    pub fn to_json(self) -> Value {
        match self {
            Target::Track(id) => json!({ "kind": "track", "id": id }),
            Target::Bus(id) => json!({ "kind": "bus", "id": id }),
            Target::Master => json!({ "kind": "master" }),
            Target::Clip(id) => json!({ "kind": "clip", "id": id }),
        }
    }

    /// Reads [`Self::to_json`] back.
    pub fn from_json(v: &Value) -> Option<Target> {
        let id = || v["id"].as_str().and_then(|s| s.parse().ok());
        Some(match v["kind"].as_str()? {
            "track" => Target::Track(id()?),
            "bus" => Target::Bus(id()?),
            "master" => Target::Master,
            "clip" => Target::Clip(id()?),
            _ => return None,
        })
    }

    /// The name people know it by.
    pub fn name(self, p: &Project) -> String {
        match self {
            Target::Track(id) => p.track(id).map(|t| t.name.clone()).unwrap_or_default(),
            Target::Bus(id) => p.mixer.bus(id).map(|b| b.name.clone()).unwrap_or_default(),
            Target::Master => "Master".into(),
            Target::Clip(id) => p.clip(id).map(|c| c.name.clone()).unwrap_or_default(),
        }
    }

    pub fn chain(self, p: &Project) -> CmdResult<Vec<Insert>> {
        Ok(match self {
            Target::Track(id) => p.track(id).ok_or("track not found")?.mix.effects.clone(),
            Target::Bus(id) => p.mixer.bus(id).ok_or("bus not found")?.mix.effects.clone(),
            Target::Master => p.mixer.master.effects.clone(),
            Target::Clip(id) => p.clip(id).ok_or("clip not found")?.audio.effects.clone(),
        })
    }

    /// Automation keyframes (timeline times); clips have none here (their `volume` and `pan`
    /// keyframes are the clip's own).
    pub fn keyframes(self, p: &Project) -> Option<Keyframes> {
        match self {
            Target::Track(id) => p.track(id).map(|t| t.mix.keyframes.clone()),
            Target::Bus(id) => p.mixer.bus(id).map(|b| b.mix.keyframes.clone()),
            Target::Master => Some(p.mixer.master.keyframes.clone()),
            Target::Clip(_) => None,
        }
    }

    /// The edit that gives it `chain` (and `keys`, when given).
    pub fn edit(self, p: &Project, chain: Vec<Insert>, keys: Option<Keyframes>) -> CmdResult<Edit> {
        Ok(match self {
            Target::Track(id) => {
                let mut mix = p.track(id).ok_or("track not found")?.mix.clone();
                mix.effects = chain;
                if let Some(k) = keys {
                    mix.keyframes = k;
                }
                Edit::UpdateTrack { track_id: id, patch: TrackPatch { mix: Some(mix), ..Default::default() } }
            }
            Target::Bus(id) => {
                let mut mixer = p.mixer.clone();
                let b = mixer.buses.iter_mut().find(|b| b.id == id).ok_or("bus not found")?;
                b.mix.effects = chain;
                if let Some(k) = keys {
                    b.mix.keyframes = k;
                }
                Edit::SetMixer { mixer }
            }
            Target::Master => {
                let mut mixer = p.mixer.clone();
                mixer.master.effects = chain;
                if let Some(k) = keys {
                    mixer.master.keyframes = k;
                }
                Edit::SetMixer { mixer }
            }
            Target::Clip(id) => {
                let mut audio = p.clip(id).ok_or("clip not found")?.audio.clone();
                audio.effects = chain;
                Edit::UpdateClip { clip_id: id, patch: ClipPatch { audio: Some(audio), ..Default::default() } }
            }
        })
    }
}

/// A target by id or name (`master`; `track:`, `bus:`, `clip:` prefixes when names clash).
pub fn target(p: &Project, key: &str) -> CmdResult<Target> {
    let key = key.trim();
    if key.eq_ignore_ascii_case("master") || key.eq_ignore_ascii_case("stereo out") {
        return Ok(Target::Master);
    }
    if let Some((kind, rest)) = key.split_once(':') {
        match kind.trim().to_ascii_lowercase().as_str() {
            "track" => return Ok(Target::Track(resolve::track(p, rest)?)),
            "bus" => return Ok(Target::Bus(bus(p, rest)?)),
            "clip" => return Ok(Target::Clip(resolve::clip(p, rest)?)),
            _ => {}
        }
    }
    let tracks: Vec<Id> = p.tracks.iter().filter(|t| t.id.to_string() == key || t.name.eq_ignore_ascii_case(key)).map(|t| t.id).collect();
    let buses: Vec<Id> = p.mixer.buses.iter().filter(|b| b.id.to_string() == key || b.name.eq_ignore_ascii_case(key)).map(|b| b.id).collect();
    let clips: Vec<Id> = p.clips().filter(|(_, c)| c.id.to_string() == key || c.name.eq_ignore_ascii_case(key)).map(|(_, c)| c.id).collect();
    match (tracks.as_slice(), buses.as_slice(), clips.as_slice()) {
        ([t], [], []) => Ok(Target::Track(*t)),
        ([], [b], []) => Ok(Target::Bus(*b)),
        ([], [], [c]) => Ok(Target::Clip(*c)),
        ([], [], []) => {
            let mut names: Vec<&str> = vec!["master"];
            names.extend(p.tracks.iter().map(|t| t.name.as_str()));
            names.extend(p.mixer.buses.iter().map(|b| b.name.as_str()));
            let hint = closest(key, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
            Err(format!("No track, bus or clip is called `{key}`.{hint} Tracks: {}; buses: {}; or \"master\".", list(p.tracks.iter().map(|t| t.name.as_str())), list(p.mixer.buses.iter().map(|b| b.name.as_str()))))
        }
        _ => Err(format!("`{key}` names more than one thing; write track:{key}, bus:{key} or clip:{key}, or use an id.")),
    }
}

fn list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let v: Vec<&str> = names.take(12).collect();
    if v.is_empty() { "none".into() } else { v.join(", ") }
}

/// A bus by id or name.
pub fn bus(p: &Project, key: &str) -> CmdResult<Id> {
    let key = key.trim();
    if let Some(b) = p.mixer.buses.iter().find(|b| b.id.to_string() == key) {
        return Ok(b.id);
    }
    let found: Vec<&Bus> = p.mixer.buses.iter().filter(|b| b.name.eq_ignore_ascii_case(key)).collect();
    match found.as_slice() {
        [one] => Ok(one.id),
        [] => {
            let names: Vec<&str> = p.mixer.buses.iter().map(|b| b.name.as_str()).collect();
            let hint = closest(key, &names).map(|c| format!(" Did you mean {c}?")).unwrap_or_default();
            if names.is_empty() {
                Err(format!("There is no bus `{key}`: the project has no buses yet (audio.addBus makes one)."))
            } else {
                Err(format!("Unknown bus `{key}`.{hint} Buses: {}.", names.join(", ")))
            }
        }
        many => Err(format!("{} buses are named `{key}`; use an id: {}.", many.len(), many.iter().map(|b| b.id.to_string()).collect::<Vec<_>>().join(", "))),
    }
}

/// Position of an effect in a chain: its slot id, its name, or its position from 1.
pub fn slot_index(chain: &[Insert], slot: &Value) -> CmdResult<usize> {
    let n = chain.len();
    if n == 0 {
        return Err("That chain has no effects (audio.addEffect adds one).".into());
    }
    let pos = |i: i64| -> CmdResult<usize> {
        if i >= 1 && i as usize <= n { Ok(i as usize - 1) } else { Err(format!("Position {i} is outside the chain, which has {n} effect{}.", if n == 1 { "" } else { "s" })) }
    };
    match slot {
        Value::Number(x) => pos(x.as_f64().unwrap_or(0.0) as i64),
        Value::String(key) => {
            let key = key.trim();
            if let Some(i) = chain.iter().position(|e| e.id == key) {
                return Ok(i);
            }
            if let Ok(i) = key.parse::<i64>() {
                return pos(i);
            }
            let named: Vec<usize> = (0..n).filter(|i| chain[*i].name.eq_ignore_ascii_case(key) || chain[*i].plugin_id().eq_ignore_ascii_case(key)).collect();
            match named.as_slice() {
                [i] => Ok(*i),
                [] => {
                    let names: Vec<&str> = chain.iter().map(|e| e.name.as_str()).collect();
                    let hint = closest(key, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
                    Err(format!("No effect `{key}` in that chain.{hint} It holds: {}.", chain.iter().enumerate().map(|(i, e)| format!("{} {} (slot {})", i + 1, e.name, e.id)).collect::<Vec<_>>().join(", ")))
                }
                _ => Err(format!("The chain has {} `{key}`; give the slot id or position instead.", named.len())),
            }
        }
        other => Err(format!("slot is a slot id, an effect name or a position from 1, not {other}.")),
    }
}

/// A slot id no other effect in `chain` has.
fn new_slot(chain: &[Insert]) -> String {
    loop {
        let id = format!("fx-{}", &new_id().simple().to_string()[..6]);
        if !chain.iter().any(|e| e.id == id) {
            return id;
        }
    }
}

// ---- reading the mix ------------------------------------------------------------------------

/// Whether a clip makes sound (audio, or video with sound; songs are audio).
pub fn has_sound(p: &Project, clip: &kimchi_core::Clip) -> bool {
    clip.asset_id().and_then(|a| p.asset(a)).is_some_and(|a| a.kind == MediaKind::Audio || (a.kind == MediaKind::Video && a.meta.has_audio))
}

fn param_name(infos: &[ParamInfo], id: u32) -> String {
    infos.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string())
}

fn param_text(infos: &[ParamInfo], id: u32, v: f64) -> String {
    infos.iter().find(|p| p.id == id).map(|p| p.text(v)).unwrap_or_else(|| format!("{v}"))
}

/// The parameters of an effect slot's plugin. Stock effects are cheap to describe; external
/// plugins are loaded once to ask (only when `load` allows it).
fn infos_of(e: &Insert, load: bool) -> Vec<ParamInfo> {
    let id = e.plugin_id();
    if !load && !id.starts_with("stock:") {
        return vec![];
    }
    plugins::params(&id).unwrap_or_default()
}

/// One effect slot as people read it: its parameters by name, with their values as text.
pub fn effect_json(e: &Insert, position: usize) -> Value {
    let infos = infos_of(e, false);
    let params: Map<String, Value> = if infos.is_empty() {
        e.params.iter().map(|(id, v)| (id.to_string(), json!(v))).collect()
    } else {
        infos.iter().map(|p| (p.name.clone(), json!(p.text(e.params.get(&p.id).copied().unwrap_or(p.default))))).collect()
    };
    let mut v = json!({ "slot": e.id, "position": position + 1, "effect": e.name, "plugin": e.plugin_id(), "params": params });
    if e.is_bypassed() {
        v["bypassed"] = json!(true);
    }
    if !e.blob.is_empty() {
        v["state"] = json!("saved by the plugin");
    }
    v
}

fn chain_json(chain: &[Insert]) -> Vec<Value> {
    chain.iter().enumerate().map(|(i, e)| effect_json(e, i)).collect()
}

/// Names of the automated properties, readable (`gainDb`, `Space.Mix`).
fn automated(chain: &[Insert], keys: &Keyframes) -> Vec<String> {
    keys.keys()
        .map(|k| match parse_effect_key(k) {
            Some((slot, param)) => match chain.iter().find(|e| e.id == slot) {
                Some(e) => format!("{}.{}", e.name, param_name(&infos_of(e, false), param)),
                None => k.clone(),
            },
            None => k.clone(),
        })
        .collect()
}

fn duck_json(p: &Project, d: &Duck) -> Value {
    let under: Vec<String> = d.under.iter().filter_map(|id| p.track(*id).map(|t| t.name.clone())).collect();
    json!({
        "under": if under.is_empty() { json!("every other track") } else { json!(under) },
        "amountDb": d.amount_db,
        "thresholdDb": d.threshold_db,
        "attack": d.attack,
        "release": d.release,
    })
}

/// A track's mix.
pub fn track_json(p: &Project, t: &kimchi_core::Track) -> Value {
    let m = &t.mix;
    let mut v = json!({
        "id": t.id,
        "name": t.name,
        "kind": t.kind,
        "gainDb": round(m.gain_db),
        "pan": round(m.pan),
        "muted": t.muted,
        "solo": m.solo,
        "output": m.output.and_then(|b| p.mixer.bus(b)).map(|b| b.name.clone()).unwrap_or_else(|| "master".into()),
        "clipsWithSound": t.clips.iter().filter(|c| has_sound(p, c)).count(),
    });
    if !m.effects.is_empty() {
        v["effects"] = json!(chain_json(&m.effects));
    }
    if !m.sends.is_empty() {
        v["sends"] = json!(m.sends.iter().map(|s| json!({ "bus": p.mixer.bus(s.bus).map(|b| b.name.clone()), "busId": s.bus, "levelDb": round(s.level_db), "preFader": s.pre_fader })).collect::<Vec<_>>());
    }
    if let Some(d) = &m.duck {
        v["duck"] = duck_json(p, d);
    }
    if m.armed {
        v["armed"] = json!(true);
    }
    if !m.keyframes.is_empty() {
        v["automated"] = json!(automated(&m.effects, &m.keyframes));
    }
    v
}

fn bus_json(b: &Bus) -> Value {
    let mut v = json!({ "id": b.id, "name": b.name, "gainDb": round(b.mix.gain_db), "pan": round(b.mix.pan), "muted": b.muted, "solo": b.mix.solo });
    if !b.mix.effects.is_empty() {
        v["effects"] = json!(chain_json(&b.mix.effects));
    }
    if !b.mix.keyframes.is_empty() {
        v["automated"] = json!(automated(&b.mix.effects, &b.mix.keyframes));
    }
    v
}

fn master_json(m: &Mixer) -> Value {
    let master = &m.master;
    let mut v = json!({
        "gainDb": round(master.gain_db),
        "limiter": master.limiter,
        "ceilingDb": master.ceiling_db,
        "loudness": master.loudness,
    });
    if let Some(l) = master.loudness
        && let Some((name, _)) = LOUDNESS_PRESETS.iter().find(|(_, v)| (*v - l).abs() < 1e-9)
    {
        v["loudnessFor"] = json!(name);
    }
    if !master.effects.is_empty() {
        v["effects"] = json!(chain_json(&master.effects));
    }
    if !master.keyframes.is_empty() {
        v["automated"] = json!(automated(&master.effects, &master.keyframes));
    }
    v
}

/// A clip's sound, as `audio.setClip` takes it.
pub fn clip_sound_json(p: &Project, c: &kimchi_core::Clip) -> Value {
    let a = &c.audio;
    let mut v = json!({
        "id": c.id,
        "name": c.name,
        "volume": round(c.volume),
        "gainDb": round(gain_to_db(c.volume)),
        "pan": round(a.pan),
        "fadeIn": round(c.fade_in),
        "fadeOut": round(c.fade_out),
        "fadeCurve": a.fade_curve.name(),
        "channels": channels_name(a.channels),
        "pitch": a.pitch,
        "preservePitch": a.preserve_pitch,
        "muted": a.muted,
    });
    if let Some((ti, _)) = p.locate_clip(c.id) {
        v["track"] = json!(p.tracks[ti].name);
    }
    if !a.effects.is_empty() {
        v["effects"] = json!(chain_json(&a.effects));
    }
    let animated: Vec<&String> = c.keyframes.keys().filter(|k| *k == "volume" || *k == "pan").collect();
    if !animated.is_empty() {
        v["animated"] = json!(animated);
    }
    v
}

pub fn channels_name(c: Channels) -> &'static str {
    match c {
        Channels::Stereo => "stereo",
        Channels::Mono => "mono",
        Channels::Left => "left",
        Channels::Right => "right",
        Channels::Swap => "swap",
    }
}

fn beats_json(b: &Beats) -> Value {
    json!({
        "tempo": (b.tempo * 10.0).round() / 10.0,
        "beatsPerBar": b.beats_per_bar,
        "beats": b.times.len(),
        "firstDownbeat": b.times.get(b.first_downbeat).map(|t| round(*t)),
        "source": b.source,
    })
}

/// Whether a song's file was saved since it was rendered (or is gone).
pub fn song_state(r: &SongRef) -> &'static str {
    match modified(Path::new(&r.song)) {
        None => "missing",
        Some(m) if r.song_modified.as_deref() != Some(m.as_str()) => "changed",
        Some(_) => "current",
    }
}

/// A file's modification time, RFC 3339.
fn modified(path: &Path) -> Option<String> {
    let t = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
}

pub fn overview(p: &Project) -> Value {
    let mut problems: Vec<String> = vec![];
    let soloed: Vec<&str> = p.tracks.iter().filter(|t| t.mix.solo).map(|t| t.name.as_str()).collect();
    if !soloed.is_empty() {
        problems.push(format!("Solo is on: only {} (and the buses they feed) can be heard.", soloed.join(", ")));
    }
    let known: Vec<String> = plugins::effects(None).into_iter().map(|e| e.id).collect();
    let mut chains: Vec<(String, &Vec<Insert>)> = p.tracks.iter().map(|t| (t.name.clone(), &t.mix.effects)).collect();
    chains.extend(p.mixer.buses.iter().map(|b| (b.name.clone(), &b.mix.effects)));
    chains.push(("Master".into(), &p.mixer.master.effects));
    chains.extend(p.clips().map(|(_, c)| (c.name.clone(), &c.audio.effects)));
    for (owner, chain) in chains {
        for e in chain.iter().filter(|e| !known.contains(&e.plugin_id())) {
            problems.push(format!("{owner}: the effect {} ({}) isn't on this computer, so it is skipped.", e.name, e.plugin_id()));
        }
    }
    let mut songs = vec![];
    let mut beats = vec![];
    for asset in &p.assets {
        let clips: Vec<Id> = p.clips().filter(|(_, c)| c.asset_id() == Some(asset.id)).map(|(_, c)| c.id).collect();
        if let AssetOrigin::Song(r) = &asset.origin {
            let state = song_state(r);
            match state {
                "changed" => problems.push(format!("The song {} was saved in ryolune since kimchi rendered it: audio.refreshSongs renders it again.", r.title)),
                "missing" => problems.push(format!("The song file {} is gone; kimchi keeps playing its last render.", r.song)),
                _ => {}
            }
            songs.push(json!({ "assetId": asset.id, "name": asset.name, "song": r.song, "title": r.title, "stem": r.track, "tempo": r.tempo, "state": state, "clips": clips }));
        }
        if let Some(b) = &asset.beats {
            let mut v = beats_json(b);
            v["assetId"] = json!(asset.id);
            v["name"] = json!(asset.name);
            beats.push(v);
        }
    }
    let muted_clips: Vec<&str> = p.clips().filter(|(_, c)| c.audio.muted && has_sound(p, c)).map(|(_, c)| c.name.as_str()).collect();
    if !muted_clips.is_empty() {
        problems.push(format!("These clips' own sound is muted: {}.", muted_clips.join(", ")));
    }
    for b in &p.mixer.buses {
        let fed = p.tracks.iter().any(|t| t.mix.output == Some(b.id) || t.mix.sends.iter().any(|s| s.bus == b.id));
        if !fed {
            problems.push(format!("Nothing feeds the bus {}.", b.name));
        }
    }
    let clips: Vec<Value> = p
        .clips()
        .filter(|(_, c)| has_sound(p, c) && (!c.audio.is_default() || c.volume != 1.0 || c.keyframes.contains_key("volume") || c.keyframes.contains_key("pan")))
        .map(|(_, c)| clip_sound_json(p, c))
        .collect();
    json!({
        "sampleRate": kimchi_media::audio::rate(p),
        "tracks": p.tracks.iter().map(|t| track_json(p, t)).collect::<Vec<_>>(),
        "buses": p.mixer.buses.iter().map(bus_json).collect::<Vec<_>>(),
        "master": master_json(&p.mixer),
        "clips": clips,
        "songs": songs,
        "beats": beats,
        "loudnessTarget": p.mixer.master.loudness,
        "problems": problems,
    })
}

// ---- tracks, clips, buses, master -----------------------------------------------------------

fn level(name: &str, v: f64) -> CmdResult<f64> {
    if !v.is_finite() || v > MAX_DB + 1e-9 {
        return Err(format!("{name} goes from {MIN_DB} to +{MAX_DB} dB, not {v}."));
    }
    Ok(v.max(MIN_DB))
}

fn pan(v: f64) -> CmdResult<f64> {
    if !(-1.0..=1.0).contains(&v) {
        return Err(format!("pan goes from -1 (left) to 1 (right), not {v}."));
    }
    Ok(v)
}

fn set_track(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let id = resolve::track(&p, a.str("trackId")?)?;
    let t = p.track(id).ok_or("track not found")?;
    let mut mix = t.mix.clone();
    if let Some(v) = a.opt_f64("gainDb") {
        mix.gain_db = level("gainDb", v)?;
    }
    if let Some(v) = a.opt_f64("pan") {
        mix.pan = pan(v)?;
    }
    if let Some(v) = a.opt_bool("solo") {
        mix.solo = v;
    }
    if let Some(v) = a.opt_bool("armed") {
        if v && t.kind != TrackKind::Audio {
            return Err(format!("\"{}\" is a video track; voice-overs are recorded onto audio tracks.", t.name));
        }
        mix.armed = v;
    }
    if let Some(o) = a.opt_str("output") {
        mix.output = match o.trim() {
            x if x.is_empty() || x.eq_ignore_ascii_case("master") || x.eq_ignore_ascii_case("none") => None,
            x => Some(bus(&p, x)?),
        };
    }
    if a.0.contains_key("duck") {
        mix.duck = duck(&p, id, mix.duck.take(), a.0.get("duck").unwrap_or(&Value::Null))?;
    }
    let patch = TrackPatch { mix: Some(mix), muted: a.opt_bool("muted"), ..Default::default() };
    s.apply(cx.label(), cx.source, &Edit::UpdateTrack { track_id: id, patch }, a.coalesce())?;
    s.read(|ed| ed.project().track(id).map(|t| track_json(ed.project(), t)).unwrap_or(Value::Null))
}

/// `duck` as given: on, off, or its settings on top of what the track has.
fn duck(p: &Project, me: Id, current: Option<Duck>, v: &Value) -> CmdResult<Option<Duck>> {
    Ok(match v {
        Value::Null | Value::Bool(false) => None,
        Value::String(x) if ["off", "none", "false"].contains(&x.to_ascii_lowercase().as_str()) => None,
        Value::Bool(true) => Some(current.unwrap_or_default()),
        Value::String(x) if ["on", "true"].contains(&x.to_ascii_lowercase().as_str()) => Some(current.unwrap_or_default()),
        Value::Object(o) => {
            let mut d = current.unwrap_or_default();
            for (k, val) in o {
                let num = || val.as_f64().ok_or_else(|| format!("duck.{k} should be a number"));
                match k.as_str() {
                    "under" => {
                        let names: Vec<String> = match val {
                            Value::String(s) => vec![s.clone()],
                            Value::Array(list) => list.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                            _ => return Err("duck.under is a list of tracks".into()),
                        };
                        d.under = names.iter().map(|n| resolve::track(p, n)).collect::<CmdResult<Vec<Id>>>()?;
                        if d.under.contains(&me) {
                            return Err("A track can't duck under itself.".into());
                        }
                    }
                    "amountDb" | "amount" => d.amount_db = -num()?.abs(),
                    "thresholdDb" | "threshold" => d.threshold_db = num()?,
                    "attack" => d.attack = num()?,
                    "release" => d.release = num()?,
                    other => return Err(format!("Unknown duck field `{other}`. Fields: under, amountDb, thresholdDb, attack, release.")),
                }
            }
            Some(d)
        }
        other => return Err(format!("duck is true, false or {{under, amountDb, thresholdDb, attack, release}}, not {other}")),
    })
}

fn named<T: Copy>(what: &str, key: &str, options: &[(&str, T)]) -> CmdResult<T> {
    let k = key.trim();
    if let Some((_, v)) = options.iter().find(|(n, _)| n.eq_ignore_ascii_case(k) || n.replace(['-', '_', ' '], "").eq_ignore_ascii_case(&k.replace(['-', '_', ' '], ""))) {
        return Ok(*v);
    }
    let names: Vec<&str> = options.iter().map(|(n, _)| *n).collect();
    let hint = closest(k, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("{what} is one of {}, not `{k}`.{hint}", names.join(", ")))
}

pub fn fade_curve(key: &str) -> CmdResult<FadeCurve> {
    named("fadeCurve", key, &[("linear", FadeCurve::Linear), ("equalPower", FadeCurve::EqualPower), ("exponential", FadeCurve::Exponential), ("sCurve", FadeCurve::SCurve)])
}

pub fn channels(key: &str) -> CmdResult<Channels> {
    named("channels", key, &[("stereo", Channels::Stereo), ("mono", Channels::Mono), ("left", Channels::Left), ("right", Channels::Right), ("swap", Channels::Swap)])
}

fn set_clip(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let ids = resolve::clips(&p, &a.strings("clipIds"))?;
    if ids.is_empty() {
        return Err("`clipIds` is empty".into());
    }
    if a.has("gainDb") && a.has("volume") {
        return Err("Give gainDb or volume, not both.".into());
    }
    let volume = match (a.opt_f64("gainDb"), a.opt_f64("volume")) {
        (Some(db), _) => Some(db_to_gain(level("gainDb", db)?).min(4.0)),
        (_, Some(v)) if !(0.0..=4.0).contains(&v) => return Err(format!("volume goes from 0 to 4, not {v}.")),
        (_, v) => v,
    };
    let curve = a.opt_str("fadeCurve").map(fade_curve).transpose()?;
    let chans = a.opt_str("channels").map(channels).transpose()?;
    let mut edits = vec![];
    for id in &ids {
        let c = p.clip(*id).ok_or("clip not found")?;
        if !has_sound(&p, c) {
            return Err(format!("\"{}\" has no sound.", c.name));
        }
        let mut audio = c.audio.clone();
        if let Some(v) = a.opt_f64("pan") {
            audio.pan = pan(v)?;
        }
        if let Some(v) = curve {
            audio.fade_curve = v;
        }
        if let Some(v) = chans {
            audio.channels = v;
        }
        if let Some(v) = a.opt_f64("pitch") {
            audio.pitch = v;
        }
        if let Some(v) = a.opt_bool("preservePitch") {
            audio.preserve_pitch = v;
        }
        if let Some(v) = a.opt_bool("muted") {
            audio.muted = v;
        }
        audio.check().map_err(|e| format!("\"{}\": {e}", c.name))?;
        let patch = ClipPatch { volume, fade_in: a.opt_f64("fadeIn"), fade_out: a.opt_f64("fadeOut"), audio: (audio != c.audio).then_some(audio), ..Default::default() };
        edits.push(Edit::UpdateClip { clip_id: *id, patch });
    }
    apply_all(s, cx, &edits, a.coalesce())?;
    s.read(|ed| json!(ids.iter().filter_map(|id| ed.project().clip(*id)).map(|c| clip_sound_json(ed.project(), c)).collect::<Vec<_>>()))
}

fn add_bus(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    if p.mixer.buses.len() >= MAX_BUSES {
        return Err(format!("A project has at most {MAX_BUSES} buses."));
    }
    let name = match a.opt_str("name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => (1..).map(|i| format!("Bus {i}")).find(|n| !p.mixer.buses.iter().any(|b| &b.name == n)).unwrap_or_default(),
    };
    let mut mix = TrackMix::default();
    if let Some(v) = a.opt_f64("gainDb") {
        mix.gain_db = level("gainDb", v)?;
    }
    if let Some(e) = a.opt_str("effect") {
        mix.effects.push(plugins::new_insert(e, new_slot(&[]))?);
    }
    let bus = Bus { id: new_id(), name, muted: false, mix };
    let id = bus.id;
    let mut mixer = p.mixer.clone();
    mixer.buses.push(bus);
    s.apply(cx.label(), cx.source, &Edit::SetMixer { mixer }, None)?;
    s.read(|ed| ed.project().mixer.bus(id).map(bus_json).unwrap_or(Value::Null))
}

fn set_bus(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let id = bus(&p, a.str("busId")?)?;
    let mut mixer = p.mixer.clone();
    let b = mixer.buses.iter_mut().find(|b| b.id == id).ok_or("bus not found")?;
    if let Some(n) = a.opt_str("name").map(str::trim) {
        if n.is_empty() {
            return Err("A bus needs a name.".into());
        }
        b.name = n.to_string();
    }
    if let Some(v) = a.opt_f64("gainDb") {
        b.mix.gain_db = level("gainDb", v)?;
    }
    if let Some(v) = a.opt_f64("pan") {
        b.mix.pan = pan(v)?;
    }
    if let Some(v) = a.opt_bool("muted") {
        b.muted = v;
    }
    if let Some(v) = a.opt_bool("solo") {
        b.mix.solo = v;
    }
    s.apply(cx.label(), cx.source, &Edit::SetMixer { mixer }, a.coalesce())?;
    s.read(|ed| ed.project().mixer.bus(id).map(bus_json).unwrap_or(Value::Null))
}

fn set_send(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let track = resolve::track(&p, a.str("trackId")?)?;
    let to = bus(&p, a.str("busId")?)?;
    let t = p.track(track).ok_or("track not found")?;
    let mut mix = t.mix.clone();
    if cx.spec.name == "audio.removeSend" {
        if !mix.sends.iter().any(|s| s.bus == to) {
            return Err(format!("\"{}\" doesn't send to that bus.", t.name));
        }
        mix.sends.retain(|s| s.bus != to);
    } else {
        let level = a.opt_f64("levelDb").map(|v| level("levelDb", v)).transpose()?;
        match mix.sends.iter_mut().find(|s| s.bus == to) {
            Some(send) => {
                send.level_db = level.unwrap_or(send.level_db);
                send.pre_fader = a.opt_bool("preFader").unwrap_or(send.pre_fader);
            }
            None => mix.sends.push(Send { bus: to, level_db: level.unwrap_or(0.0), pre_fader: a.bool_or("preFader", false) }),
        }
        if mix.output == Some(to) {
            return Err(format!("\"{}\" already feeds that bus whole; a send would double it.", t.name));
        }
    }
    s.apply(cx.label(), cx.source, &Edit::UpdateTrack { track_id: track, patch: TrackPatch { mix: Some(mix), ..Default::default() } }, a.coalesce())?;
    s.read(|ed| ed.project().track(track).map(|t| track_json(ed.project(), t)).unwrap_or(Value::Null))
}

/// A loudness target as given: LUFS, a preset's name, or off.
pub fn loudness(v: &Value) -> CmdResult<Option<f64>> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(false) => Ok(None),
        Value::Number(n) => {
            let l = n.as_f64().unwrap_or(f64::NAN);
            if !(l.is_finite() && (-40.0..=-5.0).contains(&l)) {
                return Err(format!("Loudness targets go from -40 to -5 LUFS, not {n}."));
            }
            Ok(Some(l))
        }
        Value::String(s) => {
            let k = s.trim().to_ascii_lowercase();
            if ["off", "none", "null", ""].contains(&k.as_str()) {
                return Ok(None);
            }
            if let Ok(n) = k.trim_end_matches("lufs").trim().parse::<f64>() {
                return loudness(&json!(n));
            }
            LOUDNESS_PRESETS.iter().find(|(n, _)| *n == k).map(|(_, l)| Some(*l)).ok_or_else(|| format!("loudness is a number of LUFS, youtube (-14), podcast (-16), broadcast (-23) or off, not `{s}`."))
        }
        other => Err(format!("loudness is a number of LUFS or a name like youtube, not {other}.")),
    }
}

fn set_master(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let mut mixer = p.mixer.clone();
    let m = &mut mixer.master;
    if let Some(v) = a.opt_f64("gainDb") {
        m.gain_db = level("gainDb", v)?;
    }
    if let Some(v) = a.opt_bool("limiter") {
        m.limiter = v;
    }
    if let Some(v) = a.opt_f64("ceilingDb") {
        m.ceiling_db = v;
    }
    if a.0.contains_key("loudness") {
        m.loudness = loudness(a.0.get("loudness").unwrap_or(&Value::Null))?;
    }
    s.apply(cx.label(), cx.source, &Edit::SetMixer { mixer }, a.coalesce())?;
    s.read(|ed| master_json(&ed.project().mixer))
}

// ---- effects --------------------------------------------------------------------------------

/// A parameter of `infos` by id or name (case and spaces ignored), with a "did you mean".
fn find_param<'a>(effect: &str, infos: &'a [ParamInfo], key: &str) -> CmdResult<&'a ParamInfo> {
    let squash = |x: &str| x.chars().filter(|c| !c.is_whitespace() && *c != '_' && *c != '-').collect::<String>().to_lowercase();
    let k = squash(key);
    if let Some(p) = infos.iter().find(|p| p.id.to_string() == key.trim() || squash(&p.name) == k) {
        return Ok(p);
    }
    let names: Vec<&str> = infos.iter().map(|p| p.name.as_str()).collect();
    let hint = closest(key, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("{effect} has no parameter `{key}`.{hint} Parameters: {}.", names.join(", ")))
}

/// A value as given (a number in the parameter's unit, text like "-6 dB" or "Hall", or a
/// switch's true / false), checked against the parameter's range.
pub fn param_value(info: &ParamInfo, v: &Value) -> CmdResult<f64> {
    let range = || {
        if !info.labels.is_empty() {
            format!("one of {}", info.labels.join(", "))
        } else {
            format!("{} to {}", info.text(info.min), info.text(info.max))
        }
    };
    let x = match v {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()).ok_or_else(|| format!("{} should be a number", info.name))?,
        Value::Bool(b) => {
            if *b {
                info.max
            } else {
                info.min
            }
        }
        Value::String(t) => info.parse(t).ok_or_else(|| format!("`{t}` isn't a value of {} ({}).", info.name, range()))?,
        other => return Err(format!("{} takes a number or text, not {other}.", info.name)),
    };
    let tol = (info.max - info.min).abs() * 1e-9 + 1e-9;
    if x < info.min - tol || x > info.max + tol {
        return Err(format!("{} goes from {}, not {}.", info.name, range(), info.text(x)));
    }
    Ok(x.clamp(info.min.min(info.max), info.max.max(info.min)))
}

/// Applies `{name or id: value}` to a slot.
fn set_params(e: &mut Insert, infos: &[ParamInfo], values: &Map<String, Value>) -> CmdResult<()> {
    if infos.is_empty() && !values.is_empty() {
        return Err(format!("{} doesn't say what its parameters are, so they can't be set by name.", e.name));
    }
    for (k, v) in values {
        let info = find_param(&e.name, infos, k)?;
        e.params.insert(info.id, param_value(info, v)?);
    }
    Ok(())
}

fn add_effect(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let t = target(&p, a.str("target")?)?;
    let mut chain = t.chain(&p)?;
    if chain.len() >= MAX_EFFECTS {
        return Err(format!("{} already has {MAX_EFFECTS} effects, as many as a chain holds.", t.name(&p)));
    }
    let mut e = plugins::new_insert(a.str("effect")?, new_slot(&chain))?;
    if let Some(values) = a.object("params") {
        let infos = plugins::params(&e.plugin_id())?;
        set_params(&mut e, &infos, values)?;
    }
    if a.bool_or("bypassed", false) {
        e.state = "bypassed".into();
    }
    let index = a.opt_i64("index").map(|i| (i.max(0) as usize).min(chain.len())).unwrap_or(chain.len());
    let slot = e.id.clone();
    chain.insert(index, e);
    s.apply(cx.label(), cx.source, &t.edit(&p, chain, None)?, None)?;
    let p = s.project()?;
    let chain = t.chain(&p)?;
    let i = chain.iter().position(|e| e.id == slot).unwrap_or(index);
    let mut v = effect_json(&chain[i], i);
    v["target"] = json!(t.name(&p));
    Ok(v)
}

/// removeEffect, moveEffect, setEffect, applyPreset: one slot of one chain.
fn change_effect(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let t = target(&p, a.str("target")?)?;
    let mut chain = t.chain(&p)?;
    let i = slot_index(&chain, a.get("slot").ok_or("`slot` is required")?)?;
    let slot = chain[i].id.clone();
    let mut keys = t.keyframes(&p);
    match cx.spec.name {
        "audio.removeEffect" => {
            let gone = chain.remove(i);
            // Its automation goes with it.
            if let Some(k) = keys.as_mut() {
                k.retain(|name, _| parse_effect_key(name).is_none_or(|(s, _)| s != slot));
            }
            s.apply(cx.label(), cx.source, &t.edit(&p, chain, keys)?, None)?;
            return Ok(json!({ "removed": gone.name, "slot": slot, "target": t.name(&p) }));
        }
        "audio.moveEffect" => {
            let to = a.opt_i64("index").unwrap_or(0).max(0) as usize;
            let e = chain.remove(i);
            let to = to.min(chain.len());
            chain.insert(to, e);
        }
        "audio.applyPreset" => {
            let name = a.str("preset")?;
            let effect = chain[i].name.clone();
            let presets: Vec<&ryolune_engine::stock::FactoryPreset> = ryolune_engine::stock::FACTORY_PRESETS.iter().filter(|(n, _, _)| *n == effect).collect();
            if presets.is_empty() {
                return Err(format!("{effect} has no presets."));
            }
            let Some((_, _, values)) = presets.iter().find(|(_, pr, _)| pr.eq_ignore_ascii_case(name.trim())) else {
                let names: Vec<&str> = presets.iter().map(|(_, pr, _)| *pr).collect();
                let hint = closest(name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
                return Err(format!("{effect} has no preset `{name}`.{hint} Presets: {}.", names.join(", ")));
            };
            chain[i].params = values.iter().copied().collect();
        }
        _ => {
            // setEffect.
            let infos = infos_of(&chain[i], true);
            if a.bool_or("reset", false) {
                chain[i].params.clear();
            }
            if let Some(b) = a.opt_bool("bypassed") {
                chain[i].state = if b { "bypassed" } else { "active" }.into();
            }
            if let Some(values) = a.object("params") {
                let mut fresh = Map::new();
                let playhead = s.ui_state().playhead;
                for (k, v) in values {
                    let info = find_param(&chain[i].name, &infos, k)?;
                    let key = effect_key(&slot, info.id);
                    match keys.as_mut().filter(|k| k.contains_key(&key)) {
                        // Automated: a keyframe at the playhead, as the window's controls do.
                        Some(k) => {
                            let value = param_value(info, v)?;
                            kimchi_core::anim::set_key(k, &key, Keyframe { time: playhead, value: KeyValue::Number(value), easing: Easing::default() });
                        }
                        None => {
                            fresh.insert(k.clone(), v.clone());
                        }
                    }
                }
                set_params(&mut chain[i], &infos, &fresh)?;
            }
        }
    }
    s.apply(cx.label(), cx.source, &t.edit(&p, chain, keys)?, a.coalesce())?;
    let p = s.project()?;
    let chain = t.chain(&p)?;
    let i = chain.iter().position(|e| e.id == slot).unwrap_or(0);
    let mut v = effect_json(&chain[i], i);
    v["target"] = json!(t.name(&p));
    Ok(v)
}

fn copy_effects(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let from = target(&p, a.str("from")?)?;
    let chain = from.chain(&p)?;
    if chain.is_empty() {
        return Err(format!("{} has no effects to copy.", from.name(&p)));
    }
    let to: Vec<Target> = a.strings("to").iter().map(|k| target(&p, k)).collect::<CmdResult<_>>()?;
    if to.is_empty() {
        return Err("`to` is empty".into());
    }
    let mut edits = vec![];
    // Each edit is made on the project as the ones before leave it (the master and the buses
    // share one `SetMixer`).
    let mut work = p.clone();
    for t in &to {
        if *t == from {
            continue;
        }
        let mut dest = if a.bool_or("append", false) { t.chain(&work)? } else { vec![] };
        for e in &chain {
            let mut e = e.clone();
            if dest.iter().any(|d| d.id == e.id) {
                e.id = new_slot(&dest);
            }
            dest.push(e);
        }
        if dest.len() > MAX_EFFECTS {
            return Err(format!("{} would hold {} effects; a chain holds {MAX_EFFECTS}.", t.name(&p), dest.len()));
        }
        // Automation of the slots it loses goes with them.
        let keys = t.keyframes(&work).map(|mut k| {
            k.retain(|name, _| parse_effect_key(name).is_none_or(|(slot, _)| dest.iter().any(|e| e.id == slot)));
            k
        });
        let edit = t.edit(&work, dest, keys)?;
        work.apply(&edit).map_err(err)?;
        edits.push(edit);
    }
    apply_all(s, cx, &edits, None)?;
    Ok(json!({ "copied": chain.len(), "to": to.iter().map(|t| t.name(&p)).collect::<Vec<_>>() }))
}

fn effect_params(s: &Arc<Session>, a: &Args) -> CmdResult {
    let (slot, plugin) = match (a.opt_str("target"), a.opt_str("effect")) {
        (Some(k), _) => {
            let p = s.project()?;
            let t = target(&p, k)?;
            let chain = t.chain(&p)?;
            let i = slot_index(&chain, a.get("slot").ok_or("Give slot with target: which effect of the chain.")?)?;
            let keys = t.keyframes(&p).unwrap_or_default();
            (Some((chain[i].clone(), keys)), chain[i].plugin_id())
        }
        (None, Some(e)) => (None, plugins::effect(e)?.id),
        (None, None) => return Err("Give effect (an effect's id or name), or target and slot.".into()),
    };
    let info = plugins::effect(&plugin).ok();
    let infos = plugins::params(&plugin)?;
    let params: Vec<Value> = infos
        .iter()
        .map(|p| {
            let mut v = json!({
                "id": p.id,
                "name": p.name,
                "min": p.min,
                "max": p.max,
                "default": p.default,
                "defaultText": p.text(p.default),
                "unit": p.unit,
            });
            if p.steps > 0 {
                v["steps"] = json!(p.steps);
            }
            if p.log {
                v["log"] = json!(true);
            }
            if !p.labels.is_empty() {
                v["choices"] = json!(p.labels);
            }
            if let Some((e, keys)) = &slot {
                let value = e.params.get(&p.id).copied().unwrap_or(p.default);
                v["value"] = json!(value);
                v["text"] = json!(p.text(value));
                if keys.contains_key(&effect_key(&e.id, p.id)) {
                    v["automated"] = json!(true);
                }
            }
            v
        })
        .collect();
    let mut out = json!({ "plugin": plugin, "params": params });
    if let Some(i) = info {
        out["effect"] = json!(i.name);
        out["category"] = json!(i.category);
        out["description"] = json!(i.description);
    }
    if let Some((e, _)) = slot {
        out["slot"] = json!(e.id);
        out["bypassed"] = json!(e.is_bypassed());
    }
    Ok(out)
}

/// The names of ryolune's factory presets for a stock effect (by its name).
pub fn preset_names(effect: &str) -> Vec<&'static str> {
    ryolune_engine::stock::FACTORY_PRESETS.iter().filter(|(n, _, _)| *n == effect).map(|(_, p, _)| *p).collect()
}

// ---- automation -----------------------------------------------------------------------------

/// What an automation property is.
enum Prop {
    Gain,
    Pan,
    Param { slot: String, info: ParamInfo },
}

impl Prop {
    fn key(&self) -> String {
        match self {
            Prop::Gain => "gainDb".into(),
            Prop::Pan => "pan".into(),
            Prop::Param { slot, info } => effect_key(slot, info.id),
        }
    }

    fn check(&self, v: &Value) -> CmdResult<f64> {
        match self {
            Prop::Gain => level("gainDb", v.as_f64().ok_or("gainDb keyframes are numbers of dB")?),
            Prop::Pan => pan(v.as_f64().ok_or("pan keyframes are numbers from -1 to 1")?),
            Prop::Param { info, .. } => param_value(info, v),
        }
    }
}

fn prop(p: &Project, t: Target, name: &str) -> CmdResult<Prop> {
    let n = name.trim();
    match n.to_ascii_lowercase().as_str() {
        "gaindb" | "gain" | "volume" | "fader" | "level" => return Ok(Prop::Gain),
        "pan" if t == Target::Master => return Err("The master has no pan; automate its gainDb or an effect parameter.".into()),
        "pan" => return Ok(Prop::Pan),
        _ => {}
    }
    let chain = t.chain(p)?;
    if let Some((slot, id)) = parse_effect_key(n) {
        let e = chain.iter().find(|e| e.id == slot).ok_or_else(|| format!("{} has no effect slot `{slot}`.", t.name(p)))?;
        let infos = infos_of(e, true);
        let info = infos.iter().find(|i| i.id == id).cloned().ok_or_else(|| format!("{} has no parameter {id}.", e.name))?;
        return Ok(Prop::Param { slot: e.id.clone(), info });
    }
    // "<effect or slot>.<parameter>": the effect's part may itself contain dots.
    let n = n.strip_prefix("effects.").unwrap_or(n);
    let splits: Vec<usize> = n.match_indices('.').map(|(i, _)| i).collect();
    let mut last_err = None;
    for i in splits.into_iter().rev() {
        let (effect, param) = (&n[..i], &n[i + 1..]);
        match slot_index(&chain, &json!(effect)) {
            Ok(si) => {
                let infos = infos_of(&chain[si], true);
                let info = find_param(&chain[si].name, &infos, param)?.clone();
                return Ok(Prop::Param { slot: chain[si].id.clone(), info });
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| format!("Unknown property `{name}`: gainDb, pan, or \"<effect>.<parameter>\" such as \"Space.Mix\".")))
}

/// What a property is at timeline time `t` (its automation, else its value).
fn value_now(p: &Project, target: Target, prop: &Prop, t: f64) -> f64 {
    let keys = target.keyframes(p).unwrap_or_default();
    if let Some(v) = kimchi_core::anim::number_at(&keys, &prop.key(), t) {
        return v;
    }
    match (target, prop) {
        (Target::Track(id), Prop::Gain) => p.track(id).map(|x| x.mix.gain_db).unwrap_or(0.0),
        (Target::Track(id), Prop::Pan) => p.track(id).map(|x| x.mix.pan).unwrap_or(0.0),
        (Target::Bus(id), Prop::Gain) => p.mixer.bus(id).map(|b| b.mix.gain_db).unwrap_or(0.0),
        (Target::Bus(id), Prop::Pan) => p.mixer.bus(id).map(|b| b.mix.pan).unwrap_or(0.0),
        (Target::Master, Prop::Gain) => p.mixer.master.gain_db,
        (_, Prop::Param { slot, info }) => target.chain(p).ok().and_then(|c| c.iter().find(|e| &e.id == slot).and_then(|e| e.params.get(&info.id).copied())).unwrap_or(info.default),
        _ => 0.0,
    }
}

fn automation(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let t = target(&p, a.str("target")?)?;
    if let Target::Clip(_) = t {
        return Err("Clips are animated with clip.setKeyframes (properties volume and pan); audio automation is for tracks, buses and the master.".into());
    }
    let prop = prop(&p, t, a.str("property")?)?;
    let key = prop.key();
    let mut keys = t.keyframes(&p).unwrap_or_default();
    let chain = t.chain(&p)?;
    let playhead = s.ui_state().playhead;
    // When an automation goes, the property keeps what it was at the playhead.
    let mut hold: Option<f64> = None;
    match cx.spec.name {
        "audio.setAutomation" => {
            let list = a.array("keyframes").ok_or("keyframes is a list, like [[0, -6], [2, 0, \"easeOut\"]].")?;
            let mut out = vec![];
            for (i, k) in list.iter().enumerate() {
                // Text values ("-6 dB") are read by the parameter, not as colours.
                let (raw, k) = match k {
                    Value::Array(parts) if parts.len() >= 2 => (parts[1].clone(), k.clone()),
                    Value::Object(o) => (o.get("value").or_else(|| o.get("v")).cloned().unwrap_or(Value::Null), k.clone()),
                    _ => (Value::Null, k.clone()),
                };
                let mut key = Keyframe::from_json(&k).map_err(|e| format!("keyframes[{i}]: {e}"))?;
                key.value = KeyValue::Number(prop.check(&raw).map_err(|e| format!("keyframes[{i}]: {e}"))?);
                out.push(key);
            }
            if out.is_empty() {
                hold = Some(value_now(&p, t, &prop, playhead));
                keys.remove(&key);
            } else {
                keys.insert(key.clone(), out);
                kimchi_core::anim::normalize(&mut keys);
            }
        }
        "audio.addAutomationKey" => {
            let time = a.opt_f64("time").unwrap_or(playhead).max(0.0);
            let value = match a.get("value") {
                Some(v) => prop.check(v)?,
                None => value_now(&p, t, &prop, time),
            };
            let easing = a.opt_str("easing").map(Easing::parse).transpose()?.unwrap_or_default();
            kimchi_core::anim::set_key(&mut keys, &key, Keyframe { time, value: KeyValue::Number(value), easing });
        }
        _ => {
            if !keys.contains_key(&key) {
                return Err(format!("{} has no automation on `{}`.", t.name(&p), a.str("property")?));
            }
            match a.opt_f64("time") {
                Some(time) => {
                    if !kimchi_core::anim::remove_key(&mut keys, &key, time) {
                        return Err(format!("No keyframe at {time} s on `{}`.", a.str("property")?));
                    }
                }
                None => {
                    hold = Some(value_now(&p, t, &prop, playhead));
                    keys.remove(&key);
                }
            }
        }
    }
    let edit = match hold {
        Some(v) => held(&p, t, &prop, v, chain, keys)?,
        None => t.edit(&p, chain, Some(keys))?,
    };
    s.apply(cx.label(), cx.source, &edit, a.coalesce())?;
    let p = s.project()?;
    let keys = t.keyframes(&p).unwrap_or_default();
    Ok(json!({ "target": t.name(&p), "property": key, "keyframes": crate::commands::motion::keys_json(&keys.into_iter().filter(|(k, _)| *k == key).collect()) }))
}

/// The edit that sets `keys` and gives the property the still value `v`.
fn held(p: &Project, t: Target, prop: &Prop, v: f64, mut chain: Vec<Insert>, keys: Keyframes) -> CmdResult<Edit> {
    if let Prop::Param { slot, info } = prop
        && let Some(e) = chain.iter_mut().find(|e| &e.id == slot)
    {
        e.params.insert(info.id, v);
    }
    let edit = t.edit(p, chain, Some(keys))?;
    Ok(match (edit, prop) {
        (Edit::UpdateTrack { track_id, mut patch }, Prop::Gain | Prop::Pan) => {
            if let Some(m) = patch.mix.as_mut() {
                if matches!(prop, Prop::Gain) { m.gain_db = v } else { m.pan = v }
            }
            Edit::UpdateTrack { track_id, patch }
        }
        (Edit::SetMixer { mut mixer }, Prop::Gain | Prop::Pan) => {
            match t {
                Target::Bus(id) => {
                    if let Some(b) = mixer.buses.iter_mut().find(|b| b.id == id) {
                        if matches!(prop, Prop::Gain) { b.mix.gain_db = v } else { b.mix.pan = v }
                    }
                }
                _ => mixer.master.gain_db = v,
            }
            Edit::SetMixer { mixer }
        }
        (e, _) => e,
    })
}

// ---- analysis -------------------------------------------------------------------------------

fn loudness_json(l: &kimchi_audio::loudness::Loudness, target: Option<f64>) -> Value {
    let r = |x: f64| (x * 10.0).round() / 10.0;
    let silent = l.integrated <= MIN_DB + 1.0;
    let mut v = json!({
        "integrated": if silent { Value::Null } else { json!(r(l.integrated)) },
        "range": r(l.range),
        "truePeak": if l.true_peak <= MIN_DB + 1.0 { Value::Null } else { json!(r(l.true_peak)) },
        "momentaryMax": if l.momentary_max <= MIN_DB + 1.0 { Value::Null } else { json!(r(l.momentary_max)) },
        "shortTermMax": if l.short_term_max <= MIN_DB + 1.0 { Value::Null } else { json!(r(l.short_term_max)) },
        "unit": "LUFS (true peak in dBTP)",
    });
    if silent {
        v["note"] = json!("Silence: nothing to measure.");
    }
    if let (Some(t), false) = (target, silent) {
        v["target"] = json!(t);
        v["toTarget"] = json!(r(t - l.integrated));
    }
    v
}

async fn measure(s: &Arc<Session>, a: &Args) -> CmdResult {
    let p = s.project()?;
    let tools = s.tools()?;
    if let Some(k) = a.opt_str("clipId") {
        let id = resolve::clip(&p, k)?;
        let c = p.clip(id).ok_or("clip not found")?;
        if !has_sound(&p, c) {
            return Err(format!("\"{}\" has no sound.", c.name));
        }
        let l = kimchi_media::audio::clip_loudness(&tools, &p, id).await.map_err(err)?;
        let mut v = loudness_json(&l, Some(s.settings().audio.default_loudness));
        v["clip"] = json!(c.name);
        v["note"] = json!("The clip on its own at volume 1, with its effects.");
        return Ok(v);
    }
    let mut range = kimchi_media::audio::Range::default();
    let end = p.duration();
    if a.has("from") || a.has("to") {
        let from = a.opt_f64("from").unwrap_or(0.0).max(0.0);
        let to = a.opt_f64("to").unwrap_or(end);
        if to <= from {
            return Err("The span is empty: `to` must come after `from`.".into());
        }
        range.span = Some((from, to));
    }
    let mut what = "the whole mix".to_string();
    if let Some(k) = a.opt_str("trackId") {
        let id = resolve::track(&p, k)?;
        range.selection = kimchi_audio::mixer::Selection { tracks: Some(vec![id]), skip_master: true };
        what = format!("the track {}", p.track(id).map(|t| t.name.clone()).unwrap_or_default());
    }
    if end <= 0.0 {
        return Err("The timeline is empty: there is nothing to measure.".into());
    }
    let l = kimchi_media::audio::measure(&tools, &p, &range).await.map_err(err)?;
    let mut v = loudness_json(&l, p.mixer.master.loudness);
    v["measured"] = json!(what);
    if let Some((f, t)) = range.span {
        v["from"] = json!(round(f));
        v["to"] = json!(round(t));
    }
    Ok(v)
}

async fn normalize(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let ids = resolve::clips(&p, &a.strings("clipIds"))?;
    if ids.is_empty() {
        return Err("`clipIds` is empty".into());
    }
    let peak = match a.opt_str("mode").unwrap_or("loudness") {
        "loudness" | "lufs" => false,
        "peak" => true,
        other => return Err(format!("mode is loudness or peak, not `{other}`.")),
    };
    let target = a.opt_f64("target").unwrap_or(if peak { -1.0 } else { s.settings().audio.default_loudness });
    if peak && !(-40.0..=0.0).contains(&target) {
        return Err(format!("Peak targets go from -40 to 0 dBFS, not {target}."));
    }
    if !peak && !(-40.0..=-5.0).contains(&target) {
        return Err(format!("Loudness targets go from -40 to -5 LUFS, not {target}."));
    }
    for id in &ids {
        let c = p.clip(*id).ok_or("clip not found")?;
        if !has_sound(&p, c) {
            return Err(format!("\"{}\" has no sound.", c.name));
        }
    }
    let tools = s.tools()?;
    let mut edits = vec![];
    let mut out = vec![];
    for id in &ids {
        let c = p.clip(*id).ok_or("clip not found")?;
        let l = kimchi_media::audio::clip_loudness(&tools, &p, *id).await.map_err(err)?;
        let measured = if peak { l.true_peak } else { l.integrated };
        if measured <= MIN_DB + 1.0 {
            out.push(json!({ "clipId": id, "name": c.name, "note": "silent: left as it was" }));
            continue;
        }
        let volume = db_to_gain(target - measured).clamp(0.0, 4.0);
        let gain = gain_to_db(volume);
        let mut v = json!({ "clipId": id, "name": c.name, "measured": (measured * 10.0).round() / 10.0, "gainDb": (gain * 10.0).round() / 10.0, "volume": round(volume) });
        if (target - measured - gain).abs() > 0.1 {
            v["note"] = json!("kept at the most a clip can be raised (+12 dB)");
        }
        out.push(v);
        edits.push(Edit::UpdateClip { clip_id: *id, patch: ClipPatch { volume: Some(volume), ..Default::default() } });
    }
    if !edits.is_empty() {
        apply_all(s, cx, &edits, None)?;
    }
    Ok(json!({ "target": target, "unit": if peak { "dBTP" } else { "LUFS" }, "clips": out }))
}

/// Listens to a media item for its beats and stores them with it.
async fn detect_beats(s: &Arc<Session>, p: &Project, asset: Id) -> CmdResult<Beats> {
    let a = p.asset(asset).ok_or("media not found")?.clone();
    if !(a.kind == MediaKind::Audio || a.meta.has_audio) {
        return Err(format!("{} has no sound.", a.name));
    }
    let tools = s.tools()?;
    const RATE: u32 = 22_050;
    let frames = kimchi_media::audio::asset_samples(&tools, &a, RATE).await.map_err(err)?;
    let found = tokio::task::spawn_blocking(move || kimchi_audio::beats::detect(&frames, RATE)).await.map_err(err)?;
    let mut beats = found.ok_or_else(|| format!("kimchi found no steady beat in {}.", a.name))?;
    if beats.source.is_empty() {
        beats.source = "detected".into();
    }
    let mut updated = a.clone();
    updated.beats = Some(beats.clone());
    s.with_project(p.id, |ed| ed.apply(&Edit::UpdateAsset { asset: updated }, None).map(|_| ()))?.map_err(err)?;
    Ok(beats)
}

/// Timeline times of a clip's beats (every `every` beats from the first downbeat + `offset`).
pub fn clip_beats(c: &kimchi_core::Clip, b: &Beats, every: usize, offset: usize) -> Vec<f64> {
    let every = every.max(1) as i64;
    let span = c.duration * c.speed;
    b.times
        .iter()
        .enumerate()
        .filter(|(i, _)| (*i as i64 - b.first_downbeat as i64 - offset as i64).rem_euclid(every) == 0)
        .filter_map(|(_, src)| {
            let along = src - c.in_point;
            if !(0.0..=span).contains(&along) {
                return None;
            }
            let local = if c.reverse { span - along } else { along };
            Some(c.start + local / c.speed.max(1e-6))
        })
        .collect()
}

async fn beat_cut(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let music = resolve::clip(&p, a.str("musicClipId")?)?;
    let mc = p.clip(music).ok_or("clip not found")?.clone();
    let asset = mc.asset_id().filter(|_| has_sound(&p, &mc)).ok_or_else(|| format!("\"{}\" has no sound to follow.", mc.name))?;
    let beats = match p.asset(asset).and_then(|x| x.beats.clone()) {
        Some(b) => b,
        None => detect_beats(s, &p, asset).await?,
    };
    let p = s.project()?;
    let every = a.opt_i64("every").unwrap_or(4);
    if every < 1 {
        return Err("every is a number of beats, 1 or more.".into());
    }
    let offset = a.opt_i64("offset").unwrap_or(0).max(0) as usize;
    let from = a.opt_f64("from").unwrap_or(mc.start);
    let to = a.opt_f64("to").unwrap_or(mc.end());
    let times: Vec<f64> = clip_beats(&mc, &beats, every as usize, offset).into_iter().filter(|t| *t >= from - 1e-6 && *t <= to + 1e-6).collect();
    if times.is_empty() {
        return Err(format!("No beats of \"{}\" fall in that span.", mc.name));
    }
    if a.bool_or("markers", false) {
        let edits: Vec<Edit> = times.iter().enumerate().map(|(i, t)| Edit::AddMarker { time: *t, label: format!("Beat {}", i as i64 * every + 1 + offset as i64) }).collect();
        apply_all(s, cx, &edits, None)?;
        return Ok(json!({ "markers": times.len(), "times": times.iter().map(|t| round(*t)).collect::<Vec<_>>(), "tempo": beats.tempo }));
    }
    let track = match a.opt_str("trackId") {
        Some(k) => resolve::track(&p, k)?,
        None => p.tracks.iter().find(|t| t.kind == TrackKind::Video && !t.captions && !t.clips.is_empty()).map(|t| t.id).ok_or("There is no video track with clips to cut; give trackId.")?,
    };
    let t = p.track(track).ok_or("track not found")?;
    if t.locked {
        return Err(format!("\"{}\" is locked.", t.name));
    }
    let frame = p.settings.frame();
    let mut created = vec![];
    let mut cuts = 0;
    s.edit(cx.label(), cx.source, |ed| {
        ed.begin_batch(cx.label(), cx.source.as_str());
        for time in &times {
            let under: Vec<Id> = ed.project().track(track).map(|t| t.clips.iter().filter(|c| *time > c.start + frame && *time < c.end() - frame).map(|c| c.id).collect()).unwrap_or_default();
            if under.is_empty() {
                continue;
            }
            match ed.apply(&Edit::Split { time: *time, clip_ids: Some(under) }, None) {
                Ok(out) => {
                    cuts += 1;
                    created.extend(out.created_clips);
                }
                Err(e) => {
                    ed.rollback_batch();
                    return Err(err(e));
                }
            }
        }
        ed.end_batch();
        Ok(())
    })?;
    Ok(json!({ "cuts": cuts, "track": t.name, "tempo": (beats.tempo * 10.0).round() / 10.0, "every": every, "times": times.iter().map(|t| round(*t)).collect::<Vec<_>>(), "created": created }))
}

/// Words in a track's name that say it holds music, or speech.
const MUSIC_WORDS: &[&str] = &["music", "song", "score", "bgm", "beat", "soundtrack", "musique", "track bed", "bed"];
const SPEECH_WORDS: &[&str] = &["voice", "dialog", "dialogue", "vo", "narration", "speech", "mic", "interview", "voix"];

fn auto_duck(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let p = s.project()?;
    let sounding: Vec<&kimchi_core::Track> = p.tracks.iter().filter(|t| t.clips.iter().any(|c| has_sound(&p, c))).collect();
    let words = |t: &kimchi_core::Track, list: &[&str]| {
        let n = t.name.to_lowercase();
        list.iter().any(|w| n.split(|c: char| !c.is_alphanumeric()).any(|x| x == *w) || (w.len() > 3 && n.contains(w)))
    };
    let musical = |t: &kimchi_core::Track| {
        words(t, MUSIC_WORDS) || t.clips.iter().any(|c| c.asset_id().and_then(|x| p.asset(x)).is_some_and(|x| matches!(x.origin, AssetOrigin::Song(_)) || x.beats.is_some()))
    };
    let music: Vec<Id> = if a.array("music").is_some() {
        a.strings("music").iter().map(|k| resolve::track(&p, k)).collect::<CmdResult<_>>()?
    } else {
        let mut m: Vec<Id> = sounding.iter().filter(|t| musical(t) && !words(t, SPEECH_WORDS)).map(|t| t.id).collect();
        // One audio track under picture with sound: that's the music bed.
        if m.is_empty() {
            let audio: Vec<&&kimchi_core::Track> = sounding.iter().filter(|t| t.kind == TrackKind::Audio && !words(t, SPEECH_WORDS)).collect();
            if audio.len() == 1 && sounding.iter().any(|t| t.kind == TrackKind::Video) {
                m.push(audio[0].id);
            }
        }
        m
    };
    if music.is_empty() {
        return Err(format!(
            "kimchi can't tell which track is the music. Name it: audio.autoDuck music=[\"…\"]. Tracks with sound: {}.",
            list(sounding.iter().map(|t| t.name.as_str()))
        ));
    }
    let dialogue: Vec<Id> = if a.array("dialogue").is_some() {
        a.strings("dialogue").iter().map(|k| resolve::track(&p, k)).collect::<CmdResult<_>>()?
    } else {
        sounding.iter().map(|t| t.id).filter(|id| !music.contains(id)).collect()
    };
    let off = a.bool_or("off", false);
    if !off && dialogue.is_empty() {
        return Err("There is no other track with sound for the music to duck under.".into());
    }
    if let Some(both) = music.iter().find(|m| dialogue.contains(m)) {
        return Err(format!("\"{}\" can't be the music and the dialogue at once.", p.track(*both).map(|t| t.name.as_str()).unwrap_or("")));
    }
    let mut edits = vec![];
    for id in &music {
        let t = p.track(*id).ok_or("track not found")?;
        let mut mix = t.mix.clone();
        mix.duck = if off {
            None
        } else {
            let mut d = mix.duck.take().unwrap_or_default();
            d.under = dialogue.clone();
            if let Some(v) = a.opt_f64("amountDb") {
                d.amount_db = -v.abs();
            }
            if let Some(v) = a.opt_f64("thresholdDb") {
                d.threshold_db = v;
            }
            if let Some(v) = a.opt_f64("attack") {
                d.attack = v;
            }
            if let Some(v) = a.opt_f64("release") {
                d.release = v;
            }
            Some(d)
        };
        edits.push(Edit::UpdateTrack { track_id: *id, patch: TrackPatch { mix: Some(mix), ..Default::default() } });
    }
    apply_all(s, cx, &edits, None)?;
    let p = s.project()?;
    let names = |ids: &[Id]| ids.iter().filter_map(|id| p.track(*id).map(|t| t.name.clone())).collect::<Vec<_>>();
    let mut v = json!({ "music": names(&music), "dialogue": names(&dialogue), "ducked": !off });
    if let Some(d) = music.first().and_then(|id| p.track(*id)).and_then(|t| t.mix.duck.as_ref()) {
        v["duck"] = duck_json(&p, d);
    }
    Ok(v)
}

// ---- ryolune songs --------------------------------------------------------------------------

/// Whether a path is a ryolune song (`.ryolune`, or `.ondera` from before its rename).
pub fn is_song(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ryolune") || e.eq_ignore_ascii_case("ondera"))
}

fn safe(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let n = n.trim_matches('-').chars().take(40).collect::<String>();
    if n.is_empty() { "song".into() } else { n }
}

/// Renders a song (its mix, or the stem of ryolune track `stem`) into the project's generated
/// media and makes an asset of it, with its beats. Not yet added to the project.
pub async fn song_asset(s: &Arc<Session>, project: &Project, path: &Path, stem: Option<(&str, &str)>) -> CmdResult<(Asset, kimchi_audio::song::SongInfo)> {
    let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !path.is_file() {
        return Err(format!("{} doesn't exist.", path.display()));
    }
    let dir = s.library.generated_dir(project.id).join("songs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let stem_name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "song".into());
    let label = match stem {
        Some((_, track)) => format!("{stem_name}-{track}"),
        None => stem_name.clone(),
    };
    let out = dir.join(format!("{}-{}.wav", safe(&label), &new_id().simple().to_string()[..8]));
    let rate = kimchi_media::audio::rate(project);
    let (src, dst, track) = (path.clone(), out.clone(), stem.map(|(id, _)| id.to_string()));
    let modified_before = modified(&path);
    let info = tokio::task::spawn_blocking(move || kimchi_audio::song::render(&src, track.as_deref(), &dst, rate)).await.map_err(err)??;
    let tools = s.tools()?;
    let probe = kimchi_media::probe(&tools, &out).await.map_err(|e| format!("The song rendered, but its file can't be read: {e}"))?;
    let title = match stem {
        Some((_, track)) => format!("{} · {track}", info.name),
        None => info.name.clone(),
    };
    let beats = (!info.beats.is_empty()).then(|| Beats {
        tempo: info.tempo,
        beats_per_bar: info.beats_per_bar.round().max(1.0) as u32,
        times: info.beats.clone(),
        first_downbeat: 0,
        source: "ryolune".into(),
    });
    let asset = Asset {
        id: new_id(),
        name: title.clone(),
        kind: MediaKind::Audio,
        path: out.to_string_lossy().into_owned(),
        meta: probe.meta,
        origin: AssetOrigin::Song(SongRef {
            song: path.to_string_lossy().into_owned(),
            song_modified: modified_before,
            track: stem.map(|(id, _)| id.to_string()),
            title,
            tempo: info.tempo,
            beats_per_bar: info.beats_per_bar,
        }),
        created_at: chrono::Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
        beats,
    };
    Ok((asset, info))
}

async fn import_song(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let path = crate::commands::media::absolute(a.str("path")?)?;
    if !is_song(&path) {
        return Err(format!("{} isn't a ryolune song (.ryolune).", path.display()));
    }
    let stems = match a.opt_str("as").unwrap_or("mix") {
        "mix" => false,
        "stems" => true,
        other => return Err(format!("as is mix or stems, not `{other}`.")),
    };
    let p = s.project()?;
    let track = a.opt_str("trackId").map(|k| resolve::track(&p, k)).transpose()?;
    if let Some(t) = track.and_then(|t| p.track(t))
        && t.kind != TrackKind::Audio
    {
        return Err(format!("\"{}\" is a video track; songs go on audio tracks.", t.name));
    }
    let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead).max(0.0);
    let mut assets: Vec<(Asset, Option<String>)> = vec![];
    let info = if stems {
        let info = tokio::task::spawn_blocking({
            let path = path.clone();
            move || kimchi_audio::song::info(&path)
        })
        .await
        .map_err(err)??;
        if info.tracks.is_empty() {
            return Err(format!("{} has no tracks that make sound.", info.name));
        }
        for (id, name) in &info.tracks {
            let (asset, _) = song_asset(s, &p, &path, Some((id, name))).await?;
            assets.push((asset, Some(name.clone())));
        }
        info
    } else {
        let (asset, info) = song_asset(s, &p, &path, None).await?;
        assets.push((asset, None));
        info
    };
    let mut clips = vec![];
    let mut tracks = vec![];
    let markers = a.bool_or("markers", false);
    s.edit(cx.label(), cx.source, |ed| {
        ed.begin_batch(cx.label(), cx.source.as_str());
        let mut steps = || -> Result<(), kimchi_core::EditError> {
            for (asset, stem) in &assets {
                ed.apply(&Edit::AddAsset { asset: asset.clone() }, None)?;
                let track_id = match stem {
                    Some(name) => {
                        let id = *ed.apply(&Edit::AddTrack { kind: TrackKind::Audio, index: None }, None)?.created_tracks.first().ok_or(kimchi_core::EditError::Invalid("no track was made".into()))?;
                        ed.apply(&Edit::UpdateTrack { track_id: id, patch: TrackPatch { name: Some(format!("{} · {name}", info.name)), ..Default::default() } }, None)?;
                        tracks.push(id);
                        Some(id)
                    }
                    None => track,
                };
                clips.extend(ed.apply(&Edit::InsertAsset { asset_id: asset.id, track_id, start }, None)?.created_clips);
            }
            if markers {
                for (t, name) in &info.markers {
                    ed.apply(&Edit::AddMarker { time: start + t, label: name.clone() }, None)?;
                }
            }
            Ok(())
        };
        match steps() {
            Ok(()) => {
                ed.end_batch();
                Ok(())
            }
            Err(e) => {
                ed.rollback_batch();
                Err(err(e))
            }
        }
    })?;
    for (asset, _) in &assets {
        crate::commands::media::spawn_previews(s, p.id, asset.clone());
    }
    Ok(json!({
        "song": info.name,
        "tempo": info.tempo,
        "beatsPerBar": info.beats_per_bar,
        "seconds": round(info.seconds),
        "media": assets.iter().map(|(a, _)| json!({ "id": a.id, "name": a.name })).collect::<Vec<_>>(),
        "clips": clips,
        "tracks": tracks,
        "markers": if markers { info.markers.len() } else { 0 },
    }))
}

async fn refresh_songs(s: &Arc<Session>, a: &Args) -> CmdResult {
    let p = s.project()?;
    let only = if a.array("assetIds").is_some() { Some(a.strings("assetIds").iter().map(|k| resolve::asset(&p, k)).collect::<CmdResult<Vec<Id>>>()?) } else { None };
    let force = a.bool_or("force", false);
    let mut refreshed = vec![];
    let mut skipped = vec![];
    for asset in p.assets.iter().filter(|x| only.as_ref().is_none_or(|o| o.contains(&x.id))) {
        let AssetOrigin::Song(r) = &asset.origin else {
            if only.is_some() {
                return Err(format!("{} isn't a ryolune song.", asset.name));
            }
            continue;
        };
        match song_state(r) {
            "missing" => {
                skipped.push(json!({ "name": asset.name, "note": format!("{} is gone", r.song) }));
                continue;
            }
            "current" if !force => continue,
            _ => {}
        }
        let stem_name = r.title.rsplit(" · ").next().unwrap_or("").to_string();
        let stem = r.track.as_deref().map(|id| (id, stem_name.as_str()));
        let (fresh, _) = match song_asset(s, &p, Path::new(&r.song), stem).await {
            Ok(x) => x,
            Err(e) => {
                skipped.push(json!({ "name": asset.name, "note": e }));
                continue;
            }
        };
        // The same media, its new sound: every clip of it plays the new render.
        let mut updated = asset.clone();
        let old = PathBuf::from(&asset.path);
        updated.path = fresh.path;
        updated.meta = fresh.meta;
        updated.origin = fresh.origin;
        updated.beats = fresh.beats;
        updated.waveform = None;
        s.with_project(p.id, |ed| ed.apply(&Edit::UpdateAsset { asset: updated.clone() }, None).map(|_| ()))?.map_err(err)?;
        crate::commands::media::spawn_previews(s, p.id, updated.clone());
        // The old render was kimchi's own (it lives under the generated media).
        if old.starts_with(s.library.generated_dir(p.id)) {
            let _ = std::fs::remove_file(&old);
        }
        refreshed.push(json!({ "assetId": asset.id, "name": asset.name }));
    }
    Ok(json!({ "refreshed": refreshed, "skipped": skipped }))
}

/// The song file of a clip or media item.
fn song_of(p: &Project, a: &Args) -> CmdResult<SongRef> {
    let asset = match (a.opt_str("clipId"), a.opt_str("assetId")) {
        (Some(k), _) => {
            let id = resolve::clip(p, k)?;
            p.clip(id).and_then(|c| c.asset_id()).ok_or("That clip isn't a song.")?
        }
        (None, Some(k)) => resolve::asset(p, k)?,
        (None, None) => return Err("Give clipId (a song clip) or assetId.".into()),
    };
    match p.asset(asset).map(|a| &a.origin) {
        Some(AssetOrigin::Song(r)) => Ok(r.clone()),
        _ => Err("That isn't a ryolune song: only songs imported from .ryolune files open in ryolune.".into()),
    }
}

async fn open_in_ryolune(s: &Arc<Session>, a: &Args) -> CmdResult {
    let p = s.project()?;
    let r = song_of(&p, a)?;
    let path = PathBuf::from(&r.song);
    if !path.is_file() {
        return Err(format!("{} is gone.", path.display()));
    }
    // A running ryolune opens it through its bridge (unless its open song has unsaved changes).
    if let Ok(mut client) = crate::commands::handoff::RyoluneClient::connect().await {
        let info = client.call("session.info", json!({})).await.unwrap_or(Value::Null);
        let open = info.get("file").and_then(Value::as_str).map(PathBuf::from);
        if open.as_ref().is_some_and(|f| f == &path) {
            return Ok(json!({ "opened": path, "how": "already open in ryolune" }));
        }
        let dirty = info.get("dirty").and_then(Value::as_bool).unwrap_or(false);
        if dirty && !a.bool_or("force", false) {
            return Err("ryolune's open song has unsaved changes; save it there first (or pass force to lose them).".into());
        }
        client.call("session.open", json!({ "path": path })).await.map_err(|e| format!("ryolune couldn't open the song: {e}"))?;
        return Ok(json!({ "opened": path, "how": "in the running ryolune" }));
    }
    let entry = crate::discovery::find("ryolune").ok_or("ryolune isn't installed (no ~/.lsuite/apps/ryolune.json). Get it from lsuite.xyz/ryolune.")?;
    let exe = entry.executable.filter(|e| e.is_file()).ok_or("ryolune's discovery entry names no executable kimchi can start.")?;
    std::process::Command::new(&exe).arg(&path).spawn().map_err(|e| format!("Couldn't start ryolune ({}): {e}", exe.display()))?;
    Ok(json!({ "opened": path, "how": "started ryolune" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::ClipContent;

    #[test]
    fn beats_land_on_the_timeline_through_trims_and_speed() {
        let mut c = kimchi_core::Clip::new("m", 10.0, 4.0, ClipContent::Solid { color: "#000000".into() });
        c.in_point = 1.0;
        let b = Beats { tempo: 120.0, beats_per_bar: 4, times: (0..20).map(|i| i as f64 * 0.5).collect(), first_downbeat: 0, source: "detected".into() };
        // Beats from source 1.0 to 5.0, every 2: 1, 2, 3, 4, 5 → timeline 10, 11, 12, 13, 14.
        assert_eq!(clip_beats(&c, &b, 2, 0), vec![10.0, 11.0, 12.0, 13.0, 14.0]);
        c.speed = 2.0;
        c.duration = 2.0;
        // Every 4 beats: sources 2 and 4, at 1 and 3 s into the clip played twice as fast.
        assert_eq!(clip_beats(&c, &b, 4, 0), vec![10.5, 11.5]);
    }

    #[test]
    fn loudness_names_and_numbers() {
        assert_eq!(loudness(&json!("youtube")).unwrap(), Some(-14.0));
        assert_eq!(loudness(&json!("-16 LUFS")).unwrap(), Some(-16.0));
        assert_eq!(loudness(&json!(null)).unwrap(), None);
        assert!(loudness(&json!(-3)).is_err());
        assert!(loudness(&json!("loud")).unwrap_err().contains("youtube"));
    }
}
