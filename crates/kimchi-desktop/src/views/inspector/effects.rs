//! The inspector's picture effects, transition and speed controls.
//!
//! - Colour (picture clips): looks, the corrections as sliders (each with a keyframe toggle at
//!   the playhead; an animated one gets a keyframe instead of a new value), chroma key, LUT.
//! - Transition in: the kinds as a grid ("None" removes it) and the length; on audio tracks it
//!   is the crossfade.
//! - Speed extras for the Timing section: presets, reverse, freeze frame.
//!
//! Everything goes through `clip.setEffects`, `transition.set` / `transition.remove`,
//! `clip.update` and `clip.freezeFrame`.

use gpui::{AnyElement, App, Context, Entity, PathPromptOptions, SharedString, Subscription, Window, div, prelude::*, px};
use kimchi_core::effects::{EFFECT_PROPS, LOOKS, look_of};
use kimchi_core::transition::{self, KINDS};
use kimchi_core::{Clip, ClipContent, Id, MediaKind, Project, TrackKind};
use serde_json::{Value, json};

use super::color::{ColorChange, ColorField};
use super::slider::Slider;
use super::Inspector;
use crate::store::StoreExt;
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::scrub::{Scrub, ScrubChange};
use crate::ui::{Button, switch};

/// Correction sliders: (property, label, minimum). Maximums are 1.
const FIELDS: [(&str, &str, f64); 7] = [
    ("brightness", "Bright", -1.0),
    ("contrast", "Contrast", -1.0),
    ("saturation", "Saturate", -1.0),
    ("temperature", "Warmth", -1.0),
    ("tint", "Tint", -1.0),
    ("vignette", "Vignette", 0.0),
    ("sharpen", "Sharpen", 0.0),
];

/// Speed presets offered under the speed field.
const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

/// The retained controls of these sections.
pub struct EffectFields {
    sliders: Vec<(&'static str, Entity<Slider>)>,
    key_color: Entity<ColorField>,
    key_similarity: Entity<Slider>,
    key_softness: Entity<Slider>,
    key_spill: Entity<Slider>,
    lut_strength: Entity<Slider>,
    transition_len: Entity<Scrub>,
}

impl EffectFields {
    pub fn new(subs: &mut Vec<Subscription>, cx: &mut Context<Inspector>) -> Self {
        let mut slider = |cx: &mut Context<Inspector>, min: f64, neutral: f64, to: Box<dyn Fn(f64) -> (Value, &'static str)>| {
            let e = cx.new(|cx| Slider::new(min, 1.0, cx).neutral(neutral));
            subs.push(cx.subscribe(&e, move |this: &mut Inspector, _, ch: &ScrubChange, cx| {
                let (params, key) = to(ch.value);
                this.set_effects(params, key, ch.final_, cx);
            }));
            e
        };
        let sliders = FIELDS
            .iter()
            .map(|&(prop, _, min)| (prop, slider(cx, min, 0.0, Box::new(move |v| (json!({ prop: v }), prop)))))
            .collect();
        let key_similarity = slider(cx, 0.0, 0.5, Box::new(|v| (json!({ "chromaKey": { "similarity": v } }), "keySimilarity")));
        let key_softness = slider(cx, 0.0, 0.1, Box::new(|v| (json!({ "chromaKey": { "softness": v } }), "keySoftness")));
        let key_spill = slider(cx, 0.0, 0.5, Box::new(|v| (json!({ "chromaKey": { "spill": v } }), "keySpill")));
        let lut_strength = slider(cx, 0.0, 1.0, Box::new(|v| (json!({ "lutStrength": v }), "lutStrength")));
        let key_color = cx.new(|cx| ColorField::new(None, cx));
        subs.push(cx.subscribe(&key_color, |this: &mut Inspector, _, ch: &ColorChange, cx| {
            this.set_effects(json!({ "chromaKey": ch.0.get(..7).unwrap_or(&ch.0) }), "keyColor", true, cx)
        }));
        let transition_len = cx.new(|_| Scrub::new("Length", 0.05, 2).unit("s").range(0.05, 30.0));
        subs.push(cx.subscribe(&transition_len, |this: &mut Inspector, _, ch: &ScrubChange, cx| {
            let Some(id) = this.single(cx) else { return };
            let mut p = json!({ "clipIds": [id], "duration": ch.value });
            if !ch.final_ {
                p["coalesce"] = json!(format!("{id}:transition"));
            }
            this.store.update(cx, |s, cx| s.run("transition.set", p, cx));
        }));
        Self { sliders, key_color, key_similarity, key_softness, key_spill, lut_strength, transition_len }
    }
}

impl Inspector {
    /// `clip.setEffects` on the selected clip (a keyframe at the playhead when the property is
    /// animated); a drag shares one coalesce key.
    pub(super) fn set_effects(&mut self, mut params: Value, key: &str, final_: bool, cx: &mut Context<Self>) {
        if self.keyframe_instead(&params, key, final_, cx) {
            return;
        }
        let Some(id) = self.single(cx) else { return };
        params["clipIds"] = json!([id]);
        if !final_ {
            params["coalesce"] = json!(format!("{id}:{key}"));
        }
        self.store.update(cx, |s, cx| s.run("clip.setEffects", params, cx));
    }

    /// Fills the effect and transition controls from the clip at the playhead.
    pub(super) fn sync_effects(&mut self, clip: &Clip, playhead: f64, window: &Window, cx: &mut Context<Self>) {
        let fx = clip.effects_at(playhead);
        for (prop, s) in &self.fx.sliders {
            let v = fx.get(prop).unwrap_or(0.0);
            s.update(cx, |s, _| s.set_value(v));
        }
        if let Some(k) = &fx.chroma_key {
            self.fx.key_similarity.update(cx, |s, _| s.set_value(k.similarity));
            self.fx.key_softness.update(cx, |s, _| s.set_value(k.softness));
            self.fx.key_spill.update(cx, |s, _| s.set_value(k.spill));
            let c = k.color.clone();
            self.fx.key_color.update(cx, |f, cx| f.set_value(&c, window, cx));
        }
        if let Some(l) = &fx.lut {
            self.fx.lut_strength.update(cx, |s, _| s.set_value(l.strength));
        }
        if let Some(tr) = &clip.transition {
            self.fx.transition_len.update(cx, |s, _| s.set_value(tr.duration));
        }
    }

    /// The Colour section for a picture clip.
    pub(super) fn color_section(&mut self, clip: &Clip, fps: f64, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let id = clip.id;
        let playhead = self.store.read(cx).playback.read(cx).playhead;
        let tol = 0.5 / fps.max(1.0);
        let local = playhead - clip.start;
        let inside = playhead >= clip.start - tol && playhead <= clip.end() + tol;
        let fx = clip.effects_at(playhead);
        let current = look_of(&fx);

        let looks = div().flex().flex_wrap().gap(px(4.)).children(LOOKS.iter().map(|l| {
            let look = l.id;
            Button::new(SharedString::from(format!("look-{look}")), l.label)
                .small()
                .selected(current == Some(look) && look != "none")
                .tooltip(l.doc)
                .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.run("clip.setEffects", json!({ "clipIds": [id], "look": look }), cx)))
        }));

        let rows = div().flex().flex_col().gap(px(6.)).children(self.fx.sliders.iter().map(|(prop, slider)| {
            let prop: &'static str = prop;
            let label = FIELDS.iter().find(|f| f.0 == prop).map_or(prop, |f| f.1);
            let v = fx.get(prop).unwrap_or(0.0);
            let keys = clip.keyframes.get(prop);
            let here = keys.is_some_and(|k| k.iter().any(|k| (k.time - local).abs() <= tol));
            let animated = keys.is_some();
            let tip = match (here, animated) {
                (true, _) => format!("Remove the {label} keyframe here"),
                (false, true) => format!("Add a {label} keyframe at the playhead"),
                (false, false) => format!("Animate {label}: a first keyframe at the playhead"),
            };
            let mut key = Button::icon(SharedString::from(format!("fx-key-{prop}")), "diamond", tip).small().selected(here).disabled(!inside);
            if animated && !here {
                key = key.color(t.accent_text);
            }
            let key = key.on_click(move |_, _, cx| {
                cx.store().update(cx, |s, cx| {
                    let playhead = s.playback.read(cx).playhead;
                    let name = if here { "clip.removeKeyframe" } else { "clip.addKeyframe" };
                    s.run(name, json!({ "clipId": id, "property": prop, "time": playhead }), cx);
                })
            });
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().w(px(58.)).flex_none().text_size(px(sz::SM)).text_color(t.text_2).child(label))
                .child(div().flex_1().flex().child(slider.clone()))
                .child(div().w(px(34.)).flex_none().flex().justify_end().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(signed(v)))
                .child(key)
        }));

        // Chroma key.
        let keyed = clip.effects.chroma_key.is_some();
        let key_rows = keyed.then(|| {
            let row = |label: &'static str, s: &Entity<Slider>, v: f64| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(58.)).flex_none().text_size(px(sz::SM)).text_color(t.text_2).child(label))
                    .child(div().flex_1().flex().child(s.clone()))
                    .child(div().w(px(34.)).flex_none().flex().justify_end().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(format!("{}%", (v * 100.0).round())))
            };
            let k = clip.effects.chroma_key.clone().unwrap_or_default();
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(self.fx.key_color.clone())
                .child(row("Range", &self.fx.key_similarity, k.similarity))
                .child(row("Edge", &self.fx.key_softness, k.softness))
                .child(row("Spill", &self.fx.key_spill, k.spill))
        });

        // LUT.
        let lut = clip.effects.lut.clone();
        let lut_el = match &lut {
            None => Button::new("lut-load", "Load a LUT (.cube)…").small().with_icon("folder-open").full_width().on_click(move |_, _, cx| pick_lut(id, cx)).into_any_element(),
            Some(l) => {
                let name = std::path::Path::new(&l.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| l.path.clone());
                let missing = !std::path::Path::new(&l.path).is_file();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(crate::ui::icon("palette").text_color(t.text_2))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(sz::SM)).text_color(if missing { t.danger } else { t.text }).child(if missing { format!("{name} (missing)") } else { name }))
                            .child(Button::icon("lut-change", "folder-open", "Choose another LUT").small().on_click(move |_, _, cx| pick_lut(id, cx)))
                            .child(Button::icon("lut-remove", "x", "Remove the LUT").small().on_click(move |_, _, cx| {
                                cx.store().update(cx, |s, cx| s.run("clip.setEffects", json!({ "clipIds": [id], "lut": null }), cx))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().w(px(58.)).flex_none().text_size(px(sz::SM)).text_color(t.text_2).child("Strength"))
                            .child(div().flex_1().flex().child(self.fx.lut_strength.clone()))
                            .child(div().w(px(34.)).flex_none().flex().justify_end().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(format!("{}%", (l.strength * 100.0).round()))),
                    )
                    .into_any_element()
            }
        };

        let any = !clip.effects.is_default() || EFFECT_PROPS.iter().any(|p| clip.keyframes.contains_key(*p));
        self.fold("colour", "Colour", cx)
            .trailing(Button::icon("fx-reset", "rotate-ccw", "Remove every colour effect, key and LUT").small().disabled(!any).on_click(move |_, _, cx| {
                cx.store().update(cx, |s, cx| {
                    let mut steps = vec![json!({ "command": "clip.setEffects", "params": { "clipIds": [id], "reset": true } })];
                    for p in EFFECT_PROPS {
                        steps.push(json!({ "command": "clip.setKeyframes", "params": { "clipId": id, "property": p, "keyframes": [] } }));
                    }
                    s.run("project.batch", json!({ "commands": steps, "label": "reset colour" }), cx)
                })
            }))
            .child(looks)
            .child(rows)
            .child(switch(
                "chroma-key",
                "Key out a colour (green screen)",
                keyed,
                move |on, _, cx| cx.store().update(cx, |s, cx| s.run("clip.setEffects", json!({ "clipIds": [id], "chromaKey": on }), cx)),
                cx,
            ))
            .children(key_rows)
            .child(lut_el)
            .into_any_element()
    }

    /// The transition (or, on audio tracks, crossfade) at the clip's start.
    pub(super) fn transition_section(&mut self, clip: &Clip, project: &Project, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let id = clip.id;
        let Some((ti, ci)) = project.locate_clip(id) else { return div().into_any_element() };
        let track = &project.tracks[ti];
        let audio = track.kind == TrackKind::Audio;
        let span = transition::span(track, ci);
        let current = clip.transition.as_ref().map(|tr| tr.kind);
        let cut_from = ci.checked_sub(1).map(|j| &track.clips[j]).filter(|p| (p.end() - clip.start).abs() <= transition::CUT_TOLERANCE);
        let note = match (cut_from, current.is_some()) {
            (Some(p), _) => format!("From “{}”, centred on the cut. Both clips play on past it.", p.name),
            (None, true) => "Nothing ends right before this clip: it comes in over what is below.".to_string(),
            (None, false) => "Nothing ends right before this clip: a transition brings it in over what is below.".to_string(),
        };
        let chip = |key: &'static str, label: &'static str, selected: bool, tip: &'static str| {
            Button::new(SharedString::from(format!("tr-{key}")), label).small().selected(selected).tooltip(tip).full_width()
        };
        let mut grid = div().grid().grid_cols(3).gap(px(4.));
        grid = grid.child(chip("none", "None", current.is_none(), "No transition: a straight cut").on_click(move |_, _, cx| {
            cx.store().update(cx, |s, cx| s.run("transition.remove", json!({ "clipIds": [id] }), cx))
        }));
        // On audio tracks every kind is the same crossfade: offer just that.
        let kinds: Vec<_> = if audio { KINDS.iter().filter(|k| k.id == "dissolve").collect() } else { KINDS.iter().collect() };
        for k in kinds {
            let kind = k.id;
            let label = if audio { "Crossfade" } else { k.label };
            grid = grid.child(chip(kind, label, current == Some(k.kind), k.doc).on_click(move |_, _, cx| {
                cx.store().update(cx, |s, cx| s.run("transition.set", json!({ "clipIds": [id], "kind": kind }), cx))
            }));
        }
        let shortened = match (span, &clip.transition) {
            (Some(sp), Some(tr)) if (sp.duration() - tr.duration).abs() > 1e-3 => Some(format!("Plays for {:.2} s: the clips are too short for more.", sp.duration())),
            _ => None,
        };
        self.fold("transition", if audio { "Crossfade in" } else { "Transition in" }, cx)
            .child(grid)
            .when(current.is_some(), |d| d.child(self.fx.transition_len.clone()))
            .children(shortened.map(|s| div().text_size(px(sz::SM)).text_color(t.warning).child(s)))
            .child(div().text_size(px(sz::SM)).text_color(t.text_3).line_height(px(sz::SM * 1.5)).child(note))
            .into_any_element()
    }
}

/// Speed presets, reverse and freeze frame, for the Timing section of a media clip.
pub fn speed_extras(clip: &Clip, kind: Option<MediaKind>, playhead: f64, fps: f64, cx: &App) -> Option<AnyElement> {
    let id = clip.id;
    let timed = matches!(kind, Some(MediaKind::Video | MediaKind::Audio));
    let picture = matches!(kind, Some(MediaKind::Video | MediaKind::Image)) || matches!(clip.content, ClipContent::Text { .. } | ClipContent::Solid { .. } | ClipContent::Motion { .. });
    if !timed && !picture {
        return None;
    }
    let mut out = div().flex().flex_col().gap(px(8.));
    if timed {
        let speed = clip.speed;
        out = out.child(div().flex().gap(px(4.)).children(SPEEDS.iter().map(|&v| {
            let label = format!("{}×", if v < 1.0 { format!("{v}") } else { format!("{}", v as u32) });
            let b = Button::new(SharedString::from(format!("speed-{v}")), label)
                .small()
                .selected((speed - v).abs() < 1e-6)
                .full_width()
                .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.run("clip.update", json!({ "clipId": id, "speed": v }), cx)));
            div().flex_1().min_w_0().child(b)
        })));
        let reverse = clip.reverse;
        out = out.child(switch(
            "reverse",
            "Play backwards",
            reverse,
            move |on, _, cx| cx.store().update(cx, |s, cx| s.run("clip.update", json!({ "clipId": id, "reverse": on }), cx)),
            cx,
        ));
    }
    if picture && kind != Some(MediaKind::Image) {
        let inside = playhead >= clip.start && playhead < clip.end() - 0.5 / fps.max(1.0);
        out = out.child(
            Button::new("freeze-frame", "Freeze frame at the playhead")
                .small()
                .with_icon("snowflake")
                .full_width()
                .disabled(!inside)
                .tooltip("Splits the clip at the playhead and holds that frame for 2 s, pushing the rest of the track later")
                .on_click(move |_, _, cx| {
                    cx.store().update(cx, |s, cx| {
                        let time = s.playback.read(cx).playhead;
                        s.run_then("clip.freezeFrame", json!({ "clipId": id, "time": time }), cx, |s, v, cx| s.set_selection(crate::app::created(&v), cx))
                    })
                }),
        );
    }
    Some(out.into_any_element())
}

/// Asks for a `.cube` file and puts it on the clip.
fn pick_lut(id: Id, cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use LUT".into()) });
    let store = cx.store();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else { return };
        let Some(path) = paths.into_iter().next() else { return };
        store.update(cx, |s, cx| s.run("clip.setEffects", json!({ "clipIds": [id], "lut": path.to_string_lossy() }), cx));
    })
    .detach();
}

/// `+0.25`, `−0.40`, `0`.
fn signed(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 {
        "0".into()
    } else if r > 0.0 {
        format!("+{r:.2}")
    } else {
        format!("−{:.2}", -r)
    }
}

#[cfg(test)]
impl Inspector {
    /// The slider of a colour correction, for tests.
    pub fn effect_slider(&self, prop: &str) -> Option<Entity<Slider>> {
        self.fx.sliders.iter().find(|(p, _)| *p == prop).map(|(_, s)| s.clone())
    }
}
