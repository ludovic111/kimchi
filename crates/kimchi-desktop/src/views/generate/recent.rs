//! Recent generations under the composer: jobs still running, then the
//! generated media of the project, newest first. Click selects, double-click
//! puts it at the playhead, right-click offers to reuse or animate it.

use std::time::Duration;

use gpui::{Animation, AnimationExt, AnyElement, Context, FontWeight, MouseButton, ObjectFit, div, img, prelude::*, px, relative};
use kimchi_core::{Asset, AssetOrigin, MediaKind};
use serde_json::json;

use crate::store::{ComposeRef, ComposeRequest, MenuItem, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::icon;
use crate::views::generate::{eyebrow, image_path};
use crate::views::generate_panel::GeneratePanel;

const CARD: f32 = 92.;

/// The context menu of a generated asset.
pub fn asset_menu(a: &Asset) -> Vec<crate::store::MenuEntry> {
    let id = a.id;
    let mut entries = vec![
        MenuItem::new("Insert at playhead", move |_, cx| cx.store().update(cx, |s, cx| s.run("clip.insertMedia", json!({ "assetId": id.to_string() }), cx)))
            .icon("plus")
            .entry(),
    ];
    if let AssetOrigin::Generated(g) = &a.origin {
        let req = ComposeRequest {
            video: g.task.contains("video"),
            prompt: Some(g.prompt.clone()),
            negative: Some(g.negative_prompt.clone().unwrap_or_default()),
            model: Some(format!("{}::{}", g.provider, g.model)),
            seed: Some(String::new()),
            ..Default::default()
        };
        entries.push(MenuItem::new("Reuse prompt", move |_, cx| cx.store().update(cx, |s, cx| s.compose(req.clone(), cx))).icon("rotate-ccw").ai().entry());
    }
    if a.kind == MediaKind::Image {
        let r = ComposeRef { role: "start_frame", path: a.path.clone(), label: a.name.clone(), asset_id: Some(a.id) };
        let animate = ComposeRequest { video: true, refs: vec![r.clone()], ..Default::default() };
        let edit = ComposeRequest { video: false, refs: vec![ComposeRef { role: "reference", ..r }], ..Default::default() };
        entries.push(MenuItem::new("Animate", move |_, cx| cx.store().update(cx, |s, cx| s.compose(animate.clone(), cx))).icon("clapperboard").ai().entry());
        entries.push(MenuItem::new("Edit", move |_, cx| cx.store().update(cx, |s, cx| s.compose(edit.clone(), cx))).icon("wand-sparkles").ai().entry());
    }
    entries
}

impl GeneratePanel {
    pub(crate) fn recent(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let running: Vec<kimchi_gen::Job> = s.jobs.iter().filter(|j| !j.status.is_done()).cloned().collect();
        let generated: Vec<Asset> =
            s.project.as_ref().map(|p| p.assets.iter().rev().filter(|a| matches!(a.origin, AssetOrigin::Generated(_))).take(24).cloned().collect()).unwrap_or_default();
        let selected = s.selected_asset;
        if running.is_empty() && generated.is_empty() {
            return None;
        }
        let pending = running.into_iter().map(|j| {
            let id = j.id.clone();
            let message = j.progress.message.clone().unwrap_or_else(|| if j.status == kimchi_gen::JobStatus::Queued { "Queued".into() } else { "Working".into() });
            let shimmer = div().absolute().inset_0().bg(t.accent_soft).with_animation(
                gpui::ElementId::Name(format!("shimmer-{}", j.id).into()),
                Animation::new(Duration::from_millis(1600)).repeat(),
                |d, delta| d.opacity(0.55 + 0.45 * (delta * std::f32::consts::TAU).sin().abs()),
            );
            div()
                .id(gpui::ElementId::Name(format!("pending-{}", j.id).into()))
                .relative()
                .size(px(CARD))
                .rounded(px(sz::R_MD))
                .overflow_hidden()
                .bg(t.bg_sunken)
                .border_1()
                .border_color(t.accent_ring)
                .flex()
                .items_end()
                .child(shimmer)
                .child(
                    div()
                        .relative()
                        .p(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .min_w_0()
                        .child(div().text_size(px(10.5)).line_height(px(13.)).line_clamp(2).child(if j.request.prompt.is_empty() { "Untitled".to_string() } else { j.request.prompt.clone() }))
                        .child(div().text_size(px(9.5)).text_color(t.text_2).truncate().child(message)),
                )
                .when_some(j.progress.fraction, |d, f| {
                    d.child(div().absolute().left_0().right_0().bottom_0().h(px(2.)).bg(t.line).child(div().h_full().w(relative(f as f32)).bg(t.accent)))
                })
                .child(
                    div()
                        .id(gpui::ElementId::Name(format!("pending-stop-{}", j.id).into()))
                        .absolute()
                        .top(px(5.))
                        .right(px(5.))
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.55))
                        .text_color(gpui::white())
                        .cursor_pointer()
                        .tooltip(|_, cx| crate::ui::tooltip("Cancel".into(), cx))
                        .on_click(move |_, _, cx| cx.store().update(cx, |s, cx| s.run("generate.cancel", json!({ "jobId": id }), cx)))
                        .child(icon("square").size(px(9.))),
                )
                .into_any_element()
        });
        let cards = generated.into_iter().map(|a| {
            let id = a.id;
            let on = selected == Some(id);
            let source = match a.kind {
                MediaKind::Image => Some(a.path.clone()),
                _ => a.thumbnail.clone(),
            };
            let tip: gpui::SharedString = match &a.origin {
                AssetOrigin::Generated(g) => g.prompt.clone(),
                _ => a.name.clone(),
            }
            .into();
            let menu = asset_menu(&a);
            div()
                .id(id)
                .relative()
                .size(px(CARD))
                .rounded(px(sz::R_MD))
                .overflow_hidden()
                .bg(t.bg_sunken)
                .border_1()
                .border_color(t.line)
                .cursor_pointer()
                .when(on, |d| d.border_2().border_color(t.accent))
                .hover(|s| s.border_color(t.line_strong))
                .tooltip(move |_, cx| crate::ui::tooltip(tip.clone(), cx))
                .on_click(move |e, _, cx| {
                    if e.click_count() >= 2 {
                        cx.store().update(cx, |s, cx| s.run("clip.insertMedia", json!({ "assetId": id.to_string() }), cx));
                    } else {
                        cx.store().update(cx, |s, cx| s.select_asset(Some(id), cx));
                    }
                })
                .on_mouse_down(MouseButton::Right, move |e, _, cx| {
                    let pos = e.position;
                    let menu = menu.clone();
                    cx.stop_propagation();
                    cx.store().update(cx, |s, cx| {
                        s.select_asset(Some(id), cx);
                        s.open_menu(pos, menu, cx);
                    });
                })
                .child(match source {
                    Some(p) => img(image_path(&p)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                    None => div().size_full().flex().items_center().justify_center().text_color(t.text_2).child(icon("film").size(px(18.))).into_any_element(),
                })
                .when(a.kind == MediaKind::Video, |d| {
                    d.child(
                        div()
                            .absolute()
                            .right(px(5.))
                            .bottom(px(4.))
                            .px(px(4.))
                            .rounded(px(sz::R_XS))
                            .bg(gpui::black().opacity(0.6))
                            .text_color(gpui::white())
                            .text_size(px(9.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("▶"),
                    )
                })
                .into_any_element()
        });
        Some(
            div()
                .mt(px(22.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(eyebrow("Recent", cx))
                .child(div().flex().flex_wrap().gap(px(6.)).children(pending).children(cards))
                .into_any_element(),
        )
    }
}
