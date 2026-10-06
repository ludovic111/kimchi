//! An effect's panel: a floating panel with a control for every parameter the effect declares
//! (its `ParamInfo`): knobs for continuous values (logarithmic for frequencies), a menu for
//! choices, a switch for on/off; each value as text you can click and type ("-6 dB", "2.5k",
//! "Hall"), and for tracks, buses and the master a keyframe button that automates the
//! parameter at the playhead. The header has the bypass switch, the factory presets and reset.
//! It can be dragged by its header. External plugins show their parameter list the same way
//! (their own editor windows are not hosted in kimchi's window).
//!
//! Every change is `audio.setEffect` (a knob drag shares one coalesce key; an automated
//! parameter gets a keyframe at the playhead), `audio.applyPreset` or `audio.addAutomationKey`.

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, ElementId, Entity, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString, Subscription,
    Window, anchored, deferred, div, point, prelude::*, px,
};
use kimchi_audio::plugins::ParamInfo;
use kimchi_core::audio::{Insert, effect_key};
use kimchi_core::Project;
use serde_json::json;

use super::{Target, target_key, widgets};
use crate::playback::Playback;
use crate::store::{MenuEntry, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{GlassExt, drag, icon, motion, switch, tooltip};

const W: f32 = 400.;
const CELL_W: f32 = 88.;

enum PanelDrag {
    Move { from: Point<Pixels>, start: Point<Pixels> },
    Knob { id: u32, y0: Pixels, n0: f64, info: ParamInfo, key: String },
}

pub struct EffectPanel {
    store: Entity<Store>,
    playback: Entity<Playback>,
    /// Top-left corner, once dragged (else it sits at the top right of the window).
    position: Option<Point<Pixels>>,
    drag: Option<PanelDrag>,
    drags: u64,
    /// Values shown while a drag's commands are on their way.
    shown: HashMap<u32, f64>,
    typing: Option<(u32, Entity<TextInput>, Subscription)>,
    /// Parameters per plugin id (external plugins are loaded once to ask).
    infos: HashMap<String, Vec<ParamInfo>>,
    _subs: Vec<Subscription>,
}

/// A parameter's value snapped to its steps.
pub fn stepped(info: &ParamInfo, v: f64) -> f64 {
    if info.steps == 0 || info.max <= info.min {
        return v.clamp(info.min.min(info.max), info.max.max(info.min));
    }
    let f = ((v - info.min) / (info.max - info.min)).clamp(0., 1.);
    info.min + (f * info.steps as f64).round() / info.steps as f64 * (info.max - info.min)
}

/// What a parameter is at `t`: its automation, else the slot's value, else its default.
fn value_of(p: &Project, target: Target, e: &Insert, info: &ParamInfo, t: f64) -> f64 {
    let keys = target.keyframes(p).unwrap_or_default();
    kimchi_core::anim::number_at(&keys, &effect_key(&e.id, info.id), t).or_else(|| e.params.get(&info.id).copied()).unwrap_or(info.default)
}

impl EffectPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let playback = store.read(cx).playback.clone();
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&playback, |this, _, cx| {
                // Automated values follow the playhead.
                if this.store.read(cx).audio.effect.is_some() {
                    cx.notify();
                }
            }),
        ];
        Self { store, playback, position: None, drag: None, drags: 0, shown: HashMap::new(), typing: None, infos: HashMap::new(), _subs: subs }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.typing = None;
        self.store.update(cx, |s, cx| s.set_audio(|a| a.effect = None, cx));
    }

    fn infos(&mut self, plugin: &str) -> Vec<ParamInfo> {
        self.infos.entry(plugin.to_string()).or_insert_with(|| kimchi_audio::plugins::params(plugin).unwrap_or_default()).clone()
    }

    pub(super) fn set(&mut self, target: Target, slot: &str, id: u32, value: serde_json::Value, key: Option<&str>, cx: &mut Context<Self>) {
        let mut params = json!({ "target": target_key(target), "slot": slot, "params": { id.to_string(): value } });
        if let Some(k) = key {
            params["coalesce"] = json!(k);
        }
        self.store.update(cx, |s, cx| s.run("audio.setEffect", params, cx));
    }

    fn drag_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((target, slot)) = self.store.read(cx).audio.effect.clone() else { return };
        match &self.drag {
            Some(PanelDrag::Move { from, start }) => {
                self.position = Some(point(start.x + (e.position.x - from.x), start.y + (e.position.y - from.y)));
            }
            Some(PanelDrag::Knob { id, y0, n0, info, key }) => {
                let fine = if e.modifiers.shift { 0.15 } else { 1.0 };
                let n = (n0 + f32::from(*y0 - e.position.y) as f64 / 150. * fine).clamp(0., 1.);
                let v = stepped(info, info.denormalize(n));
                let (id, key) = (*id, key.clone());
                if self.shown.insert(id, v) != Some(v) {
                    self.set(target, &slot, id, json!(v), Some(&key), cx);
                }
            }
            None => return,
        }
        cx.notify();
    }

    fn drag_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        cx.notify();
    }

    fn start_typing(&mut self, target: Target, slot: String, info: &ParamInfo, value: f64, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx);
            i.mono = true;
            i.set_text(info.text(value), cx);
            i.select_all_text(cx);
            i
        });
        crate::ui::input::focus(&input, window, cx);
        let id = info.id;
        let sub = cx.subscribe(&input, move |this, input, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Blur => {
                let text = input.read(cx).text().trim().to_string();
                if !text.is_empty() {
                    this.set(target, &slot, id, json!(text), None, cx);
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
        self.typing = Some((id, input, sub));
        cx.notify();
    }

    fn presets_menu(&self, target: Target, slot: String, effect: &str, position: Point<Pixels>, cx: &mut Context<Self>) {
        let entries: Vec<MenuEntry> = ryolune_presets(effect)
            .into_iter()
            .map(|name| {
                let slot = slot.clone();
                MenuItem::new(name, move |_, cx| super::run_cmd(cx, "audio.applyPreset", json!({ "target": target_key(target), "slot": slot, "preset": name }))).entry()
            })
            .collect();
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn choice_menu(&self, target: Target, slot: String, info: &ParamInfo, position: Point<Pixels>, cx: &mut Context<Self>) {
        let n = info.labels.len().max(1);
        let entries: Vec<MenuEntry> = info
            .labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let v = if n > 1 { info.min + i as f64 / (n - 1) as f64 * (info.max - info.min) } else { info.min };
                let (slot, id) = (slot.clone(), info.id);
                MenuItem::new(label.clone(), move |_, cx| super::run_cmd(cx, "audio.setEffect", json!({ "target": target_key(target), "slot": slot, "params": { id.to_string(): v } }))).entry()
            })
            .collect();
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    fn param_cell(&mut self, p: &Project, target: Target, e: &Insert, info: &ParamInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let playhead = self.playback.read(cx).playhead;
        let slot = e.id.clone();
        let value = self.shown.get(&info.id).copied().unwrap_or_else(|| value_of(p, target, e, info, playhead));
        let automatable = !matches!(target, Target::Clip(_));
        let automated = target.keyframes(p).is_some_and(|k| k.contains_key(&effect_key(&e.id, info.id)));
        let id = info.id;
        let name: SharedString = info.name.clone().into();
        let cell_id = ElementId::Name(format!("param-{}-{}", e.id, info.id).into());
        let switch_like = info.steps == 1 && info.labels.len() <= 2;
        let control: AnyElement = if switch_like {
            let on = value >= (info.min + info.max) / 2.;
            let (slot, max, min) = (slot.clone(), info.max, info.min);
            switch(ElementId::Name(format!("switch-{}-{}", e.id, id).into()), "", on, move |v, _, cx| {
                super::run_cmd(cx, "audio.setEffect", json!({ "target": target_key(target), "slot": slot, "params": { id.to_string(): if v { max } else { min } } }))
            }, cx)
            .into_any_element()
        } else if !info.labels.is_empty() {
            let info2 = info.clone();
            let slot2 = slot.clone();
            div()
                .id(ElementId::Name(format!("choice-{}-{}", e.id, id).into()))
                .w_full()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_between()
                .px(px(6.))
                .rounded(px(sz::R_SM))
                .bg(t.bg_sunken)
                .border_1()
                .border_color(t.line)
                .text_size(px(sz::SM))
                .cursor_pointer()
                .hover(|d| d.border_color(t.line_strong))
                .child(div().min_w_0().truncate().child(info.text(value)))
                .child(icon("chevron-down").size(px(11.)).text_color(t.text_2))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| this.choice_menu(target, slot2.clone(), &info2, ev.position(), cx)))
                .into_any_element()
        } else {
            let active = matches!(&self.drag, Some(PanelDrag::Knob { id: d, .. }) if *d == id);
            let info2 = info.clone();
            div()
                .id(ElementId::Name(format!("knob-{}-{}", e.id, id).into()))
                .size(px(38.))
                .cursor(gpui::CursorStyle::ResizeUpDown)
                .child(widgets::knob(info.normalize(value) as f32, info.min < 0. && info.max > 0. && (info.min + info.max).abs() < 1e-9, active, &t).size_full())
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    let Some((target, slot)) = this.store.read(cx).audio.effect.clone() else { return };
                    if ev.click_count == 2 {
                        this.set(target, &slot, id, json!(info2.default), None, cx);
                        return;
                    }
                    this.drags += 1;
                    let key = format!("effect-{slot}-{id}-{}", this.drags);
                    this.drag = Some(PanelDrag::Knob { id, y0: ev.position.y, n0: info2.normalize(value), info: info2.clone(), key });
                    cx.notify();
                }))
                .into_any_element()
        };
        let text: AnyElement = match &self.typing {
            Some((tid, input, _)) if *tid == id => div().w_full().child(input.clone()).into_any_element(),
            _ if switch_like => div().into_any_element(),
            _ => {
                let info2 = info.clone();
                let slot2 = slot.clone();
                div()
                    .id(ElementId::Name(format!("value-{}-{}", e.id, id).into()))
                    .max_w_full()
                    .truncate()
                    .px(px(3.))
                    .rounded(px(sz::R_XS))
                    .font_family(MONO)
                    .text_size(px(10.5))
                    .text_color(if automated { t.accent_text } else { t.text })
                    .cursor_text()
                    .hover(|d| d.bg(t.hover))
                    .child(info.text(value))
                    .on_click(cx.listener(move |this, _, window, cx| this.start_typing(target, slot2.clone(), &info2, value, window, cx)))
                    .into_any_element()
            }
        };
        let key_btn = automatable.then(|| {
            let slot2 = slot.clone();
            div()
                .id(ElementId::Name(format!("auto-{}-{}", e.id, id).into()))
                .flex_none()
                .text_color(if automated { t.accent_text } else { t.text_3 })
                .cursor_pointer()
                .hover(|d| d.text_color(t.accent_text))
                .tooltip(move |_, cx| tooltip(if automated { "Keyframe at the playhead · right-click: remove the automation" } else { "Automate: a keyframe at the playhead" }.into(), cx))
                .child(icon("diamond").size(px(10.)))
                .on_click({
                    let slot = slot2.clone();
                    move |_, _, cx| super::run_cmd(cx, "audio.addAutomationKey", json!({ "target": target_key(target), "property": effect_key(&slot, id) }))
                })
                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                    cx.stop_propagation();
                    if automated {
                        super::run_cmd(cx, "audio.removeAutomationKey", json!({ "target": target_key(target), "property": effect_key(&slot2, id) }));
                    }
                })
        });
        div()
            .id(cell_id)
            .w(px(CELL_W))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(3.))
            .p(px(4.))
            .rounded(px(sz::R_SM))
            .hover(|d| d.bg(t.hover))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(3.))
                    .child(div().min_w_0().truncate().text_size(px(10.5)).text_color(t.text_2).child(name.clone()))
                    .children(key_btn),
            )
            .child(control)
            .child(text)
            .into_any_element()
    }
}

/// The factory presets ryolune ships for a stock effect, by name.
pub fn ryolune_presets(effect: &str) -> Vec<&'static str> {
    kimchi_control::commands::audio::preset_names(effect)
}

impl Render for EffectPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let (Some((target, slot)), Some(p)) = (s.audio.effect.clone(), s.project.clone()) else {
            self.shown.clear();
            return div().into_any_element();
        };
        let Some(e) = target.chain(&p).ok().and_then(|c| c.into_iter().find(|e| e.id == slot)) else {
            // The slot went (undo, another client): nothing to show.
            return div().into_any_element();
        };
        if self.drag.is_none() {
            self.shown.clear();
        }
        let infos = self.infos(&e.plugin_id());
        let info = kimchi_audio::plugins::effect(&e.plugin_id()).ok();
        let owner = target.name(&p);
        let presets = ryolune_presets(&e.name);
        let bypassed = e.is_bypassed();
        let cells: Vec<AnyElement> = infos.iter().map(|i| self.param_cell(&p, target, &e, i, cx)).collect();
        let viewport = window.viewport_size();
        let pos = self.position.unwrap_or_else(|| point(viewport.width - px(W + 24.), px(64.)));
        let (slot2, slot3, name) = (slot.clone(), slot.clone(), e.name.clone());
        let header = div()
            .id("effect-panel-header")
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .h(px(40.))
            .border_b_1()
            .border_color(t.line)
            .cursor(gpui::CursorStyle::OpenHand)
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                let start = this.position.unwrap_or_else(|| point(window.viewport_size().width - px(W + 24.), px(64.)));
                this.drag = Some(PanelDrag::Move { from: ev.position, start });
                cx.notify();
            }))
            .child(
                div()
                    .id("effect-power")
                    .flex_none()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if bypassed { t.bg_sunken } else { t.accent_soft })
                    .text_color(if bypassed { t.text_3 } else { t.accent_text })
                    .cursor_pointer()
                    .tooltip(move |_, cx| tooltip(if bypassed { "Switch on" } else { "Bypass" }.into(), cx))
                    .child(icon("power").size(px(12.)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| super::run_cmd(cx, "audio.setEffect", json!({ "target": target_key(target), "slot": slot2, "bypassed": !bypassed }))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().font_weight(FontWeight::SEMIBOLD).text_size(px(sz::BASE)).child(e.name.clone()))
                    .child(div().truncate().text_size(px(sz::XS)).text_color(t.text_2).child(format!("on {owner}{}", info.as_ref().map(|i| format!(" · {}", if i.format == "ryolune" { "stock" } else { i.format.as_str() })).unwrap_or_default()))),
            )
            .when(!presets.is_empty(), |d| {
                d.child(
                    div()
                        .id("effect-presets")
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .px(px(7.))
                        .h(px(24.))
                        .rounded(px(sz::R_SM))
                        .border_1()
                        .border_color(t.line_strong)
                        .text_size(px(sz::SM))
                        .cursor_pointer()
                        .hover(|d| d.bg(t.hover))
                        .child("Presets")
                        .child(icon("chevron-down").size(px(11.)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| this.presets_menu(target, slot3.clone(), &name, ev.position(), cx))),
                )
            })
            .child(
                div()
                    .id("effect-reset")
                    .flex_none()
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.text))
                    .tooltip(|_, cx| tooltip("Every parameter back to its default".into(), cx))
                    .child(icon("rotate-ccw"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click({
                        let slot = slot.clone();
                        move |_, _, cx| super::run_cmd(cx, "audio.setEffect", json!({ "target": target_key(target), "slot": slot, "reset": true }))
                    }),
            )
            .child(
                div()
                    .id("effect-close")
                    .flex_none()
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.text))
                    .tooltip(|_, cx| tooltip("Close".into(), cx))
                    .child(icon("x"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            );
        let description = info.as_ref().map(|i| i.description.clone()).filter(|d| !d.is_empty());
        let body = if cells.is_empty() {
            div().p(px(14.)).text_size(px(sz::SM)).text_color(t.text_2).child(format!("{} has no parameters kimchi can show. Its state is kept with the project and played as saved.", e.name)).into_any_element()
        } else {
            div().id("effect-params").flex().flex_wrap().gap(px(4.)).p(px(8.)).overflow_y_scroll().max_h(px((f32::from(viewport.height) - 200.).max(160.))).children(cells).into_any_element()
        };
        let panel = div()
            .id("effect-panel")
            .occlude()
            .relative()
            .w(px(W))
            .flex()
            .flex_col()
            .rounded(px(sz::R_MD))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .when(bypassed, |d| d.opacity(0.92))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .when_some(description, |d, text| d.child(div().px(px(12.)).pt(px(8.)).text_size(px(sz::XS)).text_color(t.text_2).child(text)))
            .child(body)
            .child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(t.line)
                    .text_size(px(10.5))
                    .text_color(t.text_3)
                    .child(if matches!(target, Target::Clip(_)) { "Drag a knob (Shift: finer) · double-click: default · click a value to type it" } else { "Drag a knob (Shift: finer) · double-click: default · ◆ keyframes it at the playhead" }),
            )
            .when(self.drag.is_some(), |d| d.child(drag::track(cx.entity(), Self::drag_move, Self::drag_up)));
        deferred(anchored().position(pos).snap_to_window_with_margin(px(8.)).child(motion::enter(panel, "effect-panel-in", motion::BASE, (0., 6.)))).with_priority(2).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_snap_values() {
        let info = ParamInfo { id: 0, name: "Mode".into(), min: 0.0, max: 3.0, default: 0.0, unit: String::new(), steps: 3, log: false, labels: vec![] };
        assert_eq!(stepped(&info, 1.4), 1.0);
        assert_eq!(stepped(&info, 9.0), 3.0);
        assert!(!ryolune_presets("Space").is_empty());
    }
}
