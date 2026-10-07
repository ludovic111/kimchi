//! Home: the welcome composer (start a project from a prompt, or empty, in a chosen format) and
//! the library of projects. Every change goes through the registry (`project.*`); the list is
//! re-read with `project.list` after the window's own changes and whenever another client (the
//! agent, MCP, the CLI) runs a `project.*` command.

use std::path::PathBuf;

use chrono::{DateTime, Local, Timelike, Utc};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, FontWeight, MouseButton, ObjectFit, Render, ScrollHandle, SharedString, Subscription, Window, actions,
    deferred, div, img, prelude::*, px, svg,
};
use kimchi_core::{Id, ProjectSummary};
use serde_json::json;

use crate::assets::icon_path;
use crate::store::{ComposeRequest, ComposeTarget, Dialog, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, GlassExt, icon};

actions!(home, [ConfirmDelete, CancelDelete]);

/// The kimchi mark (`brand/mark.svg`, written by `scripts/gen-mark.py`): a napa stalk cut square,
/// a sharp leaf and a dithered one, in the text colour.
pub fn mark(size: f32, cx: &App) -> AnyElement {
    svg().path(icon_path("mark")).flex_none().size(px(size)).text_color(cx.theme().text).into_any_element()
}

/// Canvas formats a new project can start with: (aspect, name, width, height).
const FORMATS: [(&str, &str, u32, u32); 5] =
    [("16:9", "Landscape", 1920, 1080), ("9:16", "Vertical", 1080, 1920), ("1:1", "Square", 1080, 1080), ("4:5", "Portrait", 1080, 1350), ("21:9", "Cinema", 2560, 1080)];

/// Column width the project grid aims for, and its gaps.
const CARD_MIN: f32 = 220.;
const GAP_X: f32 = 18.;
const GAP_Y: f32 = 22.;
const MAIN_MAX: f32 = 1080.;

pub struct Home {
    store: Entity<Store>,
    prompt: Entity<TextInput>,
    video: bool,
    format: usize,
    /// `app.info` said ffmpeg is missing (import and export need it).
    ffmpeg_missing: bool,
    /// The project waiting for a delete confirmation: (id, name).
    confirm: Option<(Id, String)>,
    confirm_focus: FocusHandle,
    /// Last command record seen, to notice `project.*` commands from other clients.
    seen_seq: u64,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl Home {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let prompt = cx.new(|cx| {
            let mut i = TextInput::new(cx).multiline(2).placeholder("Describe a shot, a scene, a whole idea — or start from an empty timeline.");
            i.submit_on_enter = true;
            i
        });
        let mut subs = vec![cx.observe(&store, |this: &mut Self, store, cx| {
            // Another client changed the library: read it again.
            let s = store.read(cx);
            let newest = s.commands.last().map(|r| r.seq).unwrap_or(0);
            let touched = s.commands.iter().rev().take_while(|r| r.seq > this.seen_seq).any(|r| r.ok && r.command.starts_with("project."));
            this.seen_seq = this.seen_seq.max(newest);
            if touched {
                this.refresh(cx);
            }
            cx.notify();
        })];
        subs.push(cx.subscribe_in(&prompt, window, |this, _, e: &InputEvent, _, cx| match e {
            InputEvent::Submit => this.create(true, cx),
            InputEvent::Changed(_) => cx.notify(),
            _ => {}
        }));
        cx.bind_keys([gpui::KeyBinding::new("escape", CancelDelete, Some("HomeConfirm")), gpui::KeyBinding::new("enter", ConfirmDelete, Some("HomeConfirm"))]);
        let seen_seq = store.read(cx).commands.last().map(|r| r.seq).unwrap_or(0);
        let mut home = Self {
            store,
            prompt,
            video: true,
            format: 0,
            ffmpeg_missing: false,
            confirm: None,
            confirm_focus: cx.focus_handle(),
            seen_seq,
            scroll: ScrollHandle::new(),
            _subs: subs,
        };
        home.refresh(cx);
        home.check_ffmpeg(cx);
        home
    }

    /// Re-reads the library (`project.list`) into the store.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.run_then("project.list", json!({}), cx, |s, v, cx| {
                if let Ok(list) = serde_json::from_value::<Vec<ProjectSummary>>(v) {
                    s.library = list;
                    cx.notify();
                }
            })
        });
    }

    fn check_ffmpeg(&mut self, cx: &mut Context<Self>) {
        let task = self.store.update(cx, |s, cx| s.call("app.info", json!({}), cx));
        cx.spawn(async move |this, cx| {
            if let Ok(info) = task.await {
                this.update(cx, |h, cx| {
                    h.ffmpeg_missing = info["ffmpeg"].is_null();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Creates a project in the chosen format; with a prompt, it is named after the prompt and
    /// the prompt goes to the generate composer, aimed at the start of the timeline.
    fn create(&mut self, with_prompt: bool, cx: &mut Context<Self>) {
        let prompt = self.prompt.read(cx).text().trim().to_string();
        if with_prompt && prompt.is_empty() {
            return;
        }
        let (aspect, _, w, h) = FORMATS[self.format];
        let name = if with_prompt { title_from(&prompt) } else { "Untitled".to_string() };
        let video = self.video;
        let params = json!({ "name": name, "width": w, "height": h, "fps": 30, "background": "#000000" });
        let prompt_input = self.prompt.clone();
        self.store.update(cx, |s, cx| {
            s.run_then("project.create", params, cx, move |s, _, cx| {
                if with_prompt {
                    prompt_input.update(cx, |i, cx| i.set_text("", cx));
                    s.compose(
                        ComposeRequest {
                            video,
                            prompt: Some(prompt),
                            aspect: Some(aspect.to_string()),
                            target: Some(ComposeTarget { track_id: None, start: 0.0, duration: 5.0, label: "Start of the timeline".into() }),
                            ..Default::default()
                        },
                        cx,
                    );
                }
            })
        });
    }

    fn open(id: Id, cx: &mut App) {
        cx.store().update(cx, |s, cx| s.run("project.open", json!({ "projectId": id }), cx));
    }

    fn duplicate(this: &gpui::WeakEntity<Self>, id: Id, cx: &mut App) {
        let this = this.clone();
        cx.store().update(cx, |s, cx| {
            s.run_then("project.duplicate", json!({ "projectId": id }), cx, move |s, v, cx| {
                s.info(format!("Duplicated as “{}”", v["name"].as_str().unwrap_or("copy")), cx);
                this.update(cx, |h, cx| h.refresh(cx)).ok();
            })
        });
    }

    fn ask_delete(&mut self, id: Id, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = Some((id, name));
        window.focus(&self.confirm_focus, cx);
        cx.notify();
    }

    fn confirm_delete(&mut self, _: &ConfirmDelete, _: &mut Window, cx: &mut Context<Self>) {
        let Some((id, name)) = self.confirm.take() else { return };
        let this = cx.entity().downgrade();
        self.store.update(cx, |s, cx| {
            s.run_then("project.delete", json!({ "projectId": id }), cx, move |s, _, cx| {
                s.info(format!("Deleted “{name}”"), cx);
                this.update(cx, |h, cx| h.refresh(cx)).ok();
            })
        });
        cx.notify();
    }

    fn cancel_delete(&mut self, _: &CancelDelete, _: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        cx.notify();
    }

    fn project_menu(&self, p: &ProjectSummary, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let (id, name) = (p.id, p.name.clone());
        let this_dup = this.clone();
        let entries = vec![
            MenuItem::new("Open", move |_, cx| Self::open(id, cx)).icon("folder-open").entry(),
            MenuItem::new("Duplicate", move |_, cx| Self::duplicate(&this_dup, id, cx)).icon("copy").entry(),
            crate::store::MenuEntry::Separator,
            MenuItem::new("Delete project…", move |window, cx| {
                let name = name.clone();
                this.update(cx, |h, cx| h.ask_delete(id, name, window, cx)).ok();
            })
            .icon("trash")
            .danger()
            .entry(),
        ];
        self.store.update(cx, |s, cx| s.open_menu(position, entries, cx));
    }

    // ---- pieces ---------------------------------------------------------------

    fn header(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let s = self.store.read(cx);
        let connected = s.providers.iter().filter(|p| p.ready).count();
        let connected_logos: Vec<String> = s.providers.iter().filter(|p| p.ready && crate::ui::logos::logo_file(&p.info.id).is_some()).take(4).map(|p| p.info.id.clone()).collect();
        let update = s.update.available.clone();
        let fullscreen = window.is_fullscreen();
        let controls = crate::ui::window_controls(window, cx);
        let compact = crate::ui::layout::breakpoint(f32::from(window.viewport_size().width)) == crate::ui::layout::Breakpoint::Compact;
        div()
            .id("home-top")
            .h(px(crate::views::editor::TOPBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            // Room for the macOS traffic lights.
            .pl(px(if fullscreen || !cfg!(target_os = "macos") { 18. } else { 92. }))
            .when(controls.is_none(), |d| d.pr(px(14.)))
            .window_control_area(gpui::WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(div().flex().items_center().gap(px(8.)).child(mark(22., cx)).child(div().text_size(px(sz::MD)).font_weight(FontWeight::BOLD).child("kimchi")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when_some(update, |d, v| {
                        d.child(
                            Button::new("home-update", format!("kimchi {v} is available"))
                                .small()
                                .with_icon("download")
                                .color(t.accent_text)
                                .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("updates".into()) }, cx))),
                        )
                    })
                    .child(if compact {
                        Button::icon("home-support", "heart", "Support kimchi")
                    } else {
                        Button::new("home-support", "Support").small().ghost().with_icon("heart")
                    }
                    .on_click(|_, _, cx| cx.open_url(crate::app::SUPPORT_URL)))
                    .child(
                        div()
                            .id("home-keys")
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .h(px(28.))
                            .px(px(11.))
                            .glass(t.glass1)
                            .text_size(px(sz::SM))
                            .text_color(t.text_2)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text).bg(t.hover))
                            .role(gpui::Role::Button)
                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("models".into()) }, cx)))
                            .child(icon("key-round").text_color(if connected > 0 { t.success } else { t.text_2 }))
                            .children(connected_logos.iter().map(|id| crate::ui::logo(id, px(14.))))
                            .child(if connected > 0 && compact {
                                format!("{connected} connected")
                            } else if connected > 0 {
                                format!("{connected} model provider{} connected", if connected == 1 { "" } else { "s" })
                            } else {
                                "Connect a model provider".into()
                            }),
                    )
                    .child(Button::icon("home-palette", "command", crate::actions::tip("Command palette", &crate::actions::Palette)).on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Palette, cx))))
                    .child(
                        Button::icon("home-settings", "settings", crate::actions::tip("Settings", &crate::actions::OpenSettings))
                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: None }, cx))),
                    ),
            )
            .children(controls)
            .into_any_element()
    }

    fn chip(&self, id: impl Into<gpui::ElementId>, selected: bool, cx: &App) -> gpui::Stateful<gpui::Div> {
        let t = cx.theme().clone();
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .px(px(9.))
            .text_size(px(sz::SM))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .role(gpui::Role::RadioButton)
            .when(selected, |d| d.bg(t.accent).text_color(t.text_on_accent).border_1().border_color(t.accent))
            .when(!selected, |d| d.text_color(t.text_2).border_1().border_color(gpui::transparent_black()).hover(|s| s.bg(t.hover).text_color(t.text)))
    }

    fn composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let empty = self.prompt.read(cx).text().trim().is_empty();
        let mode_chip = |this: &Self, video: bool, cx: &mut Context<Self>| {
            this.chip(if video { "mode-video" } else { "mode-image" }, this.video == video, cx)
                .aria_label(if video { "Video" } else { "Image" })
                .child(icon(if video { "film" } else { "image" }).text_color(if this.video == video { cx.theme().text_on_accent } else { cx.theme().text_2 }))
                .child(if video { "Video" } else { "Image" })
                .on_click(cx.listener(move |h, _, _, cx| {
                    h.video = video;
                    cx.notify();
                }))
        };
        let formats = FORMATS.iter().enumerate().map(|(i, (aspect, label, w, h))| {
            let on_click = cx.listener(move |h: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                h.format = i;
                cx.notify();
            });
            let tip: SharedString = format!("{label} · {w}×{h}").into();
            let rw = (10. * *w as f32 / *h as f32).min(18.);
            self.chip(("format", i), self.format == i, cx)
                .font_family(MONO)
                .text_size(px(sz::XS))
                .aria_label(tip.clone())
                .tooltip(move |_, cx| crate::ui::tooltip(tip.clone(), cx))
                .child(div().w(px(rw)).h(px(10.)).border_1().border_color(if self.format == i { t.text_on_accent } else { t.text_2 }))
                .child(*aspect)
                .on_click(on_click)
        });
        let formats: Vec<_> = formats.collect();
        let panel = div()
            .w_full()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .text_size(px(sz::MD))
            .child(self.prompt.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(4.))
                            .child(mode_chip(self, true, cx))
                            .child(mode_chip(self, false, cx))
                            .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(t.line_strong))
                            .children(formats),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(8.))
                            .child(Button::new("empty-project", "Empty project").small().ghost().on_click(cx.listener(|h, _, _, cx| h.create(false, cx))))
                            .child(
                                div()
                                    .id("create-generate")
                                    .size(px(34.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(t.accent)
                                    .text_color(t.text_on_accent)
                                    .role(gpui::Role::Button)
                                    .aria_label("Create the project and generate")
                                    .tooltip(|_, cx| crate::ui::tooltip("Create and generate (Enter)".into(), cx))
                                    .when(empty, |d| d.opacity(0.3).cursor_not_allowed())
                                    .when(!empty, |d| d.cursor_pointer().hover(|s| s.bg(t.accent_hover)).on_click(cx.listener(|h, _, _, cx| h.create(true, cx))))
                                    .child(icon("arrow-up").size(px(16.)).text_color(t.text_on_accent)),
                            ),
                    ),
            );
        // Framed like a viewfinder.
        div().relative().w_full().child(crate::ui::grain::brackets(14., -9., t.text_3)).child(panel).into_any_element()
    }

    fn card(&self, i: usize, p: &ProjectSummary, width: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let id = p.id;
        let cover_h = width * 10. / 16.;
        let menu_p = p.clone();
        let menu_p2 = p.clone();
        let pill = |c| div().absolute().bottom(px(8.)).px(px(7.)).py(px(2.)).text_size(px(10.5)).bg(gpui::black().opacity(0.72)).text_color(c);
        div()
            .id(gpui::ElementId::from(SharedString::from(format!("project-{id}"))))
            .group("project-card")
            .w(px(width))
            .flex()
            .flex_col()
            .gap(px(10.))
            .cursor_pointer()
            .role(gpui::Role::Button)
            .aria_label(format!("Open {}", p.name))
            .on_click(move |_, _, cx| Self::open(id, cx))
            .on_mouse_down(MouseButton::Right, cx.listener(move |h, e: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                h.project_menu(&menu_p, e.position, cx)
            }))
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .h(px(cover_h))
                    .rounded(px(sz::R_LG))
                    .overflow_hidden()
                    .bg(t.bg_sunken)
                    .border_1()
                    .border_color(t.line)
                    .group_hover("project-card", |s| s.border_color(t.accent_ring))
                    .child(match &p.cover {
                        Some(path) => img(PathBuf::from(path)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                        // No picture yet: a dithered fade and the mark.
                        None => div()
                            .relative()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().absolute().top_0().left_0().child(crate::ui::grain::dither(480., 300., 0.22, window, cx)))
                            .child(div().opacity(0.4).child(mark(28., cx)))
                            .into_any_element(),
                    })
                    .when(p.generated_count > 0, |d| {
                        d.child(
                            pill(gpui::white())
                                .left(px(8.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(icon("sparkles").size(px(11.)).text_color(gpui::white()))
                                .child(p.generated_count.to_string()),
                        )
                    })
                    .when(p.duration > 0.0, |d| d.child(pill(crate::theme::grey(0.85)).right(px(8.)).font_family(MONO).child(short(p.duration))))
                    .child(
                        div()
                            .id(("card-menu", i))
                            .absolute()
                            .top(px(8.))
                            .right(px(8.))
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(gpui::black().opacity(0.72))
                            .text_color(gpui::white())
                            .opacity(0.)
                            .group_hover("project-card", |s| s.opacity(1.))
                            .role(gpui::Role::Button)
                            .aria_label("Project actions")
                            .tooltip(|_, cx| crate::ui::tooltip("More".into(), cx))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |h, e: &gpui::ClickEvent, _, cx| {
                                cx.stop_propagation();
                                h.project_menu(&menu_p2, e.position(), cx)
                            }))
                            .child(icon("ellipsis").text_color(gpui::white())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(2.))
                    .child(div().text_size(px(sz::BASE)).font_weight(FontWeight::SEMIBOLD).truncate().child(p.name.clone()))
                    .child(div().text_size(px(sz::XS)).text_color(t.text_2).child(format!("{} · {}×{}", ago(p.updated_at), p.width, p.height))),
            )
            .into_any_element()
    }

    fn projects(&self, main_w: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let list = self.store.read(cx).library.clone();
        let cols = (((main_w + GAP_X) / (CARD_MIN + GAP_X)).floor() as usize).max(1);
        let card_w = ((main_w - GAP_X * (cols as f32 - 1.)) / cols as f32).floor();
        let mut cards = Vec::with_capacity(list.len());
        for (i, p) in list.iter().enumerate() {
            cards.push(self.card(i, p, card_w, window, cx));
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(14.))
                    .child(div().text_size(px(sz::MD)).font_weight(FontWeight::SEMIBOLD).child("Projects"))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            // The editors whose projects it opens, by their own logos.
                            .child(div().flex().items_center().gap(px(3.)).mr(px(2.)).children(crate::views::dialogs::interop::import_apps().into_iter().map(|(id, name)| {
                                div().id(SharedString::from(format!("from-{id}"))).tooltip(move |_, cx| crate::ui::tooltip(format!("Opens {name} projects").into(), cx)).child(crate::ui::logo(id, px(16.)))
                            })))
                            .child(Button::new("open-other", "Open from another editor").small().ghost().with_icon("folder-open").on_click(|_, _, cx| crate::views::dialogs::interop::open_from_other(cx)))
                            .child(Button::new("new-project", "New").small().ghost().with_icon("plus").on_click(cx.listener(|h, _, _, cx| h.create(false, cx)))),
                    ),
            )
            .when(list.is_empty(), |d| {
                d.child(
                    div()
                        .relative()
                        .overflow_hidden()
                        .py(px(28.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .border_1()
                        .border_dashed()
                        .border_color(t.line_strong)
                        .text_color(t.text_2)
                        .child(div().absolute().top_0().left_0().child(crate::ui::grain::dither(MAIN_MAX, 120., 0.16, window, cx)))
                        .child(icon("clapperboard").size(px(20.)).text_color(t.text_2))
                        .child("Nothing here yet. Your projects will show up here."),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(GAP_X))
                    .gap_y(px(GAP_Y))
                    .children(cards),
            )
            .into_any_element()
    }

    fn confirm_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, name) = self.confirm.clone()?;
        let t = cx.theme().clone();
        Some(
            deferred(crate::ui::motion::fade(
                div()
                    .id("confirm-scrim")
                    .key_context("HomeConfirm")
                    .track_focus(&self.confirm_focus)
                    .on_action(cx.listener(Self::confirm_delete))
                    .on_action(cx.listener(Self::cancel_delete))
                    .occlude()
                    .absolute()
                    .inset_0()
                    .bg(t.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, cx.listener(|h, _, _, cx| {
                        h.confirm = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id("confirm-delete")
                            .role(gpui::Role::AlertDialog)
                            .aria_label("Delete project")
                            .occlude()
                            .w(px(420.))
                            .p(px(20.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .rounded(px(sz::R_LG))
                            .glass(t.glass3)
                            .shadow(t.glass_shadow())
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(div().text_size(px(sz::LG)).font_weight(FontWeight::SEMIBOLD).child(format!("Delete “{name}”?")))
                            .child(div().text_color(t.text_2).child("Media generated inside it is deleted too. This can't be undone."))
                            .child(
                                div()
                                    .mt(px(8.))
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.))
                                    .child(Button::new("confirm-cancel", "Cancel").ghost().on_click(cx.listener(|h, _, w, cx| h.cancel_delete(&CancelDelete, w, cx))))
                                    .child(Button::new("confirm-yes", "Delete").danger().with_icon("trash").on_click(cx.listener(|h, _, w, cx| h.confirm_delete(&ConfirmDelete, w, cx)))),
                            ),
                    ),
                "confirm-in",
                crate::ui::motion::FAST,
            ))
            .with_priority(1)
            .into_any_element(),
        )
    }
}

impl Render for Home {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let none_ready = {
            let s = self.store.read(cx);
            !s.providers.is_empty() && !s.providers.iter().any(|p| p.ready)
        };
        let viewport = f32::from(window.viewport_size().width);
        let main_w = (viewport - 64.).clamp(240., MAIN_MAX);
        let hour = Local::now().hour();
        let greeting = match hour {
            0..5 => "Up late",
            5..12 => "Good morning",
            12..18 => "Good afternoon",
            _ => "Good evening",
        };
        let header = self.header(window, cx);
        let composer = self.composer(cx);
        let projects = self.projects(main_w, window, cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div().id("home-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).child(
                    div()
                        .w(px(main_w))
                        .mx_auto()
                        .pt(px(48.))
                        .pb(px(64.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(760.))
                                .mb(px(64.))
                                .flex()
                                .flex_col()
                                .items_center()
                                // A pixel, the greeting and the hour, like a slate.
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .font_family(MONO)
                                        .text_size(px(sz::XS))
                                        .text_color(t.text_2)
                                        .child(div().size(px(6.)).bg(t.text))
                                        .child(greeting.to_uppercase())
                                        .child(div().text_color(t.text_3).child(Local::now().format("// %H:%M").to_string())),
                                )
                                .child(
                                    div()
                                        .mt(px(10.))
                                        .mb(px(28.))
                                        .flex()
                                        .flex_wrap()
                                        .justify_center()
                                        .gap(px(12.))
                                        .text_size(px(if viewport < 760. { sz::XXL + 6. } else { 52. }))
                                        .line_height(gpui::relative(1.05))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("What are we")
                                        // The word in negative: a block of ink.
                                        .child(div().px(px(10.)).bg(t.accent).text_color(t.text_on_accent).child("making"))
                                        .child("today?"),
                                )
                                .child(composer)
                                .when(none_ready, |d| {
                                    d.child(
                                        div()
                                            .id("nudge")
                                            .mt(px(16.))
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .px(px(10.))
                                            .py(px(6.))
                                            .rounded(px(sz::R_MD))
                                            .text_size(px(sz::SM))
                                            .text_color(t.text_2)
                                            .cursor_pointer()
                                            .hover(|s| s.text_color(t.text).bg(t.hover))
                                            .role(gpui::Role::Button)
                                            .on_click(|_, _, cx| cx.store().update(cx, |s, cx| s.open_dialog(Dialog::Settings { section: Some("models".into()) }, cx)))
                                            .child(icon("sparkles").text_color(t.accent_text))
                                            .child("Bring your own key — OpenRouter, fal, Replicate, OpenAI, Google… — or point kimchi at ComfyUI on your machine."),
                                    )
                                }),
                        )
                        .child(projects),
                ),
            )
            .when(self.ffmpeg_missing, |d| {
                d.child(
                    div().absolute().bottom(px(18.)).left_0().right_0().flex().justify_center().child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(14.))
                            .py(px(8.))
                            .glass(t.glass2)
                            .text_size(px(sz::SM))
                            .text_color(t.warning)
                            .child(icon("circle-alert").text_color(t.warning))
                            .child("ffmpeg wasn't found — import and export need it. Install it with")
                            .child(div().font_family(MONO).child("brew install ffmpeg")),
                    ),
                )
            })
            .children(self.confirm_overlay(cx))
    }
}

/// A project name from the first words of a prompt.
pub fn title_from(p: &str) -> String {
    let words = p.split_whitespace().take(5).collect::<Vec<_>>().join(" ");
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "Untitled".into(),
    }
}

/// Compact duration: `4.2s`, `12s`, `1:05`.
pub fn short(t: f64) -> String {
    crate::views::timeline::geom::short(t)
}

/// "just now", "5 min ago", "3 h ago", "2 d ago", else the date.
pub fn ago(at: DateTime<Utc>) -> String {
    let s = (Utc::now() - at).num_seconds();
    match s {
        ..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        86400..604800 => format!("{} d ago", s / 86400),
        _ => at.with_timezone(&Local).format("%b %-d").to_string(),
    }
}
