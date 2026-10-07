//! Other editors' projects in the window: Home's and the menu's "Open from another editor…",
//! "Export for <app>…" in the editor's menu, the sheet that says what made the trip, and
//! "Find missing files…". Everything runs `project.importFrom`, `project.exportTo` and
//! `media.relink`.

use gpui::{AnyElement, App, FontWeight, PathPromptOptions, SharedString, div, prelude::*, px};
use kimchi_control::ToastKind;
use kimchi_interop::apps::APPS;
use kimchi_interop::timeline::{FORMATS, Support};
use serde_json::{Value, json};

use crate::store::{Dialog, StoreExt};
use crate::theme::{ActiveTheme, size as sz};
use crate::ui::{Button, icon};

/// Apps kimchi writes a project for, with the format each gets: `(app id, app name, format id)`.
pub fn export_targets() -> Vec<(&'static str, &'static str, &'static str)> {
    APPS.iter()
        .filter_map(|a| {
            let f = a.writes.iter().find(|w| FORMATS.iter().any(|f| f.id == **w && f.export != Support::No))?;
            Some((a.id, a.name, *f))
        })
        .collect()
}

/// Editors whose projects kimchi opens (a format it reads), with a logo: `(app id, name)`.
pub fn import_apps() -> Vec<(&'static str, &'static str)> {
    APPS.iter()
        .filter(|a| a.opens.iter().any(|o| FORMATS.iter().any(|f| f.id == *o && f.import != Support::No)))
        .filter(|a| crate::ui::logos::logo_file(a.id).is_some())
        .map(|a| (a.id, a.name))
        .collect()
}

/// The file name extension a format is written with.
pub fn extension(format: &str) -> &'static str {
    match format {
        "xmeml" => "xml",
        other => FORMATS.iter().find(|f| f.id == other).map_or("xml", |f| f.extensions[0]),
    }
}

/// Picks another editor's project (a file, or a bundle or draft folder) and opens it as a new
/// project.
pub fn open_from_other(cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: true, multiple: false, prompt: Some("Open".into()) });
    let store = cx.store();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else { return };
        let Some(path) = paths.into_iter().next() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        store.update(cx, |s, cx| {
            s.run_then("project.importFrom", json!({ "path": path.to_string_lossy() }), cx, move |s, v, cx| {
                s.open_dialog(Dialog::Interop { title: format!("Opened {name}"), report: v["report"].clone() }, cx);
            })
        });
    })
    .detach();
}

/// Asks where to write the open project for `app` and writes it there.
pub fn export_for(app: &'static str, app_name: &'static str, format: &'static str, cx: &mut App) {
    let name = cx.store().read(cx).project.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Project".into());
    let dir = crate::views::dialogs::export::default_dir();
    let rx = cx.prompt_for_new_path(&dir, Some(&format!("{name}.{}", extension(format))));
    let store = cx.store();
    cx.spawn(async move |cx| {
        let Some(path) = rx.await.ok().and_then(Result::ok).flatten() else { return };
        store.update(cx, |s, cx| {
            s.toast(ToastKind::Info, format!("Writing the project for {app_name}…"), cx);
            s.run_then("project.exportTo", json!({ "path": path.to_string_lossy(), "app": app, "format": format }), cx, move |s, v, cx| {
                s.open_dialog(Dialog::Interop { title: format!("Written for {app_name}"), report: v["report"].clone() }, cx);
            })
        });
    })
    .detach();
}

/// Picks a folder and points every missing media file at the file of the same name in it.
pub fn find_missing(cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Search here".into()) });
    let store = cx.store();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else { return };
        let Some(dir) = paths.into_iter().next() else { return };
        store.update(cx, |s, cx| {
            s.run_then("media.relink", json!({ "folder": dir.to_string_lossy() }), cx, |s, v, cx| {
                let n = v["relinked"].as_u64().unwrap_or(0);
                let left = v["missing"].as_array().map_or(0, Vec::len);
                let text = match (n, left) {
                    (0, _) => "No missing files were found there.".to_string(),
                    (n, 0) => format!("Found {n} file{}: nothing is missing now.", if n == 1 { "" } else { "s" }),
                    (n, l) => format!("Found {n} file{}; {l} still missing.", if n == 1 { "" } else { "s" }),
                };
                s.toast(if n > 0 { ToastKind::Success } else { ToastKind::Info }, text, cx);
                if s.dialog.as_ref().is_some_and(|d| matches!(d, Dialog::Interop { .. })) && left == 0 {
                    s.close_dialog(cx);
                }
            })
        });
    })
    .detach();
}

fn list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn section(title: &'static str, glyph: &'static str, items: &[String], color: gpui::Hsla, cx: &App) -> Option<AnyElement> {
    if items.is_empty() {
        return None;
    }
    let t = cx.theme().clone();
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().flex().items_center().gap(px(6.)).text_size(px(sz::SM)).font_weight(FontWeight::SEMIBOLD).child(icon(glyph).text_color(color)).child(title))
            .children(items.iter().map(|s| div().pl(px(22.)).text_size(px(sz::SM)).text_color(t.text_2).child(SharedString::from(s.clone()))))
            .into_any_element(),
    )
}

/// What came through a trip to or from another editor, in plain words.
pub fn sheet(title: String, report: Value, cx: &App) -> AnyElement {
    let t = cx.theme().clone();
    let (kept, approx, dropped, missing) = (list(&report["kept"]), list(&report["approximated"]), list(&report["dropped"]), list(&report["missingMedia"]));
    let exact = approx.is_empty() && dropped.is_empty() && missing.is_empty();
    let subtitle = if exact { "Everything came through as it was." } else { "Most of it came through; here is what changed." };
    let body: Vec<AnyElement> = [
        section("Came through", "check", &kept, t.success, cx),
        section("Changed on the way", "shuffle", &approx, t.warning, cx),
        section("Left out", "minus", &dropped, t.text_2, cx),
        section("Missing files", "circle-alert", &missing, t.danger, cx),
    ]
    .into_iter()
    .flatten()
    .collect();
    let has_missing = !missing.is_empty();
    div()
        .id("interop-report")
        .role(gpui::Role::Dialog)
        .aria_label("What came through")
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
                .child(Button::icon("interop-close", "x", "Close (Esc)").on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))),
        )
        .child(div().id("interop-body").flex_1().min_h_0().max_h(px(520.)).overflow_y_scroll().px(px(20.)).py(px(16.)).flex().flex_col().gap(px(16.)).children(body))
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .px(px(20.))
                .py(px(12.))
                .border_t_1()
                .border_color(t.line)
                .when(has_missing, |d| d.child(Button::new("interop-find", "Find missing files…").small().ghost().with_icon("folder-search").on_click(|_, _, cx| find_missing(cx))))
                .child(Button::new("interop-ok", "Continue").primary().on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.close_dialog(cx)))),
        )
        .into_any_element()
}
