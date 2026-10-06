//! The mixer: one strip per track with sound, the buses and the master, in the timeline's
//! area (in its place, or beside it). A strip shows its name, its effect slots (bypass, open,
//! reorder by dragging, remove, add from the browser), its sends, pan, the fader with its dB
//! scale beside the meter (peak, RMS, hold, clip light, ducking), the level (click to type),
//! mute / solo / record arm and where it goes. The master adds the loudness readout (momentary,
//! short-term, integrated over the last play), the limiter and the loudness target.
//!
//! Strips are solid work surfaces. Every change is an `audio.*` command; a drag sends one
//! coalesce key, so a fader move is one undo step, and an automated fader or pan writes a
//! keyframe at the playhead instead (as the inspector's controls do). The strips give up
//! detail as the area gets short (effects and sends fold into chips), and scroll sideways
//! when they don't fit, the master pinned on the right.

pub mod browser;
pub mod effect_panel;
pub mod export;
pub mod live;
pub mod recording;
pub mod scrub;
#[cfg(test)]
mod tests;
pub mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Context, ElementId, Entity, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollHandle,
    SharedString, Subscription, Window, canvas, div, prelude::*, px, relative,
};
pub use kimchi_control::commands::audio::Target;
use kimchi_core::audio::{Insert, MIN_DB};
use kimchi_core::{Id, Project, TrackKind};
use serde_json::{Value, json};

use crate::playback::Playback;
use crate::store::{MenuEntry, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, drag, icon, tooltip};
use widgets::{Reading, db_text, db_to_pos, pan_text, pos_to_db, scale_text};

/// What the window's audio views show (`Store::audio`; `ui.state` reports it as `audio`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioView {
    /// The mixer is shown.
    pub mixer: bool,
    /// Beside the timeline (else in its place).
    pub beside: bool,
    /// The strip last clicked: the inspector shows its track's mix when no clip is selected.
    pub strip: Option<Target>,
    /// The effect whose panel is open: its chain's owner and its slot id.
    pub effect: Option<(Target, String)>,
    /// The effect browser, open to add to this chain, where it was asked for.
    pub browser: Option<(Target, Point<Pixels>)>,
    /// A voice-over take in progress.
    pub recording: Option<Recording>,
}

/// A voice-over take: counting in, then recording onto `track` from `start`.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub track: Id,
    /// Timeline time the take starts at.
    pub start: f64,
    /// Counting in until this many seconds have passed (0 once recording).
    pub count_in: f64,
    /// When the count-in (or the take) started.
    pub since: std::time::Instant,
}

impl AudioView {
    /// The audio part of `ui.state`.
    pub fn to_json(&self, p: Option<&Project>) -> Value {
        let target = |t: &Target| {
            let mut v = t.to_json();
            if let Some(p) = p {
                v["name"] = json!(t.name(p));
            }
            v
        };
        json!({
            "mixer": self.mixer,
            "layout": if self.beside { "beside" } else { "replace" },
            "strip": self.strip.as_ref().map(target),
            "effect": self.effect.as_ref().map(|(t, slot)| {
                let mut v = target(t);
                v["slot"] = json!(slot);
                if let Some(e) = p.and_then(|p| t.chain(p).ok()).and_then(|c| c.into_iter().find(|e| &e.id == slot)) {
                    v["effect"] = json!(e.name);
                }
                v
            }),
            "browser": self.browser.as_ref().map(|(t, _)| target(t)),
            "recording": self.recording.as_ref().map(|r| json!({ "trackId": r.track, "start": r.start, "countingIn": r.count_in > r.since.elapsed().as_secs_f64() })),
        })
    }

    /// A project was switched: what pointed into the old one goes.
    pub fn forget_project(&mut self) {
        self.strip = None;
        self.effect = None;
        self.browser = None;
        self.recording = None;
    }
}

/// The command key of a target (`track:<id>`, `bus:<id>`, `clip:<id>`, `master`).
pub fn target_key(t: Target) -> String {
    match t {
        Target::Track(id) => format!("track:{id}"),
        Target::Bus(id) => format!("bus:{id}"),
        Target::Clip(id) => format!("clip:{id}"),
        Target::Master => "master".into(),
    }
}

/// The meter key of a target.
fn meter_key(t: Target) -> String {
    match t {
        Target::Track(id) => live::key_of("track", Some(id)),
        Target::Bus(id) => live::key_of("bus", Some(id)),
        Target::Clip(id) => live::key_of("clip", Some(id)),
        Target::Master => "master".into(),
    }
}

/// Renders the project's ryolune songs again when their files were saved since (a project
/// opening, the window coming back to the front).
pub fn refresh_songs(s: &mut Store, cx: &mut Context<Store>) {
    let Some(p) = s.project.clone() else { return };
    if !s.settings.audio.refresh_songs {
        return;
    }
    let changed = p.assets.iter().any(|a| matches!(&a.origin, kimchi_core::AssetOrigin::Song(r) if kimchi_control::commands::audio::song_state(r) == "changed"));
    if !changed {
        return;
    }
    s.run_then("audio.refreshSongs", json!({}), cx, |s, v, cx| {
        let n = v["refreshed"].as_array().map(Vec::len).unwrap_or(0);
        if n > 0 {
            s.flash(format!("Updated {} from ryolune", crate::app::count(n, "song")), cx);
        }
    });
}

// ---- window-only commands ---------------------------------------------------------------------

/// `audio.meters`, `audio.devices`, `audio.record`, `audio.showMixer`, for every client.
pub fn ui_command(command: &str, params: Value, cx: &mut App) -> kimchi_control::CmdResult {
    let store = cx.store();
    match command {
        "audio.meters" => {
            let pb = store.read(cx).playback.read(cx);
            let (playhead, playing) = (pb.playhead, pb.playing);
            Ok(live::meters_json(cx, playhead, playing))
        }
        "audio.devices" => Ok(devices_json(&store.read(cx).settings)),
        "audio.record" => recording::command(&params, cx),
        "audio.showMixer" => {
            if store.read(cx).project.is_none() {
                return Err(kimchi_control::session::NO_PROJECT.into());
            }
            store.update(cx, |s, cx| {
                s.set_audio(
                    |a| {
                        if let Some(open) = params["open"].as_bool() {
                            a.mixer = open;
                        } else if params.get("target").is_none() && params.get("closeEffect").is_none() {
                            a.mixer = true;
                        }
                        match params["layout"].as_str() {
                            Some("beside") => a.beside = true,
                            Some("replace") => a.beside = false,
                            _ => {}
                        }
                        if params["closeEffect"].as_bool() == Some(true) {
                            a.effect = None;
                        }
                        if let (Some(t), Some(slot)) = (Target::from_json(&params["target"]), params["slot"].as_str()) {
                            a.effect = Some((t, slot.to_string()));
                        }
                    },
                    cx,
                )
            });
            let s = store.read(cx);
            Ok(s.audio.to_json(s.project.as_deref()))
        }
        other => Err(format!("the window doesn't handle `{other}`")),
    }
}

/// Outputs and inputs on this computer, and the ones the settings ask for.
pub fn devices_json(settings: &kimchi_control::Settings) -> Value {
    let outputs = kimchi_audio::devices::outputs();
    let inputs = kimchi_audio::devices::inputs();
    let names = |list: &[kimchi_audio::devices::Device]| list.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
    let default = |list: &[kimchi_audio::devices::Device]| list.iter().find(|d| d.default).map(|d| d.name.clone());
    json!({
        "outputs": names(&outputs),
        "inputs": names(&inputs),
        "defaultOutput": default(&outputs),
        "defaultInput": default(&inputs),
        "output": if settings.audio.output_device.is_empty() { Value::Null } else { json!(settings.audio.output_device) },
        "input": if settings.audio.input_device.is_empty() { Value::Null } else { json!(settings.audio.input_device) },
        "devices": { "outputs": outputs, "inputs": inputs },
    })
}

// ---- the view ---------------------------------------------------------------------------------

enum MixDrag {
    Fader { target: Target, y0: Pixels, pos0: f32, travel: f32, key: String, automated: bool },
    Pan { target: Target, y0: Pixels, v0: f64, key: String, automated: bool },
    Send { track: Id, bus: Id, x0: Pixels, v0: f32, width: f32, key: String },
    Slot { target: Target, from: usize, to: usize, len: usize, y0: Pixels, started: bool },
}

/// How much a strip shows, from the height it has.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Detail {
    /// Name, fader and meter, level, mute and solo.
    Small,
    /// Effects and sends as chips.
    Medium,
    Full,
}

const STRIP_W: f32 = 88.;
const MASTER_W: f32 = 118.;
const SLOT_H: f32 = 18.;
/// Effect slots a full strip lists before folding the rest into "N more".
const SLOTS_SHOWN: usize = 3;

/// A strip's facts for one frame.
struct Strip {
    target: Target,
    name: String,
    kind: StripKind,
    gain_db: f64,
    gain_automated: bool,
    pan: Option<f64>,
    pan_automated: bool,
    muted: bool,
    solo: bool,
    armed: Option<bool>,
    output: Option<String>,
    effects: Vec<Insert>,
    sends: Vec<(Id, String, f64)>,
    duck: Option<f64>,
}

#[derive(Clone, Copy, PartialEq)]
enum StripKind {
    Audio,
    Video,
    Bus,
    Master,
}

pub struct MixerView {
    store: Entity<Store>,
    playback: Entity<Playback>,
    drag: Option<MixDrag>,
    drags: u64,
    /// Values shown while a drag's commands are on their way (by control key).
    shown: HashMap<String, f64>,
    /// After a drag: the project its last command was sent on; `shown` clears once it changes.
    settle: Option<usize>,
    /// The strips' area as last drawn.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Each fader's travel as last drawn, by meter key.
    faders: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    scroll: ScrollHandle,
    typing: Option<(Target, Entity<TextInput>, Subscription)>,
    _subs: Vec<Subscription>,
}

fn project_key(p: &Arc<Project>) -> usize {
    Arc::as_ptr(p) as usize
}

impl MixerView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            // Meters move while playing.
            cx.observe(&playback, |_, pb, cx| {
                if pb.read(cx).playing {
                    cx.notify();
                }
            }),
        ];
        Self {
            store,
            playback,
            drag: None,
            drags: 0,
            shown: HashMap::new(),
            settle: None,
            bounds: Rc::default(),
            faders: Rc::default(),
            scroll: ScrollHandle::new(),
            typing: None,
            _subs: subs,
        }
    }

    // ---- reading the project -------------------------------------------------------------

    fn strips(&self, p: &Project, playhead: f64) -> Vec<Strip> {
        let sounding = |t: &kimchi_core::Track| t.kind == TrackKind::Audio || t.clips.iter().any(|c| kimchi_control::commands::audio::has_sound(p, c));
        let mut out = vec![];
        for t in p.tracks.iter().filter(|t| !t.captions && sounding(t)) {
            let m = &t.mix;
            out.push(Strip {
                target: Target::Track(t.id),
                name: t.name.clone(),
                kind: if t.kind == TrackKind::Audio { StripKind::Audio } else { StripKind::Video },
                gain_db: m.gain_db_at(playhead),
                gain_automated: m.keyframes.contains_key("gainDb"),
                pan: Some(m.pan_at(playhead)),
                pan_automated: m.keyframes.contains_key("pan"),
                muted: t.muted,
                solo: m.solo,
                armed: (t.kind == TrackKind::Audio).then_some(m.armed),
                output: Some(m.output.and_then(|b| p.mixer.bus(b)).map(|b| b.name.clone()).unwrap_or_else(|| "Master".into())),
                effects: m.effects.clone(),
                sends: m.sends.iter().filter_map(|s| p.mixer.bus(s.bus).map(|b| (b.id, b.name.clone(), s.level_db))).collect(),
                duck: m.duck.as_ref().map(|d| d.amount_db),
            });
        }
        for b in &p.mixer.buses {
            out.push(Strip {
                target: Target::Bus(b.id),
                name: b.name.clone(),
                kind: StripKind::Bus,
                gain_db: b.mix.gain_db_at(playhead),
                gain_automated: b.mix.keyframes.contains_key("gainDb"),
                pan: Some(b.mix.pan_at(playhead)),
                pan_automated: b.mix.keyframes.contains_key("pan"),
                muted: b.muted,
                solo: b.mix.solo,
                armed: None,
                output: None,
                effects: b.mix.effects.clone(),
                sends: vec![],
                duck: None,
            });
        }
        out
    }

    fn master(&self, p: &Project, playhead: f64) -> Strip {
        let m = &p.mixer.master;
        Strip {
            target: Target::Master,
            name: "Master".into(),
            kind: StripKind::Master,
            gain_db: kimchi_core::anim::number_at(&m.keyframes, "gainDb", playhead).unwrap_or(m.gain_db),
            gain_automated: m.keyframes.contains_key("gainDb"),
            pan: None,
            pan_automated: false,
            muted: false,
            solo: false,
            armed: None,
            output: None,
            effects: m.effects.clone(),
            sends: vec![],
            duck: None,
        }
    }

    // ---- changes ---------------------------------------------------------------------------

    fn run(&self, name: &str, params: Value, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| s.run(name, params, cx));
    }

    /// The fader of `target` to `db` (a keyframe at the playhead when it is automated).
    fn set_gain(&self, target: Target, db: f64, automated: bool, key: Option<&str>, cx: &mut Context<Self>) {
        let mut params = if automated {
            json!({ "target": target_key(target), "property": "gainDb", "value": db })
        } else {
            match target {
                Target::Track(id) => json!({ "trackId": id, "gainDb": db }),
                Target::Bus(id) => json!({ "busId": id, "gainDb": db }),
                _ => json!({ "gainDb": db }),
            }
        };
        if let Some(k) = key {
            params["coalesce"] = json!(k);
        }
        let name = match (automated, target) {
            (true, _) => "audio.addAutomationKey",
            (_, Target::Track(_)) => "audio.setTrack",
            (_, Target::Bus(_)) => "audio.setBus",
            _ => "audio.setMaster",
        };
        self.run(name, params, cx);
    }

    fn set_pan(&self, target: Target, pan: f64, automated: bool, key: Option<&str>, cx: &mut Context<Self>) {
        let mut params = if automated {
            json!({ "target": target_key(target), "property": "pan", "value": pan })
        } else {
            match target {
                Target::Track(id) => json!({ "trackId": id, "pan": pan }),
                Target::Bus(id) => json!({ "busId": id, "pan": pan }),
                _ => return,
            }
        };
        if let Some(k) = key {
            params["coalesce"] = json!(k);
        }
        let name = match (automated, target) {
            (true, _) => "audio.addAutomationKey",
            (_, Target::Track(_)) => "audio.setTrack",
            _ => "audio.setBus",
        };
        self.run(name, params, cx);
    }

    fn toggle(&self, target: Target, field: &'static str, on: bool, cx: &mut Context<Self>) {
        match target {
            Target::Track(id) => self.run("audio.setTrack", json!({ "trackId": id, field: on }), cx),
            Target::Bus(id) => self.run("audio.setBus", json!({ "busId": id, field: on }), cx),
            _ => {}
        }
    }

    fn select(&self, target: Target, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            if !s.selection.is_empty() {
                s.clear_selection(cx);
            }
            s.set_audio(|a| a.strip = Some(target), cx);
        });
    }

    fn next_key(&mut self, what: &str) -> String {
        self.drags += 1;
        format!("mixer-{what}-{}", self.drags)
    }

    // ---- pointer ---------------------------------------------------------------------------

    fn fader_down(&mut self, s: &Strip, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let target = s.target;
        if e.click_count == 2 {
            // Back to unity.
            self.set_gain(target, 0.0, s.gain_automated, None, cx);
            return;
        }
        let travel = self.faders.borrow().get(&meter_key(target)).map(|b| f32::from(b.size.height)).unwrap_or(120.).max(20.);
        let key = self.next_key("fader");
        let db = self.shown.get(&format!("fader:{}", meter_key(target))).copied().unwrap_or(s.gain_db);
        self.drag = Some(MixDrag::Fader { target, y0: e.position.y, pos0: db_to_pos(db), travel, key, automated: s.gain_automated });
        cx.notify();
    }

    fn pan_down(&mut self, s: &Strip, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let target = s.target;
        if e.click_count == 2 {
            self.set_pan(target, 0.0, s.pan_automated, None, cx);
            return;
        }
        let key = self.next_key("pan");
        self.drag = Some(MixDrag::Pan { target, y0: e.position.y, v0: s.pan.unwrap_or(0.0), key, automated: s.pan_automated });
        cx.notify();
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let fine = if e.modifiers.shift { 0.15 } else { 1.0 };
        match &mut self.drag {
            Some(MixDrag::Fader { target, y0, pos0, travel, key, automated }) => {
                let pos = (*pos0 + f32::from(*y0 - e.position.y) / *travel * fine).clamp(0., 1.);
                let db = pos_to_db(pos);
                let (target, key, automated) = (*target, key.clone(), *automated);
                if self.shown.insert(format!("fader:{}", meter_key(target)), db) != Some(db) {
                    self.set_gain(target, db, automated, Some(&key), cx);
                }
            }
            Some(MixDrag::Pan { target, y0, v0, key, automated }) => {
                let v = ((*v0 + f32::from(*y0 - e.position.y) as f64 / 80. * fine as f64).clamp(-1., 1.) * 100.).round() / 100.;
                let (target, key, automated) = (*target, key.clone(), *automated);
                if self.shown.insert(format!("pan:{}", meter_key(target)), v) != Some(v) {
                    self.set_pan(target, v, automated, Some(&key), cx);
                }
            }
            Some(MixDrag::Send { track, bus, x0, v0, width, key }) => {
                let pos = (*v0 + f32::from(e.position.x - *x0) / *width * fine).clamp(0., 1.);
                let db = pos_to_db(pos);
                let (track, bus, key) = (*track, *bus, key.clone());
                if self.shown.insert(format!("send:{track}:{bus}"), db) != Some(db) {
                    self.run("audio.setSend", json!({ "trackId": track, "busId": bus, "levelDb": db, "coalesce": key }), cx);
                }
            }
            Some(MixDrag::Slot { from, to, len, y0, started, .. }) => {
                let dy = f32::from(e.position.y - *y0);
                if !*started && dy.abs() < 4. {
                    return;
                }
                *started = true;
                *to = ((*from as f32 + (dy / (SLOT_H + 2.)).round()).max(0.) as usize).min(len.saturating_sub(1));
            }
            None => return,
        }
        cx.notify();
    }

    fn drag_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        if let MixDrag::Slot { target, from, to, started, .. } = drag {
            if started && to != from {
                self.run("audio.moveEffect", json!({ "target": target_key(target), "slot": from + 1, "index": to }), cx);
            } else if !started {
                // A click: open the effect's panel.
                let p = self.store.read(cx).project.clone();
                if let Some(slot) = p.and_then(|p| target.chain(&p).ok()).and_then(|c| c.get(from).map(|e| e.id.clone())) {
                    self.store.update(cx, |s, cx| s.set_audio(|a| a.effect = Some((target, slot)), cx));
                }
            }
        }
        self.settle = self.store.read(cx).project.as_ref().map(project_key);
        cx.notify();
    }

    fn start_typing(&mut self, target: Target, db: f64, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx);
            i.mono = true;
            i.set_text(db_text(db), cx);
            i.select_all_text(cx);
            i
        });
        crate::ui::input::focus(&input, window, cx);
        let automated = self.store.read(cx).project.as_ref().is_some_and(|p| target.keyframes(p).is_some_and(|k| k.contains_key("gainDb")));
        let sub = cx.subscribe(&input, move |this, input, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Blur => {
                if let Some(db) = parse_db(input.read(cx).text()) {
                    this.set_gain(target, db, automated, None, cx);
                }
                this.typing = None;
                cx.notify();
            }
            InputEvent::Cancel => {
                this.typing = None;
                cx.notify();
            }
            InputEvent::Changed(_) => {}
        });
        self.typing = Some((target, input, sub));
        cx.notify();
    }

    // ---- menus -----------------------------------------------------------------------------

    fn output_menu(&self, track: Id, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(p) = self.store.read(cx).project.clone() else { return };
        let current = p.track(track).and_then(|t| t.mix.output);
        let mut entries = vec![MenuItem::new(if current.is_none() { "✓ Master" } else { "Master" }, move |_, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": track, "output": "master" }))).entry()];
        for b in &p.mixer.buses {
            let (id, name) = (b.id, b.name.clone());
            let label = if current == Some(id) { format!("✓ {name}") } else { name };
            entries.push(MenuItem::new(label, move |_, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": track, "output": id }))).entry());
        }
        entries.push(MenuEntry::Separator);
        entries.push(MenuItem::new("New bus…", move |_, cx| {
            cx.store().update(cx, |s, cx| {
                s.run_then("audio.addBus", json!({}), cx, move |s, v, cx| {
                    if let Some(id) = v["id"].as_str() {
                        s.run("audio.setTrack", json!({ "trackId": track, "output": id }), cx);
                    }
                })
            })
        }).icon("plus").entry());
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn strip_menu(&self, strip: &Strip, position: Point<Pixels>, cx: &mut Context<Self>) {
        let t = strip.target;
        let key = target_key(t);
        let mut entries = vec![];
        if strip.gain_automated {
            entries.push(MenuItem::new("Remove the fader's automation", move |_, cx| run_cmd(cx, "audio.removeAutomationKey", json!({ "target": key, "property": "gainDb" }))).icon("x").entry());
        } else {
            let params = match t {
                Target::Track(id) => ("audio.setTrack", json!({ "trackId": id, "gainDb": 0 })),
                Target::Bus(id) => ("audio.setBus", json!({ "busId": id, "gainDb": 0 })),
                _ => ("audio.setMaster", json!({ "gainDb": 0 })),
            };
            entries.push(MenuItem::new("Fader to 0 dB", move |_, cx| run_cmd(cx, params.0, params.1.clone())).icon("rotate-ccw").entry());
        }
        entries.push(MenuItem::new("Automate the fader here", {
            let key = target_key(t);
            move |_, cx| run_cmd(cx, "audio.addAutomationKey", json!({ "target": key, "property": "gainDb" }))
        }).icon("diamond").entry());
        match t {
            Target::Track(id) => {
                entries.push(MenuItem::new("Add effect…", move |w, cx| open_browser(Target::Track(id), w.mouse_position(), cx)).icon("plus").entry());
                if !strip.effects.is_empty() {
                    entries.push(MenuItem::new("Copy effects to every track", move |_, cx| {
                        let Some(p) = cx.store().read(cx).project.clone() else { return };
                        let to: Vec<String> = p.tracks.iter().filter(|x| x.id != id && !x.captions).map(|x| target_key(Target::Track(x.id))).collect();
                        run_cmd(cx, "audio.copyEffects", json!({ "from": target_key(Target::Track(id)), "to": to }));
                    }).icon("copy").entry());
                }
                entries.push(MenuEntry::Separator);
                if strip.duck.is_some() {
                    entries.push(MenuItem::new("Stop ducking", move |_, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": id, "duck": false }))).icon("arrow-up").entry());
                } else {
                    entries.push(MenuItem::new("Duck under the other tracks", move |_, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": id, "duck": true }))).icon("arrow-down").entry());
                }
                entries.push(MenuItem::new("Measure loudness", move |_, cx| measure(json!({ "trackId": id }), cx)).icon("gauge").entry());
            }
            Target::Bus(id) => {
                entries.push(MenuItem::new("Add effect…", move |w, cx| open_browser(Target::Bus(id), w.mouse_position(), cx)).icon("plus").entry());
                entries.push(MenuEntry::Separator);
                entries.push(MenuItem::new("Remove bus", move |_, cx| run_cmd(cx, "audio.removeBus", json!({ "busId": id }))).icon("trash").danger().entry());
            }
            _ => {
                entries.push(MenuItem::new("Add effect…", |w, cx| open_browser(Target::Master, w.mouse_position(), cx)).icon("plus").entry());
                entries.push(MenuItem::new("Measure the mix", |_, cx| measure(json!({}), cx)).icon("gauge").entry());
            }
        }
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn effects_menu(&self, target: Target, effects: &[Insert], position: Point<Pixels>, cx: &mut Context<Self>) {
        let mut entries: Vec<MenuEntry> = effects
            .iter()
            .map(|e| {
                let slot = e.id.clone();
                let label = if e.is_bypassed() { format!("{} (off)", e.name) } else { e.name.clone() };
                MenuItem::new(label, move |_, cx| {
                    let slot = slot.clone();
                    cx.store().update(cx, |s, cx| s.set_audio(|a| a.effect = Some((target, slot)), cx))
                })
                .icon("sliders-horizontal")
                .entry()
            })
            .collect();
        entries.push(MenuEntry::Separator);
        entries.push(MenuItem::new("Add effect…", move |w, cx| open_browser(target, w.mouse_position(), cx)).icon("plus").entry());
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn send_menu(&self, track: Id, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(p) = self.store.read(cx).project.clone() else { return };
        let t = p.track(track).cloned();
        let mut entries = vec![];
        for b in &p.mixer.buses {
            let id = b.id;
            let has = t.as_ref().is_some_and(|t| t.mix.sends.iter().any(|s| s.bus == id));
            if has {
                entries.push(MenuItem::new(format!("Stop sending to {}", b.name), move |_, cx| run_cmd(cx, "audio.removeSend", json!({ "trackId": track, "busId": id }))).icon("x").entry());
            } else if t.as_ref().is_some_and(|t| t.mix.output != Some(id)) {
                entries.push(MenuItem::new(format!("Send to {}", b.name), move |_, cx| run_cmd(cx, "audio.setSend", json!({ "trackId": track, "busId": id, "levelDb": -12 }))).icon("corner-down-right").entry());
            }
        }
        if !entries.is_empty() {
            entries.push(MenuEntry::Separator);
        }
        entries.push(MenuItem::new("New bus with a reverb", move |_, cx| {
            cx.store().update(cx, |s, cx| {
                s.run_then("audio.addBus", json!({ "name": "Reverb", "effect": "Space" }), cx, move |s, v, cx| {
                    if let Some(id) = v["id"].as_str() {
                        s.run("audio.setSend", json!({ "trackId": track, "busId": id, "levelDb": -12 }), cx);
                    }
                })
            })
        }).icon("plus").entry());
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    // ---- drawing ---------------------------------------------------------------------------

    fn detail(&self) -> Detail {
        let h = f32::from(self.bounds.get().size.height);
        if h <= 1. || h >= 330. {
            Detail::Full
        } else if h >= 236. {
            Detail::Medium
        } else {
            Detail::Small
        }
    }

    fn strip_el(&mut self, s: Strip, readings: (Vec<Reading>, bool), ducking: f64, detail: Detail, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let store = self.store.read(cx);
        let selected = store.audio.strip == Some(s.target) && store.selection.is_empty();
        let open_effect = store.audio.effect.clone();
        let target = s.target;
        let mkey = meter_key(target);
        let width = if s.kind == StripKind::Master { MASTER_W } else { STRIP_W };
        let gain = self.shown.get(&format!("fader:{mkey}")).copied().unwrap_or(s.gain_db);
        let pan = s.pan.map(|p| self.shown.get(&format!("pan:{mkey}")).copied().unwrap_or(p));
        let color = match s.kind {
            StripKind::Audio => t.clip_audio,
            StripKind::Video => t.clip_video,
            StripKind::Bus => t.accent_soft,
            StripKind::Master => t.accent,
        };
        let id_base: SharedString = format!("strip-{mkey}").into();
        let strip = Rc::new(s);

        // Name.
        let header = {
            let st = strip.clone();
            div()
                .id(ElementId::Name(format!("{id_base}-name").into()))
                .flex_none()
                .w_full()
                .h(px(22.))
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(4.))
                .rounded(px(sz::R_XS))
                .cursor_pointer()
                .hover(|d| d.bg(t.hover))
                .child(div().flex_none().w(px(3.)).h(px(12.)).bg(color))
                .child(div().flex_1().min_w_0().truncate().text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(strip.name.clone()))
                .tooltip({
                    let name: SharedString = strip.name.clone().into();
                    move |_, cx| tooltip(name.clone(), cx)
                })
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.strip_menu(&st, e.position, cx);
                }))
        };

        // Effect slots.
        let slots: Option<AnyElement> = match detail {
            Detail::Small => None,
            Detail::Medium => {
                let n = strip.effects.len();
                let st = strip.clone();
                Some(
                    div()
                        .id(ElementId::Name(format!("{id_base}-fx").into()))
                        .flex_none()
                        .w_full()
                        .h(px(SLOT_H))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(4.))
                        .rounded(px(sz::R_XS))
                        .bg(t.bg_sunken)
                        .text_size(px(10.5))
                        .text_color(if n > 0 { t.text } else { t.text_3 })
                        .cursor_pointer()
                        .hover(|d| d.bg(t.hover))
                        .child(icon(if n > 0 { "sliders-horizontal" } else { "plus" }).size(px(11.)))
                        .child(if n > 0 { format!("{n} FX") } else { "FX".into() })
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            if st.effects.is_empty() {
                                open_browser(st.target, e.position(), cx);
                            } else {
                                this.effects_menu(st.target, &st.effects, e.position(), cx);
                            }
                        }))
                        .into_any_element(),
                )
            }
            Detail::Full => Some(self.slots_el(&strip, open_effect.as_ref(), cx)),
        };

        // Sends.
        let sends: Option<AnyElement> = match (detail, strip.kind) {
            (Detail::Full, StripKind::Audio | StripKind::Video) => Some(self.sends_el(&strip, cx)),
            _ => None,
        };

        // Pan.
        let pan_el = pan.map(|v| {
            let st = strip.clone();
            let active = matches!(&self.drag, Some(MixDrag::Pan { target: dt, .. }) if *dt == target);
            div()
                .id(ElementId::Name(format!("{id_base}-pan").into()))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .cursor(gpui::CursorStyle::ResizeUpDown)
                .tooltip(move |_, cx| tooltip(format!("Pan {} · drag up or down · double-click to centre", pan_text(v)).into(), cx))
                .child(widgets::knob(((v + 1.) / 2.) as f32, true, active, &t).size(px(if detail == Detail::Small { 18. } else { 24. })))
                .child(div().w(px(26.)).font_family(MONO).text_size(px(10.)).text_color(if strip.pan_automated { t.accent_text } else { t.text_2 }).child(pan_text(v)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.pan_down(&st, e, cx);
                }))
        });

        // Fader, scale and meter.
        let fader = self.fader_el(&strip, gain, readings, ducking, &id_base, cx);

        // Level readout (click to type).
        let readout = match &self.typing {
            Some((tt, input, _)) if *tt == target => div().w_full().h(px(20.)).child(input.clone()).into_any_element(),
            _ => div()
                .id(ElementId::Name(format!("{id_base}-db").into()))
                .flex_none()
                .w_full()
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(sz::R_XS))
                .font_family(MONO)
                .text_size(px(sz::XS))
                .text_color(if strip.gain_automated { t.accent_text } else { t.text })
                .cursor_text()
                .hover(|d| d.bg(t.hover))
                .tooltip(move |_, cx| tooltip("Click to type a level in dB".into(), cx))
                .child(format!("{} dB", db_text(gain)))
                .on_click(cx.listener(move |this, _, window, cx| this.start_typing(target, gain, window, cx)))
                .into_any_element(),
        };

        // Mute, solo, record arm.
        let keys = (strip.kind != StripKind::Master).then(|| {
            let key_btn = |name: &'static str, label: &'static str, on: bool, on_color: gpui::Hsla, tip: &'static str| {
                div()
                    .id(ElementId::Name(format!("{id_base}-{name}").into()))
                    .flex_1()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(sz::R_XS))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::BOLD)
                    .font_family(MONO)
                    .border_1()
                    .border_color(if on { on_color } else { t.line })
                    .bg(if on { on_color.opacity(0.22) } else { t.bg_sunken })
                    .text_color(if on { on_color } else { t.text_2 })
                    .cursor_pointer()
                    .hover(|d| d.border_color(t.line_strong))
                    .tooltip(move |_, cx| tooltip(tip.into(), cx))
                    .child(label)
            };
            let (muted, solo) = (strip.muted, strip.solo);
            div()
                .flex_none()
                .w_full()
                .flex()
                .gap(px(3.))
                .child(key_btn("mute", "M", muted, t.warning, "Mute").on_click(cx.listener(move |this, _, _, cx| this.toggle(target, "muted", !muted, cx))))
                .child(key_btn("solo", "S", solo, t.accent_text, "Solo").on_click(cx.listener(move |this, _, _, cx| this.toggle(target, "solo", !solo, cx))))
                .when_some(strip.armed, |d, armed| {
                    d.child(key_btn("arm", "R", armed, t.danger, "Arm for a voice-over take").on_click(cx.listener(move |this, _, _, cx| this.toggle(target, "armed", !armed, cx))))
                })
        });

        // Where it goes.
        let output = strip.output.clone().filter(|_| detail != Detail::Small).and_then(|o| match target {
            Target::Track(track) => Some(
                div()
                    .id(ElementId::Name(format!("{id_base}-out").into()))
                    .flex_none()
                    .w_full()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .px(px(4.))
                    .rounded(px(sz::R_XS))
                    .bg(t.bg_sunken)
                    .text_size(px(10.))
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover))
                    .tooltip(|_, cx| tooltip("Where this track goes: the master or a bus".into(), cx))
                    .child(icon("arrow-right").size(px(10.)))
                    .child(div().min_w_0().truncate().child(o))
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| this.output_menu(track, e.position(), cx))),
            ),
            _ => None,
        });

        let master_extras = (strip.kind == StripKind::Master).then(|| self.master_extras(detail, cx));
        let st = strip.clone();
        div()
            .id(ElementId::Name(id_base.clone()))
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(if detail == Detail::Small { 3. } else { 5. }))
            .px(px(5.))
            .py(px(if detail == Detail::Small { 4. } else { 6. }))
            .rounded(px(sz::R_SM))
            .bg(t.bg_raised)
            .border_1()
            .border_color(if selected { t.accent } else { t.line })
            .overflow_y_scroll()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| this.select(target, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| this.strip_menu(&st, e.position, cx)))
            .child(header)
            .children(slots)
            .children(sends)
            .children(master_extras)
            .children(pan_el)
            .child(fader)
            .child(readout)
            .children(keys)
            .children(output)
            .into_any_element()
    }

    fn slots_el(&mut self, strip: &Rc<Strip>, open: Option<&(Target, String)>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let target = strip.target;
        let mkey = meter_key(target);
        let n = strip.effects.len();
        let reorder = match &self.drag {
            Some(MixDrag::Slot { target: dt, from, to, started: true, .. }) if *dt == target => Some((*from, *to)),
            _ => None,
        };
        let shown = n.min(SLOTS_SHOWN);
        let mut rows: Vec<AnyElement> = vec![];
        for (i, e) in strip.effects.iter().enumerate().take(shown) {
            let bypassed = e.is_bypassed();
            let is_open = open.is_some_and(|(ot, slot)| *ot == target && *slot == e.id);
            let slot = e.id.clone();
            let dragging = reorder.is_some_and(|(from, _)| from == i);
            let drop_here = reorder.is_some_and(|(from, to)| to == i && from != i);
            let name: SharedString = e.name.clone().into();
            rows.push(
                div()
                    .id(ElementId::Name(format!("fx-{mkey}-{i}").into()))
                    .group(SharedString::from(format!("fx-{mkey}-{i}")))
                    .flex_none()
                    .w_full()
                    .h(px(SLOT_H))
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .pl(px(3.))
                    .pr(px(2.))
                    .rounded(px(sz::R_XS))
                    .bg(if is_open { t.accent_soft } else { t.bg_sunken })
                    .border_1()
                    .border_color(if drop_here { t.accent } else if is_open { t.accent_ring } else { gpui::transparent_black() })
                    .when(dragging, |d| d.opacity(0.6))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover))
                    .tooltip({
                        let name = name.clone();
                        move |_, cx| tooltip(format!("{name} · click to open · drag to reorder").into(), cx)
                    })
                    .child(
                        div()
                            .id(ElementId::Name(format!("fx-{mkey}-{i}-power").into()))
                            .flex_none()
                            .size(px(9.))
                            .border_1()
                            .border_color(if bypassed { t.text_3 } else { t.accent })
                            .bg(if bypassed { gpui::transparent_black() } else { t.accent })
                            .tooltip(move |_, cx| tooltip(if bypassed { "Switch on" } else { "Bypass" }.into(), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click({
                                let slot = slot.clone();
                                move |_, _, cx| run_cmd(cx, "audio.setEffect", json!({ "target": target_key(target), "slot": slot, "bypassed": !bypassed }))
                            }),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_size(px(10.5)).text_color(if bypassed { t.text_3 } else { t.text }).child(name))
                    .child(
                        div()
                            .id(ElementId::Name(format!("fx-{mkey}-{i}-remove").into()))
                            .flex_none()
                            .opacity(0.)
                            .group_hover(SharedString::from(format!("fx-{mkey}-{i}")), |d| d.opacity(1.))
                            .text_color(t.text_2)
                            .hover(|d| d.text_color(t.danger))
                            .child(icon("x").size(px(10.)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click({
                                let slot = slot.clone();
                                move |_, _, cx| run_cmd(cx, "audio.removeEffect", json!({ "target": target_key(target), "slot": slot }))
                            }),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.drag = Some(MixDrag::Slot { target, from: i, to: i, len: n, y0: e.position.y, started: false });
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if n > shown {
            let st = strip.clone();
            rows.push(
                div()
                    .id(ElementId::Name(format!("fx-{mkey}-more").into()))
                    .flex_none()
                    .h(px(14.))
                    .text_size(px(10.))
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.text))
                    .child(format!("{} more…", n - shown))
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| this.effects_menu(st.target, &st.effects, e.position(), cx)))
                    .into_any_element(),
            );
        }
        if n < kimchi_core::audio::MAX_EFFECTS {
            rows.push(
                div()
                    .id(ElementId::Name(format!("fx-{mkey}-add").into()))
                    .flex_none()
                    .w_full()
                    .h(px(SLOT_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(3.))
                    .rounded(px(sz::R_XS))
                    .border_1()
                    .border_dashed()
                    .border_color(t.line_strong)
                    .text_size(px(10.5))
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.text).border_color(t.accent_ring))
                    .tooltip(|_, cx| tooltip(crate::actions::tip("Add an effect", &crate::actions::AddEffect), cx))
                    .child(icon("plus").size(px(10.)))
                    .child("Effect")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |e, _, cx| open_browser(target, e.position(), cx))
                    .into_any_element(),
            );
        }
        div().flex_none().w_full().flex().flex_col().gap(px(2.)).children(rows).into_any_element()
    }

    fn sends_el(&mut self, strip: &Rc<Strip>, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Target::Track(track) = strip.target else { return div().into_any_element() };
        let has_buses = self.store.read(cx).project.as_ref().is_some_and(|p| !p.mixer.buses.is_empty());
        let mut rows: Vec<AnyElement> = vec![];
        for (bus, name, level) in &strip.sends {
            let (bus, level) = (*bus, self.shown.get(&format!("send:{track}:{bus}")).copied().unwrap_or(*level));
            let pos = db_to_pos(level);
            let name: SharedString = name.clone().into();
            let tip_name = name.clone();
            rows.push(
                div()
                    .id(ElementId::Name(format!("send-{track}-{bus}").into()))
                    .flex_none()
                    .w_full()
                    .h(px(15.))
                    .relative()
                    .rounded(px(sz::R_XS))
                    .bg(t.bg_sunken)
                    .overflow_hidden()
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .tooltip(move |_, cx| tooltip(format!("Send to {tip_name}: {} dB · drag · right-click to remove", db_text(level)).into(), cx))
                    .child(div().absolute().left_0().top_0().bottom_0().w(relative(pos)).bg(t.accent_soft))
                    .child(div().absolute().inset_0().flex().items_center().px(px(4.)).text_size(px(9.5)).text_color(t.text_2).child(div().min_w_0().truncate().child(name)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let key = this.next_key("send");
                        this.drag = Some(MixDrag::Send { track, bus, x0: e.position.x, v0: pos, width: STRIP_W - 12., key });
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                        cx.stop_propagation();
                        run_cmd(cx, "audio.removeSend", json!({ "trackId": track, "busId": bus }));
                    })
                    .into_any_element(),
            );
        }
        if strip.sends.len() < kimchi_core::audio::MAX_SENDS {
            rows.push(
                div()
                    .id(ElementId::Name(format!("send-{track}-add").into()))
                    .flex_none()
                    .h(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .text_size(px(10.))
                    .text_color(t.text_3)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.text))
                    .tooltip(move |_, cx| tooltip(if has_buses { "Send part of this track to a bus" } else { "Make a reverb bus and send to it" }.into(), cx))
                    .child(icon("corner-down-right").size(px(10.)))
                    .child("Send")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| this.send_menu(track, e.position(), cx)))
                    .into_any_element(),
            );
        }
        div().flex_none().w_full().flex().flex_col().gap(px(2.)).children(rows).into_any_element()
    }

    fn fader_el(&mut self, strip: &Rc<Strip>, gain: f64, (readings, clipped): (Vec<Reading>, bool), ducking: f64, id_base: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let target = strip.target;
        let mkey = meter_key(target);
        let pos = db_to_pos(gain);
        let active = matches!(&self.drag, Some(MixDrag::Fader { target: dt, .. }) if *dt == target);
        let wide = strip.kind == StripKind::Master;
        let cap_h = 12.;
        let faders = self.faders.clone();
        let key = mkey.clone();
        let st = strip.clone();
        // Fewer marks when the fader is short.
        let travel_h = self.faders.borrow().get(&mkey).map(|b| f32::from(b.size.height)).unwrap_or(200.);
        let marks: &[f64] = if travel_h >= 150. { &widgets::SCALE } else if travel_h >= 80. { &[12.0, 0.0, -12.0, -48.0] } else { &[0.0, -24.0] };
        let scale = div().relative().w(px(20.)).h_full().children(marks.iter().map(|db| {
            let y = 1. - db_to_pos(*db);
            div()
                .absolute()
                .right(px(1.))
                .top(relative(y))
                .mt(px(-6.))
                .font_family(MONO)
                .text_size(px(8.5))
                .text_color(if *db == 0.0 { t.text_2 } else { t.text_3 })
                .child(scale_text(*db))
        }));
        let travel = div()
            .id(ElementId::Name(format!("{id_base}-fader").into()))
            .relative()
            .w(px(if wide { 26. } else { 22. }))
            .h_full()
            .cursor(gpui::CursorStyle::ResizeUpDown)
            .child(canvas(move |b, _, _| {
                faders.borrow_mut().insert(key.clone(), b);
            }, |_, _, _, _| {}).absolute().inset_0())
            // The groove, with the unity line.
            .child(div().absolute().top_0().bottom_0().left(relative(0.5)).ml(px(-2.)).w(px(4.)).bg(t.bg_sunken).border_1().border_color(t.line))
            .child(div().absolute().left(px(2.)).right(px(2.)).top(relative(1. - db_to_pos(0.0))).h(px(1.)).bg(t.line_strong))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(relative(1. - pos))
                    .mt(px(-cap_h / 2.))
                    .h(px(cap_h))
                    .bg(if active { t.accent } else { t.text_2 })
                    .border_1()
                    .border_color(if strip.gain_automated { t.accent } else { t.line_strong })
                    .shadow_sm()
                    .child(div().mx(px(3.)).mt(px(cap_h / 2. - 1.)).h(px(1.)).bg(t.bg_raised)),
            )
            .tooltip(move |_, cx| tooltip("Drag (Shift: finer) · double-click: 0 dB".into(), cx))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
                this.fader_down(&st, e, cx);
            }));
        let clip_key = mkey.clone();
        let meter = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .h_full()
            .w(px(if wide { 16. } else { 11. }))
            .child(
                div()
                    .id(ElementId::Name(format!("{id_base}-clip").into()))
                    .flex_none()
                    .h(px(4.))
                    .bg(widgets::clip_color(clipped, &t))
                    .cursor_pointer()
                    .tooltip(move |_, cx| tooltip(if clipped { "It went over 0 dB · click to clear" } else { "Clip light" }.into(), cx))
                    .on_click(move |_, _, cx| live::clear_clip(cx, &clip_key)),
            )
            .child(div().flex_1().relative().child(widgets::meter(readings, true, &t).absolute().inset_0()).when(ducking < -0.5, |d| {
                // How far ducking pulls the track down, from the top.
                let depth = ((-ducking / 24.).min(1.)) as f32;
                d.child(div().absolute().top_0().left_0().right_0().h(relative(depth)).border_b_1().border_color(t.accent).bg(t.accent_soft))
            }));
        div()
            .flex_1()
            .min_h(px(40.))
            .w_full()
            .flex()
            .justify_center()
            .gap(px(3.))
            .py(px(4.))
            .child(scale)
            .child(travel)
            .child(meter)
            .into_any_element()
    }

    /// The master's loudness readout, limiter and target.
    fn master_extras(&self, detail: Detail, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let (playhead, playing) = {
            let pb = self.playback.read(cx);
            (pb.playhead, pb.playing)
        };
        let snap = live::snapshot(cx, playhead, playing);
        if let Some(s) = &snap {
            live::note_loudness(cx, s);
        }
        let integrated = live::integrated(cx);
        let master = self.store.read(cx).project.as_ref().map(|p| p.mixer.master.clone()).unwrap_or_default();
        let row = |label: &'static str, value: String, tip: &'static str| {
            div()
                .id(label)
                .flex()
                .justify_between()
                .w_full()
                .font_family(MONO)
                .text_size(px(10.))
                .tooltip(move |_, cx| tooltip(tip.into(), cx))
                .child(div().text_color(t.text_3).child(label))
                .child(div().text_color(t.text).child(value))
        };
        let lufs = |v: Option<f64>| v.map(widgets::lufs_text).unwrap_or_else(|| "—".into());
        let target_label = match master.loudness {
            None => "Target".to_string(),
            Some(l) => format!("{l:.0} LUFS"),
        };
        let limiter = master.limiter;
        div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(3.))
            .p(px(5.))
            .rounded(px(sz::R_XS))
            .bg(t.bg_sunken)
            .when(detail != Detail::Small, |d| d
                .child(row("M", lufs(snap.as_ref().map(|s| s.momentary_lufs)), "Momentary loudness (400 ms), LUFS"))
                .child(row("S", lufs(snap.as_ref().map(|s| s.short_term_lufs)), "Short-term loudness (3 s), LUFS")))
            .child(row("I", lufs(integrated), "Integrated loudness of the last play, LUFS"))
            .when(detail != Detail::Small, |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(3.))
                        .mt(px(2.))
                        .child(
                            div()
                                .id("master-limiter")
                                .flex_1()
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(sz::R_XS))
                                .text_size(px(10.))
                                .border_1()
                                .border_color(if limiter { t.accent_ring } else { t.line })
                                .text_color(if limiter { t.accent_text } else { t.text_3 })
                                .cursor_pointer()
                                .tooltip(move |_, cx| tooltip(format!("True-peak limiter at {:.1} dBTP · click to switch {}", master.ceiling_db, if limiter { "off" } else { "on" }).into(), cx))
                                .child("Limit")
                                .on_click(move |_, _, cx| run_cmd(cx, "audio.setMaster", json!({ "limiter": !limiter }))),
                        )
                        .child(
                            div()
                                .id("master-target")
                                .flex_1()
                                .h(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(sz::R_XS))
                                .text_size(px(10.))
                                .border_1()
                                .border_color(t.line)
                                .text_color(if master.loudness.is_some() { t.text } else { t.text_3 })
                                .cursor_pointer()
                                .tooltip(|_, cx| tooltip("Loudness exports are brought to".into(), cx))
                                .child(div().truncate().child(target_label))
                                .on_click(|e, _, cx| loudness_menu(e.position(), cx)),
                        ),
                )
            })
            .into_any_element()
    }
}

/// The loudness targets, as a menu.
pub fn loudness_menu(position: Point<Pixels>, cx: &mut App) {
    let current = cx.store().read(cx).project.as_ref().and_then(|p| p.mixer.master.loudness);
    let item = |label: &'static str, value: Value, on: bool| {
        MenuItem::new(if on { format!("✓ {label}") } else { label.to_string() }, move |_, cx| run_cmd(cx, "audio.setMaster", json!({ "loudness": value }))).entry()
    };
    let entries = vec![
        item("As mixed", Value::Null, current.is_none()),
        MenuEntry::Separator,
        item("YouTube, streaming · −14 LUFS", json!(-14), current == Some(-14.0)),
        item("Podcast · −16 LUFS", json!(-16), current == Some(-16.0)),
        item("Broadcast (EBU R128) · −23 LUFS", json!(-23), current == Some(-23.0)),
    ];
    cx.store().update(cx, |s, cx| s.open_menu(position, entries, cx));
}

/// Runs a command as the window.
pub fn run_cmd(cx: &mut App, name: &str, params: Value) {
    cx.store().update(cx, |s, cx| s.run(name, params, cx));
}

/// Opens the effect browser to add to `target`'s chain.
pub fn open_browser(target: Target, position: Point<Pixels>, cx: &mut App) {
    cx.store().update(cx, |s, cx| s.set_audio(|a| a.browser = Some((target, position)), cx));
}

/// Measures loudness and says it.
pub fn measure(params: Value, cx: &mut App) {
    cx.store().update(cx, |s, cx| {
        s.flash("Measuring loudness…", cx);
        s.run_then("audio.measure", params, cx, |s, v, cx| match v["integrated"].as_f64() {
            Some(i) => {
                let peak = v["truePeak"].as_f64().map(|p| format!(", true peak {p:.1} dBTP")).unwrap_or_default();
                s.toast(kimchi_control::ToastKind::Info, format!("{}: {i:.1} LUFS{peak}, range {:.1} LU", v["measured"].as_str().or(v["clip"].as_str()).unwrap_or("Loudness"), v["range"].as_f64().unwrap_or(0.)), cx)
            }
            None => s.info("Silence: nothing to measure there.", cx),
        })
    });
}

/// A typed level: "-6", "-6 dB", "+3", "-inf".
pub fn parse_db(text: &str) -> Option<f64> {
    let t = text.trim().trim_end_matches("dB").trim_end_matches("db").trim();
    if t.eq_ignore_ascii_case("-inf") || t == "-∞" || t.eq_ignore_ascii_case("off") {
        return Some(MIN_DB);
    }
    let v: f64 = t.replace('−', "-").trim_start_matches('+').parse().ok()?;
    v.is_finite().then(|| v.clamp(MIN_DB, kimchi_core::audio::MAX_DB))
}

impl Render for MixerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let Some(p) = self.store.read(cx).project.clone() else { return div().into_any_element() };
        if self.drag.is_none() && self.settle.is_some_and(|k| k != project_key(&p)) {
            self.shown.clear();
            self.settle = None;
        }
        let (playhead, playing) = {
            let pb = self.playback.read(cx);
            (pb.playhead, pb.playing)
        };
        let snap = live::snapshot(cx, playhead, playing);
        let detail = self.detail();
        let strips = self.strips(&p, playhead);
        let empty = strips.is_empty();
        let mut els = vec![];
        for s in strips {
            let level = match s.target {
                Target::Track(id) => snap.as_ref().and_then(|x| x.tracks.get(&id)).copied(),
                Target::Bus(id) => snap.as_ref().and_then(|x| x.buses.get(&id)).copied(),
                _ => None,
            };
            let ducking = match s.target {
                Target::Track(id) => snap.as_ref().and_then(|x| x.ducking_db.get(&id)).copied().unwrap_or(0.),
                _ => 0.,
            };
            let readings = live::reading(cx, &meter_key(s.target), level.as_ref());
            els.push(self.strip_el(s, readings, ducking, detail, cx));
        }
        let master = self.master(&p, playhead);
        let readings = live::reading(cx, "master", snap.as_ref().map(|s| &s.master));
        let master_el = self.strip_el(master, readings, 0., detail, cx);
        let bounds = self.bounds.clone();
        let add_bus = div()
            .id("mixer-add-bus")
            .flex_none()
            .w(px(30.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(sz::R_SM))
            .border_1()
            .border_dashed()
            .border_color(t.line_strong)
            .text_color(t.text_3)
            .cursor_pointer()
            .hover(|d| d.text_color(t.text).border_color(t.accent_ring))
            .tooltip(|_, cx| tooltip("Add a bus (a shared reverb, a dialogue group)".into(), cx))
            .child(icon("plus"))
            .on_click(|_, _, cx| run_cmd(cx, "audio.addBus", json!({})));
        div()
            .id("mixer")
            .key_context("Mixer")
            .size_full()
            .flex()
            .bg(t.bg_raised)
            .child(
                div()
                    .id("mixer-strips")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .gap(px(4.))
                    .p(px(6.))
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .child({
                        // The strips give up detail with height: redraw once it is known.
                        let this = cx.entity().downgrade();
                        canvas(
                            move |b, _, cx| {
                                if bounds.get() != b {
                                    bounds.set(b);
                                    cx.defer(move |cx| {
                                        this.update(cx, |_, cx| cx.notify()).ok();
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0()
                    })
                    .when(empty, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(sz::SM))
                                .text_color(t.text_3)
                                .child("Tracks with sound get a strip here: import music, a voice or a video with sound."),
                        )
                    })
                    .children(els)
                    .child(add_bus),
            )
            .child(div().flex_none().h_full().p(px(6.)).border_l_1().border_color(t.line).child(master_el))
            .when(self.drag.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_up)))
            .into_any_element()
    }
}

// ---- the timeline toolbar's audio controls ----------------------------------------------------

/// The mixer toggle, its layout, the voice-over button and the master meter, for the timeline's
/// toolbar, in one group. `compact` narrows the meter; `labelled` names the buttons.
pub fn toolbar(compact: bool, labelled: bool, cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let store = cx.store();
    let (open, beside, recording) = {
        let a = &store.read(cx).audio;
        (a.mixer, a.beside, a.recording.clone())
    };
    let (playhead, playing) = {
        let pb = store.read(cx).playback.read(cx);
        (pb.playhead, pb.playing)
    };
    let snap = live::snapshot(cx, playhead, playing);
    let (readings, clipped) = live::reading(cx, "master", snap.as_ref().map(|s| &s.master));
    let meter = div()
        .id("master-meter")
        .flex_none()
        .w(px(if compact { 44. } else { 64. }))
        .h(px(12.))
        .mx(px(8.))
        .flex()
        .gap(px(2.))
        .tooltip(|_, cx| tooltip("Master level".into(), cx))
        .child(div().flex_1().relative().child(widgets::meter(readings, false, &t).absolute().inset_0()))
        .child(div().w(px(3.)).h_full().bg(widgets::clip_color(clipped, &t)));
    let rec_on = recording.is_some();
    let mut items = vec![
        meter.into_any_element(),
        crate::ui::tool("voice-over", "mic", "Record", labelled, crate::actions::tip(if rec_on { "Stop recording" } else { "Record a voice-over at the playhead" }, &crate::actions::RecordVoiceOver))
            .selected(rec_on)
            .color(if rec_on { t.danger } else { t.text_2 })
            .on_click(|_, w, cx| w.dispatch_action(Box::new(crate::actions::RecordVoiceOver), cx))
            .into_any_element(),
        crate::ui::tool("mixer-toggle", "sliders-vertical", "Mixer", labelled, crate::actions::tip(if open { "Hide the mixer" } else { "Mixer" }, &crate::actions::ToggleMixer))
            .selected(open)
            .on_click(|_, w, cx| w.dispatch_action(Box::new(crate::actions::ToggleMixer), cx))
            .into_any_element(),
    ];
    if open {
        items.push(
            Button::icon("mixer-layout", if beside { "columns-2" } else { "panel-bottom" }, if beside { "Mixer beside the timeline: put it in its place" } else { "Mixer in the timeline's place: show both side by side" })
                .small()
                .flush()
                .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.set_audio(|a| a.beside = !beside, cx)))
                .into_any_element(),
        );
    }
    crate::ui::group(items, cx).into_any_element()
}

/// The timeline's area: the tracks, the mixer in their place, or both side by side.
pub fn area(body: AnyElement, mixer: &Entity<MixerView>, cx: &App) -> AnyElement {
    let t = cx.theme();
    let a = &cx.store().read(cx).audio;
    match (a.mixer, a.beside) {
        (false, _) => body,
        (true, false) => div().size_full().child(mixer.clone()).into_any_element(),
        (true, true) => div()
            .size_full()
            .flex()
            .child(div().flex_1().min_w(px(240.)).h_full().child(body))
            .child(div().flex_none().w(relative(0.48)).max_w(px(760.)).h_full().border_l_1().border_color(t.line).child(mixer.clone()))
            .into_any_element(),
    }
}

/// Workspace actions: the mixer, the selected track's mute / solo / arm, adding an effect.
pub fn toggle_mixer(cx: &mut App) {
    if cx.store().read(cx).project.is_none() {
        return;
    }
    cx.store().update(cx, |s, cx| s.set_audio(|a| a.mixer = !a.mixer, cx));
}

/// The track an action means: the mixer's selected strip, else the selected clips' track.
pub fn selected_track(s: &Store) -> Option<Id> {
    if let Some(Target::Track(id)) = s.audio.strip.filter(|_| s.selection.is_empty()) {
        return Some(id);
    }
    s.selection.first().and_then(|c| s.track_of(*c)).map(|t| t.id)
}

pub fn toggle_track(field: &'static str, cx: &mut App) {
    let store = cx.store();
    let s = store.read(cx);
    let Some(id) = selected_track(s) else {
        store.update(cx, |s, cx| s.flash("Select a clip (or a mixer strip) to pick its track.", cx));
        return;
    };
    let Some(t) = s.project.as_ref().and_then(|p| p.track(id)) else { return };
    let (on, name) = match field {
        "muted" => (!t.muted, t.name.clone()),
        "solo" => (!t.mix.solo, t.name.clone()),
        _ => (!t.mix.armed, t.name.clone()),
    };
    if field == "armed" && t.kind != TrackKind::Audio {
        store.update(cx, |s, cx| s.flash("Voice-overs are recorded onto audio tracks.", cx));
        return;
    }
    let what = match (field, on) {
        ("muted", true) => "muted",
        ("muted", false) => "unmuted",
        ("solo", true) => "soloed",
        ("solo", false) => "no longer soloed",
        (_, true) => "armed",
        _ => "disarmed",
    };
    store.update(cx, |s, cx| {
        s.run("audio.setTrack", json!({ "trackId": id, field: on }), cx);
        s.flash(format!("{name} {what}"), cx);
    });
}

/// Adds an effect: to the selected clip with sound, else the selected track, else the master.
pub fn add_effect(window: &Window, cx: &mut App) {
    let s = cx.store().read(cx);
    let target = match s.selection.as_slice() {
        [one] if s.project.as_ref().is_some_and(|p| p.clip(*one).is_some_and(|c| kimchi_control::commands::audio::has_sound(p, c))) => Target::Clip(*one),
        _ => match selected_track(s) {
            Some(id) => Target::Track(id),
            None => s.audio.strip.unwrap_or(Target::Master),
        },
    };
    let pos = window.mouse_position();
    open_browser(target, pos, cx);
}

/// The sound actions (mixer, mute / solo / arm, voice-over, add effect), on the workspace.
pub fn on_actions(el: gpui::Div) -> gpui::Div {
    use crate::actions::{AddEffect, ArmTrack, MuteTrack, RecordVoiceOver, SoloTrack, ToggleMixer};
    el.on_action(|_: &ToggleMixer, _, cx| toggle_mixer(cx))
        .on_action(|_: &MuteTrack, _, cx| toggle_track("muted", cx))
        .on_action(|_: &SoloTrack, _, cx| toggle_track("solo", cx))
        .on_action(|_: &ArmTrack, _, cx| toggle_track("armed", cx))
        .on_action(|_: &RecordVoiceOver, _, cx| record_toggle(cx))
        .on_action(|_: &AddEffect, window, cx| add_effect(window, cx))
}

/// Starts a voice-over take at the playhead, or stops the one recording.
pub fn record_toggle(cx: &mut App) {
    let store = cx.store();
    if store.read(cx).project.is_none() {
        return;
    }
    let action = if store.read(cx).audio.recording.is_some() { "stop" } else { "start" };
    store.update(cx, |s, cx| s.run("audio.record", json!({ "action": action }), cx));
}

/// ryolune's logo, where a song or a hand-off is shown.
pub fn ryolune_mark(size: f32, _cx: &App) -> AnyElement {
    crate::ui::logo("ryolune", px(size)).into_any_element()
}

/// The export dialog's loudness choice: the master's target (`audio.setMaster loudness`).
pub fn export_loudness(cx: &App) -> AnyElement {
    let current = cx.store().read(cx).project.as_ref().and_then(|p| p.mixer.master.loudness).map(|l| l.round() as i64);
    crate::ui::segmented(
        "export-loudness",
        vec![(None, "As mixed".into()), (Some(-14), "−14 YouTube".into()), (Some(-16), "−16 Podcast".into()), (Some(-23), "−23 TV".into())],
        current,
        |v, _, cx| run_cmd(cx, "audio.setMaster", json!({ "loudness": v })),
        cx,
    )
    .into_any_element()
}

/// The sound part of a clip's context menu: its effects, loudness, beats, and for a ryolune
/// song, opening it there.
pub fn clip_menu_items(p: &Project, clip: &kimchi_core::Clip, locked: bool) -> Vec<MenuEntry> {
    if !kimchi_control::commands::audio::has_sound(p, clip) {
        return vec![];
    }
    let id = clip.id;
    let asset = clip.asset_id().and_then(|a| p.asset(a));
    let mut items = vec![
        MenuEntry::Separator,
        MenuItem::new("Add an effect…", move |w, cx| open_browser(Target::Clip(id), w.mouse_position(), cx)).icon("sliders-horizontal").shortcut_of(&crate::actions::AddEffect).disabled(locked).entry(),
        MenuItem::new("Normalize loudness", move |_, cx| {
            cx.store().update(cx, |s, cx| {
                s.run_then("audio.normalize", json!({ "clipIds": [id] }), cx, |s, v, cx| match v["clips"][0]["gainDb"].as_f64() {
                    Some(g) => s.flash(format!("Normalized: {} dB", widgets::db_text(g)), cx),
                    None => s.info(v["clips"][0]["note"].as_str().unwrap_or("Nothing to normalize.").to_string(), cx),
                })
            })
        })
        .icon("audio-waveform")
        .disabled(locked)
        .entry(),
        MenuItem::new("Measure loudness", move |_, cx| measure(json!({ "clipId": id }), cx)).icon("gauge").entry(),
    ];
    match asset.and_then(|a| a.beats.as_ref()) {
        Some(_) => items.push(
            MenuItem::new("Cut the picture on the bars", move |_, cx| {
                cx.store().update(cx, |s, cx| s.run_then("audio.beatCut", json!({ "musicClipId": id }), cx, |s, v, cx| s.flash(format!("{} cuts on the beat", v["cuts"].as_u64().unwrap_or(0)), cx)))
            })
            .icon("scissors")
            .entry(),
        ),
        None => items.push(
            MenuItem::new("Detect beats", move |_, cx| {
                cx.store().update(cx, |s, cx| {
                    s.flash("Listening for the beat…", cx);
                    s.run_then("audio.detectBeats", json!({ "clipId": id }), cx, |s, v, cx| s.flash(format!("{:.0} BPM", v["tempo"].as_f64().unwrap_or(0.)), cx))
                })
            })
            .icon("music")
            .entry(),
        ),
    }
    if asset.is_some_and(|a| matches!(a.origin, kimchi_core::AssetOrigin::Song(_))) {
        items.push(MenuItem::new("Open in ryolune", move |_, cx| run_cmd(cx, "audio.openInRyolune", json!({ "clipId": id }))).logo("ryolune").entry());
        items.push(MenuItem::new("Render the song again", move |_, cx| run_cmd(cx, "audio.refreshSongs", json!({ "force": true }))).icon("refresh-cw").entry());
    }
    items
}
