//! The inspector's sound: a clip's Audio section and, when a mixer strip is picked with no
//! clip selected, that track's (bus's, master's) mix.
//!
//! - Clip: gain in dB and pan (each with a keyframe toggle at the playhead; an animated one
//!   gets a keyframe instead of a new value), the fades' curve, which channels play, pitch and
//!   whether speed keeps it, muting the clip's own sound, the clip's effects, a loudness
//!   readout with Normalize, Detect beats (tempo shown), and for ryolune songs: the song,
//!   whether it changed, "Open in ryolune" and "Refresh".
//! - Strip: fader, pan, mute / solo, where it goes, ducking, sends and effects.
//!
//! Every change is an `audio.*` or `clip.*` command; drags share a coalesce key.

use gpui::{AnyElement, App, Context, Entity, FontWeight, SharedString, Subscription, div, prelude::*, px};
use kimchi_control::commands::audio::{Target, channels_name, song_state};
use kimchi_core::audio::{Channels, FadeCurve, MIN_DB, db_to_gain, gain_to_db};
use kimchi_core::{AssetOrigin, Clip, Id, Project};
use serde_json::{Value, json};

use super::slider::Slider;
use super::{Inspector, section};
use crate::store::StoreExt;
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::{Button, caps, icon, segmented, switch, tooltip};
use crate::views::mixer::{open_browser, run_cmd, target_key, widgets};

/// Lowest gain the clip slider reaches (below it, mute the clip).
const GAIN_FLOOR: f64 = -36.0;

/// The retained controls of the sound sections.
pub struct AudioFields {
    gain: Entity<Slider>,
    pan: Entity<Slider>,
    pitch: Entity<Scrub>,
    strip_gain: Entity<Slider>,
    strip_pan: Entity<Slider>,
    duck_amount: Entity<Scrub>,
    /// The last loudness reading of a clip: (clip, text).
    measured: Option<(Id, String)>,
}

impl AudioFields {
    pub fn new(subs: &mut Vec<Subscription>, cx: &mut Context<Inspector>) -> Self {
        let gain = cx.new(|cx| Slider::new(GAIN_FLOOR, 12.0, cx).neutral(0.0));
        subs.push(cx.subscribe(&gain, |this: &mut Inspector, _, ch: &ScrubChange, cx| {
            let v = if ch.value <= GAIN_FLOOR + 0.01 { 0.0 } else { db_to_gain(ch.value).min(4.0) };
            this.update_clip(json!({ "volume": v }), "volume", ch.final_, cx);
        }));
        let pan = cx.new(|cx| Slider::new(-1.0, 1.0, cx).neutral(0.0));
        subs.push(cx.subscribe(&pan, |this: &mut Inspector, _, ch: &ScrubChange, cx| this.set_clip_pan(ch.value, ch.final_, cx)));
        let pitch = cx.new(|_| Scrub::new("Pitch", 0.1, 1).unit(" st").range(-24.0, 24.0));
        subs.push(cx.subscribe(&pitch, |this: &mut Inspector, _, ch: &ScrubChange, cx| this.set_sound(json!({ "pitch": ch.value }), "pitch", ch.final_, cx)));
        let strip_gain = cx.new(|cx| Slider::new(-48.0, 12.0, cx).neutral(0.0));
        subs.push(cx.subscribe(&strip_gain, |this: &mut Inspector, _, ch: &ScrubChange, cx| this.set_strip(json!({ "gainDb": if ch.value <= -47.99 { MIN_DB } else { ch.value } }), "gain", ch.final_, cx)));
        let strip_pan = cx.new(|cx| Slider::new(-1.0, 1.0, cx).neutral(0.0));
        subs.push(cx.subscribe(&strip_pan, |this: &mut Inspector, _, ch: &ScrubChange, cx| this.set_strip(json!({ "pan": ch.value }), "pan", ch.final_, cx)));
        let duck_amount = cx.new(|_| Scrub::new("Down by", 0.5, 1).unit(" dB").range(-48.0, 0.0));
        subs.push(cx.subscribe(&duck_amount, |this: &mut Inspector, _, ch: &ScrubChange, cx| this.set_strip(json!({ "duck": { "amountDb": ch.value } }), "duck", ch.final_, cx)));
        Self { gain, pan, pitch, strip_gain, strip_pan, duck_amount, measured: None }
    }
}

/// "+3.0 dB", "−∞".
fn db_label(db: f64) -> String {
    format!("{} dB", widgets::db_text(db))
}

impl Inspector {
    /// `audio.setClip` on the selected clip.
    fn set_sound(&mut self, mut params: Value, key: &str, final_: bool, cx: &mut Context<Self>) {
        let Some(id) = self.single(cx) else { return };
        params["clipIds"] = json!([id]);
        if !final_ {
            params["coalesce"] = json!(format!("{id}:{key}"));
        }
        self.store.update(cx, |s, cx| s.run("audio.setClip", params, cx));
    }

    /// The clip's pan, or a pan keyframe at the playhead when it is animated.
    fn set_clip_pan(&mut self, v: f64, final_: bool, cx: &mut Context<Self>) {
        let Some(id) = self.single(cx) else { return };
        let animated = self.store.read(cx).clip(id).is_some_and(|c| c.keyframes.contains_key("pan"));
        if animated {
            let playhead = self.store.read(cx).playback.read(cx).playhead;
            let mut p = json!({ "clipId": id, "property": "pan", "time": playhead, "value": v });
            if !final_ {
                p["coalesce"] = json!(format!("{id}:pan:key"));
            }
            self.store.update(cx, |s, cx| s.run("clip.addKeyframe", p, cx));
        } else {
            self.set_sound(json!({ "pan": v }), "pan", final_, cx);
        }
    }

    /// The strip the inspector shows (the mixer's pick, with no clip selected).
    fn strip_target(&self, cx: &App) -> Option<Target> {
        let s = self.store.read(cx);
        s.audio.strip.filter(|_| s.selection.is_empty() && s.selected_asset.is_none())
    }

    /// A change to the picked strip: `audio.setTrack`, `setBus` or `setMaster`.
    fn set_strip(&mut self, mut params: Value, key: &str, final_: bool, cx: &mut Context<Self>) {
        let Some(t) = self.strip_target(cx) else { return };
        let p = self.store.read(cx).project.clone();
        // An automated fader or pan gets a keyframe at the playhead.
        let automated = p.as_ref().and_then(|p| t.keyframes(p)).is_some_and(|k| (key == "gain" && k.contains_key("gainDb")) || (key == "pan" && k.contains_key("pan")));
        if automated {
            let (property, value) = if key == "gain" { ("gainDb", params["gainDb"].clone()) } else { ("pan", params["pan"].clone()) };
            let mut p = json!({ "target": target_key(t), "property": property, "value": value });
            if !final_ {
                p["coalesce"] = json!(format!("{}:{key}:key", target_key(t)));
            }
            self.store.update(cx, |s, cx| s.run("audio.addAutomationKey", p, cx));
            return;
        }
        let name = match t {
            Target::Track(id) => {
                params["trackId"] = json!(id);
                "audio.setTrack"
            }
            Target::Bus(id) => {
                if key == "duck" {
                    return;
                }
                params["busId"] = json!(id);
                "audio.setBus"
            }
            _ => "audio.setMaster",
        };
        if !final_ {
            params["coalesce"] = json!(format!("{}:{key}", target_key(t)));
        }
        self.store.update(cx, |s, cx| s.run(name, params, cx));
    }

    /// Fills the sound controls from the clip at the playhead.
    pub(super) fn sync_audio(&mut self, clip: &Clip, playhead: f64, cx: &mut Context<Self>) {
        let gain = gain_to_db(clip.volume_at(playhead)).max(GAIN_FLOOR);
        self.audio.gain.update(cx, |s, _| s.set_value(gain));
        let pan = clip.pan_at(playhead);
        self.audio.pan.update(cx, |s, _| s.set_value(pan));
        let pitch = clip.audio.pitch;
        self.audio.pitch.update(cx, |s, _| s.set_value(pitch));
    }

    /// A diamond that adds (or, on a keyframe, removes) a clip keyframe of `prop` at the playhead.
    fn key_toggle(&self, clip: &Clip, prop: &'static str, cx: &App) -> AnyElement {
        let t = cx.theme();
        let playhead = self.store.read(cx).playback.read(cx).playhead;
        let fps = self.store.read(cx).fps();
        let local = playhead - clip.start;
        let keys = clip.keyframes.get(prop);
        let here = keys.is_some_and(|k| k.iter().any(|k| (k.time - local).abs() <= 0.5 / fps));
        let animated = keys.is_some();
        let inside = playhead >= clip.start - 1e-6 && playhead <= clip.end() + 1e-6;
        let id = clip.id;
        div()
            .id(SharedString::from(format!("audio-key-{prop}")))
            .flex_none()
            .text_color(if here || animated { t.accent_text } else { t.text_3 })
            .when(!inside, |d| d.opacity(0.4))
            .when(inside, |d| d.cursor_pointer().hover(|d| d.text_color(t.accent_text)))
            .tooltip(move |_, cx| {
                tooltip(
                    match (here, animated) {
                        (true, _) => "Remove the keyframe here",
                        (false, true) => "Add a keyframe at the playhead",
                        _ => "Animate it: a first keyframe at the playhead",
                    }
                    .into(),
                    cx,
                )
            })
            .child(icon("diamond").size(px(11.)))
            .when(inside, |d| {
                d.on_click(move |_, _, cx| {
                    let playhead = cx.store().read(cx).playback.read(cx).playhead;
                    let name = if here { "clip.removeKeyframe" } else { "clip.addKeyframe" };
                    run_cmd(cx, name, json!({ "clipId": id, "property": prop, "time": playhead }));
                })
            })
            .into_any_element()
    }

    /// The Audio section of a clip with sound.
    pub(super) fn audio_section(&mut self, clip: &Clip, project: &Project, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let id = clip.id;
        let playhead = self.store.read(cx).playback.read(cx).playhead;
        self.sync_audio(clip, playhead, cx);
        let a = &clip.audio;
        let gain = gain_to_db(clip.volume_at(playhead));
        let pan = clip.pan_at(playhead);
        let row = |name: &'static str, control: AnyElement, value: String, key: AnyElement, cx: &App| {
            let t = cx.theme();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().w(px(44.)).flex_none().text_size(px(sz::SM)).text_color(t.text_2).child(name))
                .child(div().flex_1().min_w_0().flex().child(control))
                .child(div().w(px(52.)).flex_none().flex().justify_end().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(value))
                .child(key)
        };
        let curve = a.fade_curve;
        let curves = segmented(
            "fade-curve",
            vec![(FadeCurve::Linear, "Linear".into()), (FadeCurve::EqualPower, "Equal".into()), (FadeCurve::Exponential, "Exp".into()), (FadeCurve::SCurve, "S".into())],
            curve,
            move |c, _, cx| run_cmd(cx, "audio.setClip", json!({ "clipIds": [id], "fadeCurve": c.name() })),
            cx,
        );
        let channels = a.channels;
        let chans = segmented(
            "channels",
            vec![(Channels::Stereo, "Stereo".into()), (Channels::Mono, "Mono".into()), (Channels::Left, "L".into()), (Channels::Right, "R".into()), (Channels::Swap, "Swap".into())],
            channels,
            move |c, _, cx| run_cmd(cx, "audio.setClip", json!({ "clipIds": [id], "channels": channels_name(*c) })),
            cx,
        );
        let (muted, preserve) = (a.muted, a.preserve_pitch);
        let effects = self.chain_rows(Target::Clip(id), &a.effects, cx);
        let measured = self.audio.measured.as_ref().filter(|(c, _)| *c == id).map(|(_, s)| s.clone());
        let target_lufs = self.store.read(cx).settings.audio.default_loudness;
        let asset = clip.asset_id().and_then(|x| project.asset(x)).cloned();
        let beats = asset.as_ref().and_then(|x| x.beats.clone());
        let this = cx.entity();
        let (e1, e2) = (this.clone(), this.clone());
        let mut s = section(cx)
            .child(div().flex().items_center().justify_between().child(caps("Audio", cx)).child(
                Button::icon("audio-mixer", "sliders-vertical", crate::actions::tip("Mixer", &crate::actions::ToggleMixer))
                    .small()
                    .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.set_audio(|a| a.mixer = true, cx))),
            ))
            .child(row("Gain", self.audio.gain.clone().into_any_element(), db_label(gain), self.key_toggle(clip, "volume", cx), cx))
            .child(row("Pan", self.audio.pan.clone().into_any_element(), widgets::pan_text(pan), self.key_toggle(clip, "pan", cx), cx))
            .child(div().flex().flex_col().gap(px(4.)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Fade shape")).child(curves))
            .child(div().flex().flex_col().gap(px(4.)).child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Channels")).child(chans))
            .child(self.audio.pitch.clone())
            .child(switch("preserve-pitch", "Speed keeps the pitch", preserve, move |v, _, cx| run_cmd(cx, "audio.setClip", json!({ "clipIds": [id], "preservePitch": v })), cx))
            .child(switch("mute-sound", "Mute this clip's sound", muted, move |v, _, cx| run_cmd(cx, "audio.setClip", json!({ "clipIds": [id], "muted": v })), cx))
            .child(div().flex().flex_col().gap(px(4.)).child(div().flex().items_center().justify_between().child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Effects")).child(
                Button::new("clip-add-effect", "Add").small().ghost().with_icon("plus").tooltip(crate::actions::tip("Add an effect to this clip", &crate::actions::AddEffect)).on_click(move |e, _, cx| open_browser(Target::Clip(id), e.position(), cx)),
            )).child(effects))
            // Loudness.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO).text_size(px(sz::XS)).text_color(if measured.is_some() { t.text } else { t.text_3 }).child(measured.unwrap_or_else(|| "Loudness not measured".into())))
                    .child(Button::new("clip-measure", "Measure").small().with_icon("gauge").on_click(move |_, _, cx| {
                        let e = e1.clone();
                        cx.store().update(cx, |s, cx| {
                            s.run_then("audio.measure", json!({ "clipId": id }), cx, move |_, v, cx| {
                                let text = match v["integrated"].as_f64() {
                                    Some(i) => format!("{i:.1} LUFS · peak {}", v["truePeak"].as_f64().map(|p| format!("{p:.1} dBTP")).unwrap_or_else(|| "—".into())),
                                    None => "Silent".into(),
                                };
                                e.update(cx, |this, cx| {
                                    this.audio.measured = Some((id, text));
                                    cx.notify();
                                });
                            })
                        })
                    })),
            )
            .child(
                Button::new("clip-normalize", format!("Normalize to {target_lufs:.0} LUFS"))
                    .small()
                    .full_width()
                    .with_icon("audio-waveform")
                    .tooltip("Sets the volume so the clip is as loud as the target (Settings › Audio)")
                    .on_click(move |_, _, cx| {
                        let e = e2.clone();
                        cx.store().update(cx, |s, cx| {
                            s.run_then("audio.normalize", json!({ "clipIds": [id] }), cx, move |s, v, cx| {
                                let c = &v["clips"][0];
                                match c["gainDb"].as_f64() {
                                    Some(g) => s.flash(format!("Normalized: {} dB", widgets::db_text(g)), cx),
                                    None => s.info(c["note"].as_str().unwrap_or("Nothing to normalize.").to_string(), cx),
                                }
                                e.update(cx, |this, cx| {
                                    this.audio.measured = None;
                                    cx.notify();
                                });
                            })
                        })
                    }),
            );
        // Beats.
        s = s.child(match &beats {
            Some(b) => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(sz::SM))
                .text_color(t.text_2)
                .child(icon("music").text_color(t.accent_text))
                .child(format!("{:.0} BPM · {}/4 · {} beats{}", b.tempo, b.beats_per_bar, b.times.len(), if b.source == "ryolune" { " (the song's)" } else { "" }))
                .into_any_element(),
            None => Button::new("detect-beats", "Detect beats")
                .small()
                .full_width()
                .with_icon("music")
                .tooltip("Find the tempo and beats: the timeline shows and snaps to them, Beat cut follows them")
                .on_click(move |_, _, cx| {
                    cx.store().update(cx, |s, cx| {
                        s.flash("Listening for the beat…", cx);
                        s.run_then("audio.detectBeats", json!({ "clipId": id }), cx, |s, v, cx| s.flash(format!("{:.0} BPM, {} beats", v["tempo"].as_f64().unwrap_or(0.), v["beats"].as_u64().unwrap_or(0)), cx));
                    })
                })
                .into_any_element(),
        });
        // A ryolune song.
        if let Some(AssetOrigin::Song(r)) = asset.as_ref().map(|x| &x.origin) {
            let state = song_state(r);
            let asset_id = asset.as_ref().map(|x| x.id);
            s = s.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(8.))
                    .rounded(px(sz::R_SM))
                    .bg(t.bg_sunken)
                    .border_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(crate::views::mixer::ryolune_mark(14., cx))
                            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).text_size(px(sz::SM)).child(r.title.clone())),
                    )
                    .child(div().text_size(px(sz::XS)).text_color(if state == "current" { t.text_3 } else { t.warning }).child(match state {
                        "changed" => "Saved in ryolune since: refresh to hear the new version.".to_string(),
                        "missing" => format!("{} is gone; the last render plays.", r.song),
                        _ => format!("{:.0} BPM · up to date", r.tempo),
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(div().flex_1().child(Button::new("open-ryolune", "Open in ryolune").small().full_width().with_icon("external-link").on_click(move |_, _, cx| run_cmd(cx, "audio.openInRyolune", json!({ "clipId": id })))))
                            .child(Button::icon("refresh-song", "refresh-cw", "Render the song again").small().on_click(move |_, _, cx| run_cmd(cx, "audio.refreshSongs", json!({ "assetIds": asset_id.into_iter().collect::<Vec<_>>(), "force": true })))),
                    ),
            );
        }
        s.into_any_element()
    }

    /// Rows for an effect chain: power, name (opens its panel), remove.
    fn chain_rows(&self, target: Target, chain: &[kimchi_core::Insert], cx: &App) -> AnyElement {
        let t = cx.theme();
        if chain.is_empty() {
            return div().text_size(px(sz::XS)).text_color(t.text_3).child("No effects.").into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .children(chain.iter().enumerate().map(|(i, e)| {
                let (slot, slot2, slot3) = (e.id.clone(), e.id.clone(), e.id.clone());
                let bypassed = e.is_bypassed();
                div()
                    .id(SharedString::from(format!("chain-{}-{i}", target_key(target))))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(26.))
                    .px(px(6.))
                    .rounded(px(sz::R_SM))
                    .bg(t.bg_sunken)
                    .border_1()
                    .border_color(t.line)
                    .cursor_pointer()
                    .hover(|d| d.border_color(t.accent_ring))
                    .child(
                        div()
                            .id(SharedString::from(format!("chain-power-{i}")))
                            .size(px(10.))
                            .border_1()
                            .border_color(if bypassed { t.text_3 } else { t.accent })
                            .bg(if bypassed { gpui::transparent_black() } else { t.accent })
                            .tooltip(move |_, cx| tooltip(if bypassed { "Switch on" } else { "Bypass" }.into(), cx))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                run_cmd(cx, "audio.setEffect", json!({ "target": target_key(target), "slot": slot, "bypassed": !bypassed }))
                            }),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_size(px(sz::SM)).text_color(if bypassed { t.text_3 } else { t.text }).child(e.name.clone()))
                    .child(
                        div()
                            .id(SharedString::from(format!("chain-remove-{i}")))
                            .text_color(t.text_3)
                            .hover(|d| d.text_color(t.danger))
                            .tooltip(|_, cx| tooltip("Remove".into(), cx))
                            .child(icon("x").size(px(11.)))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                run_cmd(cx, "audio.removeEffect", json!({ "target": target_key(target), "slot": slot2 }))
                            }),
                    )
                    .on_click(move |_, _, cx| {
                        let slot = slot3.clone();
                        cx.store().update(cx, |s, cx| s.set_audio(|a| a.effect = Some((target, slot)), cx))
                    })
            }))
            .into_any_element()
    }

    /// The picked mixer strip's mix, when no clip is selected (`None`: nothing picked).
    pub(super) fn strip_view(&mut self, p: &Project, cx: &mut Context<Self>) -> Option<Vec<AnyElement>> {
        let target = self.strip_target(cx)?;
        let t = cx.theme().clone();
        let playhead = self.store.read(cx).playback.read(cx).playhead;
        let (name, kind, gain, pan, chain) = match target {
            Target::Track(id) => {
                let tr = p.track(id)?;
                (tr.name.clone(), "track", tr.mix.gain_db_at(playhead), Some(tr.mix.pan_at(playhead)), tr.mix.effects.clone())
            }
            Target::Bus(id) => {
                let b = p.mixer.bus(id)?;
                (b.name.clone(), "bus", b.mix.gain_db_at(playhead), Some(b.mix.pan_at(playhead)), b.mix.effects.clone())
            }
            Target::Master => ("Master".into(), "master", kimchi_core::anim::number_at(&p.mixer.master.keyframes, "gainDb", playhead).unwrap_or(p.mixer.master.gain_db), None, p.mixer.master.effects.clone()),
            Target::Clip(_) => return None,
        };
        self.shown = None;
        self.audio.strip_gain.update(cx, |s, _| s.set_value(gain.max(-48.0)));
        if let Some(v) = pan {
            self.audio.strip_pan.update(cx, |s, _| s.set_value(v));
        }
        let mut body: Vec<AnyElement> = vec![];
        let labeled = |name: &'static str, control: AnyElement, value: String, cx: &App| {
            let t = cx.theme();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().w(px(44.)).flex_none().text_size(px(sz::SM)).text_color(t.text_2).child(name))
                .child(div().flex_1().min_w_0().flex().child(control))
                .child(div().w(px(56.)).flex_none().flex().justify_end().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(value))
        };
        let mut mix = section(cx)
            .child(caps("Mix", cx))
            .child(labeled("Fader", self.audio.strip_gain.clone().into_any_element(), db_label(gain), cx))
            .when_some(pan, |d, v| d.child(labeled("Pan", self.audio.strip_pan.clone().into_any_element(), widgets::pan_text(v), cx)));
        if let Target::Track(id) = target {
            let tr = p.track(id)?;
            let (muted, solo) = (tr.muted, tr.mix.solo);
            let output = tr.mix.output.and_then(|b| p.mixer.bus(b)).map(|b| b.name.clone()).unwrap_or_else(|| "Master".into());
            mix = mix
                .child(switch("strip-mute", "Mute", muted, move |v, _, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": id, "muted": v })), cx))
                .child(switch("strip-solo", "Solo", solo, move |v, _, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": id, "solo": v })), cx))
                .child(
                    div().flex().items_center().justify_between().child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Goes to")).child(
                        Button::new("strip-output", output).small().icon_after("chevron-down").on_click(move |e, _, cx| {
                            let Some(p) = cx.store().read(cx).project.clone() else { return };
                            let mut entries = vec![crate::store::MenuItem::new("Master", move |_, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": id, "output": "master" }))).entry()];
                            for b in &p.mixer.buses {
                                let bid = b.id;
                                entries.push(crate::store::MenuItem::new(b.name.clone(), move |_, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": id, "output": bid }))).entry());
                            }
                            let pos = e.position();
                            cx.store().update(cx, |s, cx| s.open_menu(pos, entries, cx));
                        }),
                    ),
                );
            body.push(mix.into_any_element());
            // Ducking.
            let duck = tr.mix.duck.clone();
            if let Some(d) = &duck {
                self.audio.duck_amount.update(cx, |s, _| s.set_value(d.amount_db));
            }
            let under: Vec<String> = duck.as_ref().map(|d| d.under.iter().filter_map(|u| p.track(*u).map(|t| t.name.clone())).collect()).unwrap_or_default();
            body.push(
                section(cx)
                    .child(caps("Ducking", cx))
                    .child(switch("strip-duck", "Duck under other tracks", duck.is_some(), move |v, _, cx| run_cmd(cx, "audio.setTrack", json!({ "trackId": id, "duck": v })), cx))
                    .when(duck.is_some(), |d| {
                        d.child(self.audio.duck_amount.clone()).child(div().text_size(px(sz::XS)).text_color(t.text_2).child(if under.is_empty() { "Under every other track with sound.".to_string() } else { format!("Under {}.", under.join(", ")) }))
                    })
                    .child(
                        Button::new("auto-duck", "Duck the music under the dialogue")
                            .small()
                            .full_width()
                            .with_icon("arrow-down")
                            .tooltip("One click: kimchi finds the music and dialogue tracks")
                            .on_click(|_, _, cx| {
                                cx.store().update(cx, |s, cx| s.run_then("audio.autoDuck", json!({}), cx, |s, v, cx| s.flash(format!("Ducked {} under {}", list(&v["music"]), list(&v["dialogue"])), cx)))
                            }),
                    )
                    .into_any_element(),
            );
            // Sends.
            if !tr.mix.sends.is_empty() {
                body.push(
                    section(cx)
                        .child(caps("Sends", cx))
                        .children(tr.mix.sends.iter().map(|s| {
                            let name = p.mixer.bus(s.bus).map(|b| b.name.clone()).unwrap_or_default();
                            div().flex().justify_between().text_size(px(sz::SM)).child(div().text_color(t.text_2).child(format!("→ {name}"))).child(div().font_family(MONO).text_size(px(sz::XS)).child(format!("{} dB{}", widgets::db_text(s.level_db), if s.pre_fader { " · pre" } else { "" })))
                        }))
                        .into_any_element(),
                );
            }
        } else {
            if let Target::Bus(id) = target {
                let b = p.mixer.bus(id)?;
                let (muted, solo) = (b.muted, b.mix.solo);
                mix = mix
                    .child(switch("bus-mute", "Mute", muted, move |v, _, cx| run_cmd(cx, "audio.setBus", json!({ "busId": id, "muted": v })), cx))
                    .child(switch("bus-solo", "Solo", solo, move |v, _, cx| run_cmd(cx, "audio.setBus", json!({ "busId": id, "solo": v })), cx));
            } else {
                let m = &p.mixer.master;
                let limiter = m.limiter;
                let target_label = m.loudness.map(|l| format!("{l:.0} LUFS")).unwrap_or_else(|| "As mixed".into());
                mix = mix.child(switch("master-limiter-switch", format!("Limiter at {:.1} dBTP", m.ceiling_db), limiter, |v, _, cx| run_cmd(cx, "audio.setMaster", json!({ "limiter": v })), cx)).child(
                    div().flex().items_center().justify_between().child(div().text_size(px(sz::SM)).text_color(t.text_2).child("Exports at")).child(
                        Button::new("master-loudness", target_label).small().icon_after("chevron-down").on_click(|e, _, cx| crate::views::mixer::loudness_menu(e.position(), cx)),
                    ),
                );
            }
            body.push(mix.into_any_element());
        }
        let effects = self.chain_rows(target, &chain, cx);
        body.push(
            section(cx)
                .child(div().flex().items_center().justify_between().child(caps("Effects", cx)).child(Button::new("strip-add-effect", "Add").small().ghost().with_icon("plus").on_click(move |e, _, cx| open_browser(target, e.position(), cx))))
                .child(effects)
                .into_any_element(),
        );
        Some(vec![super::header(name, Some(kind), cx).into_any_element(), super::scroll("inspector-strip", body).into_any_element()])
    }
}

fn list(v: &Value) -> String {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use serde_json::json;

    use super::Target;
    use crate::store::StoreExt;
    use crate::tests::{setup, store_settles};
    use crate::ui::scrub::ScrubChange;

    /// The Audio section's gain slider sets the clip's level; a picked mixer strip shows the
    /// track's mix, whose fader moves the track.
    #[gpui::test]
    fn the_audio_section_and_a_strip_change_the_sound(cx: &mut TestAppContext) {
        let Ok(tools) = kimchi_media::Tools::locate() else { return eprintln!("ffmpeg not found; skipping") };
        let (f, view, cx) = setup(cx);
        let wav = f.session.data_dir.join("voice.wav");
        std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=330:duration=3"]).arg(&wav).status().unwrap();
        let v = f.call("media.import", json!({ "paths": [wav], "place": true, "start": 0, "trackId": "Audio 1" }));
        let clip: kimchi_core::Id = v["clips"][0].as_str().unwrap().parse().unwrap();
        store_settles(cx, |s| s.clip(clip).is_some());
        cx.update(|_, cx| cx.store().update(cx, |s, cx| s.set_selection(vec![clip], cx)));
        cx.run_until_parked();
        let inspector = cx.update(|_, cx| view.read(cx).editor().read(cx).inspector.clone());
        let gain = cx.update(|_, cx| inspector.read(cx).audio.gain.clone());
        cx.update(|_, cx| gain.update(cx, |_, cx| cx.emit(ScrubChange { value: -6.0, final_: true })));
        let p = f.settle(cx, |p| p.clip(clip).is_some_and(|c| (c.volume - 0.501).abs() < 1e-3));
        assert!((p.clip(clip).unwrap().volume - 0.501).abs() < 1e-3);

        // A strip picked in the mixer: the inspector shows its track.
        let track = p.tracks.iter().find(|t| t.name == "Audio 1").unwrap().id;
        cx.update(|_, cx| cx.store().update(cx, |s, cx| {
            s.clear_selection(cx);
            s.set_audio(|a| a.strip = Some(Target::Track(track)), cx);
        }));
        cx.run_until_parked();
        let fader = cx.update(|_, cx| inspector.read(cx).audio.strip_gain.clone());
        cx.update(|_, cx| fader.update(cx, |_, cx| cx.emit(ScrubChange { value: -4.0, final_: true })));
        let p = f.settle(cx, |p| p.track(track).is_some_and(|t| t.mix.gain_db == -4.0));
        assert_eq!(p.track(track).unwrap().mix.gain_db, -4.0);
    }
}
