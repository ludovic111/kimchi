//! What's new: the release notes built into this copy (`CHANGELOG.md`, the same text
//! `app.whatsNew` returns). Opens by itself once after an update (with every release since the
//! version that ran before), and from Settings › Updates, the palette and the Help menu.

use gpui::{AnyElement, App, FontWeight, div, prelude::*, px};
use kimchi_control::release_notes::{self, Release};

use crate::store::{Dialog, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::{Button, markdown};

pub const CHANGELOG_URL: &str = "https://github.com/ludovic111/kimchi/blob/main/CHANGELOG.md";

/// The releases to show: since `since` (after an update), else this version; `all` adds every
/// earlier one.
fn releases(since: Option<&str>, all: bool) -> Vec<Release> {
    if all {
        return release_notes::all().into_iter().filter(|r| !kimchi_control::update::is_newer(&r.version, kimchi_control::update::CURRENT)).collect();
    }
    let mut r = since.map(release_notes::since).unwrap_or_default();
    if r.is_empty() {
        r = release_notes::find(kimchi_control::update::CURRENT).into_iter().collect();
    }
    r
}

fn release(r: &Release, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(div().text_size(px(sz::LG)).font_weight(FontWeight::SEMIBOLD).child(format!("kimchi {}", r.version)))
                .when_some(r.date.clone(), |d, date| d.child(div().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_3).child(date))),
        )
        .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(markdown::render(&r.notes, cx)))
        .into_any_element()
}

pub fn sheet(since: Option<String>, all: bool, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let shown = releases(since.as_deref(), all);
    let updated = since.is_some() && !all;
    let title = if updated { format!("kimchi is now {}", kimchi_control::update::CURRENT) } else { "What's new".to_string() };
    let subtitle = if updated { "Here is what changed since you last opened it." } else { "What changed in each version of kimchi." };
    let mut body: Vec<AnyElement> = vec![];
    for (i, r) in shown.iter().enumerate() {
        if i > 0 {
            body.push(div().h(px(1.)).bg(t.line).into_any_element());
        }
        body.push(release(r, cx));
    }
    if shown.is_empty() {
        body.push(div().text_color(t.text_2).child("No release notes are built into this copy.").into_any_element());
    }
    div()
        .id("whats-new")
        .role(gpui::Role::Dialog)
        .aria_label("What's new")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(20.))
                .pt(px(18.))
                .pb(px(14.))
                .border_b_1()
                .border_color(t.line)
                .child(crate::views::home::mark(36., cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_size(px(sz::LG)).font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(div().text_size(px(sz::SM)).text_color(t.text_2).child(subtitle)),
                )
                .child(Button::icon("whats-new-close", "x", "Close (Esc)").on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))),
        )
        .child(div().id("whats-new-body").flex_1().min_h_0().max_h(px(560.)).overflow_y_scroll().px(px(20.)).py(px(16.)).flex().flex_col().gap(px(18.)).children(body))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .px(px(20.))
                .py(px(12.))
                .border_t_1()
                .border_color(t.line)
                .child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .when(!all, |d| {
                            d.child(Button::new("whats-new-all", "Earlier versions").small().ghost().with_icon("history").on_click(move |_, _, cx| {
                                let since = since.clone();
                                cx.store().update(cx, |s, cx| s.open_dialog(Dialog::WhatsNew { since, all: true }, cx))
                            }))
                        })
                        // People who used kimchi before the setup existed are pointed at it once here.
                        .when(cx.store().read(cx).settings.onboarding.coming_from.is_empty(), |d| {
                            d.child(Button::new("whats-new-setup", "Set up for your editor").small().ghost().with_icon("sparkles").on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_setup(None, cx))))
                        })
                        .child(Button::new("whats-new-web", "Changelog").small().ghost().icon_after("arrow-up-right").on_click(|_, _, cx| cx.open_url(CHANGELOG_URL))),
                )
                .child(Button::new("whats-new-ok", "Continue").primary().on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))),
        )
        .into_any_element()
}
