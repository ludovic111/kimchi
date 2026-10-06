//! Transitions on the lanes: a badge over the span each one plays (across the cut), with edges
//! that drag its length, a menu of kinds on right-click; and a "+" on every bare cut that adds a
//! dissolve. Clicking a badge selects the clip it leads into, whose inspector has the rest.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{AnyElement, Context, MouseButton, MouseDownEvent, Pixels, Point, SharedString, div, prelude::*, px};
use kimchi_core::transition::{self, CUT_TOLERANCE, KINDS};
use kimchi_core::{Id, Project, TrackKind};
use serde_json::json;

use super::{Drag, Preview, TimelineBody, geom, track_h};
use crate::store::{MenuEntry, MenuItem, StoreExt};
use crate::theme::{ActiveTheme, MONO};
use crate::ui::icon;

/// Shortest length a drag sets.
const MIN_LEN: f64 = 0.05;

impl TimelineBody {
    /// Badges and "+" buttons for the visible rows.
    pub(super) fn transitions(&mut self, p: &Arc<Project>, rows: &geom::Rows, preview: Option<&Preview>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let pps = s.pps;
        let selection: HashSet<Id> = s.selection.iter().copied().collect();
        let (scroll_x, scroll_y, view_w) = (self.scroll_x, self.scroll_y, self.view_w());
        let view_h = f32::from(self.lanes.get().size.height);
        // While a clip moves or trims, the badges would be in the wrong place.
        if matches!(preview, Some(Preview::Move { .. } | Preview::Trim { .. })) {
            return vec![];
        }
        let mut out = vec![];
        for (ti, track) in p.tracks.iter().enumerate() {
            let h = track_h(track.kind);
            let top = rows.tops[ti] - scroll_y + 2.;
            if top + h < 0. || top > view_h + 4. {
                continue;
            }
            let mut track = track.clone();
            if let Some(Preview::Transition { clip, duration }) = preview
                && let Some(c) = track.clips.iter_mut().find(|c| c.id == *clip)
                && let Some(tr) = c.transition.as_mut()
            {
                tr.duration = *duration;
            }
            let in_view = |a: f64, b: f64| b * pps >= scroll_x - 32. && a * pps <= scroll_x + view_w + 32.;
            let mid = top + h / 2.;
            for span in transition::spans(&track) {
                if !in_view(span.start, span.end) {
                    continue;
                }
                let to = &track.clips[span.to];
                let Some(tr) = &to.transition else { continue };
                let id = to.id;
                let x = (span.start * pps - scroll_x) as f32;
                let w = ((span.duration() * pps) as f32).max(18.);
                let selected = selection.contains(&id);
                let label = if track.kind == TrackKind::Audio { "Crossfade" } else { tr.kind.label() };
                let cut = span.from.map(|_| to.start);
                let start = span.start;
                let tip: SharedString = format!("{label}, {:.2} s. Drag an edge to change its length; right-click for other kinds.", span.duration()).into();
                let edge = |name: &'static str, left: bool, cx: &mut Context<Self>| {
                    div()
                        .id(name)
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(px(6.))
                        .when(left, |d| d.left_0())
                        .when(!left, |d| d.right_0())
                        .cursor_ew_resize()
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.consumed = true;
                            this.store.update(cx, |s, cx| s.select(id, false, cx));
                            this.drag = Some(Drag::Transition { clip: id, cut, start, value: None });
                            cx.notify();
                        }))
                };
                out.push(
                    div()
                        .id(SharedString::from(format!("transition-{id}")))
                        .absolute()
                        .left(px(x))
                        .top(px(mid - 11.))
                        .w(px(w))
                        .h(px(22.))
                        .bg(gpui::black().opacity(0.62))
                        .border(px(if selected { 2. } else { 1. }))
                        .border_color(if selected { gpui::white() } else { gpui::white().opacity(0.45) })
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(4.))
                        .overflow_hidden()
                        .text_size(px(10.5))
                        .text_color(gpui::white())
                        .cursor_pointer()
                        .child(icon("blend").size(px(12.)).text_color(gpui::white()))
                        .when(w >= 96., |d| d.child(div().truncate().child(label)))
                        .when(w >= 150., |d| d.child(div().font_family(MONO).text_color(gpui::white().opacity(0.7)).child(format!("{:.2}s", span.duration()))))
                        .tooltip(move |_, cx| crate::ui::tooltip(tip.clone(), cx))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.consumed = true;
                            this.store.update(cx, |s, cx| s.select(id, false, cx));
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.consumed = true;
                            transition_menu(id, e.position, cx);
                        }))
                        .when(!track.locked, |d| d.child(edge("edge-l", true, cx)).child(edge("edge-r", false, cx)))
                        .into_any_element(),
                );
            }
            // A "+" on each cut that has no transition: shown when a clip at the cut is selected or on hover.
            if track.locked {
                continue;
            }
            for pair in track.clips.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                if b.transition.is_some() || (a.end() - b.start).abs() > CUT_TOLERANCE || !in_view(b.start, b.start) {
                    continue;
                }
                let id = b.id;
                let near = selection.contains(&a.id) || selection.contains(&id);
                let x = (b.start * pps - scroll_x) as f32;
                out.push(
                    div()
                        .id(SharedString::from(format!("add-transition-{id}")))
                        .absolute()
                        .left(px(x - 9.))
                        .top(px(mid - 9.))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(t.accent)
                        .text_color(gpui::white())
                        .border_1()
                        .border_color(gpui::white().opacity(0.7))
                        .cursor_pointer()
                        .opacity(if near { 0.95 } else { 0. })
                        .hover(|s| s.opacity(1.))
                        .child(icon("plus").size(px(11.)).text_color(gpui::white()))
                        .tooltip(|_, cx| crate::ui::tooltip("Add a dissolve on this cut".into(), cx))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.consumed = true;
                            this.store.update(cx, |s, cx| {
                                s.select(id, false, cx);
                                s.run("transition.set", json!({ "clipIds": [id], "kind": "dissolve" }), cx);
                            });
                        }))
                        .into_any_element(),
                );
            }
        }
        out
    }

    /// The length a transition drag sets with the pointer at `time`.
    pub(super) fn transition_len(cut: Option<f64>, start: f64, time: f64) -> f64 {
        match cut {
            Some(c) => 2. * (time - c).abs(),
            None => time - start,
        }
        .max(MIN_LEN)
    }
}

/// Right-click on a transition: every kind (the current one ticked) and removing it.
fn transition_menu(id: Id, position: Point<Pixels>, cx: &mut Context<TimelineBody>) {
    let store = cx.store();
    let s = store.read(cx);
    let Some(p) = s.project.clone() else { return };
    let Some(clip) = p.clip(id) else { return };
    let current = clip.transition.as_ref().map(|t| t.kind);
    let audio = s.track_of(id).is_some_and(|t| t.kind == TrackKind::Audio);
    let mut items = vec![];
    if !audio {
        for k in KINDS {
            let kind = k.id;
            let mut item = MenuItem::new(k.label, move |_, cx| cx.store().update(cx, |s, cx| s.run("transition.set", json!({ "clipIds": [id], "kind": kind }), cx)));
            if current == Some(k.kind) {
                item = item.icon("check");
            }
            items.push(item.entry());
        }
        items.push(MenuEntry::Separator);
    }
    items.push(
        MenuItem::new("Remove transition", move |_, cx| cx.store().update(cx, |s, cx| s.run("transition.remove", json!({ "clipIds": [id] }), cx)))
            .icon("trash")
            .danger()
            .entry(),
    );
    store.update(cx, |s, cx| s.open_menu(position, items, cx));
}
