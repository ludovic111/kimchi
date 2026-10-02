//! The media panel (left column): the project's imported and generated files.
//!
//! Click selects (the inspector shows it), double-click puts it on the timeline at the
//! playhead, dragging carries it onto a track (`MediaDrag`, dropped by the timeline), and the
//! context menu has the rest: insert, the AI actions, reveal, remove (asked first).

#[path = "media/confirm.rs"]
mod confirm;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FocusHandle, FontWeight, MouseButton, MouseDownEvent, ObjectFit, Pixels, Point, Render,
    SharedString, Stateful, Subscription, Window, canvas, div, img, prelude::*, px,
};
use kimchi_core::{Asset, Id, MediaKind};
use serde_json::json;

use crate::store::{MenuEntry, MenuItem, Store, StoreExt};
use crate::theme::{ActiveTheme, MONO, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, icon};
use crate::views::inspector::{ai, format::short, insert_at_playhead, poster_path};
use crate::views::timeline::dnd::MediaDrag;
use confirm::Removal;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Filter {
    All,
    Video,
    Image,
    Audio,
    Generated,
}

impl Filter {
    const ALL: [(Filter, &'static str); 5] =
        [(Filter::All, "All"), (Filter::Video, "Video"), (Filter::Image, "Images"), (Filter::Audio, "Audio"), (Filter::Generated, "Generated")];

    fn keeps(self, a: &Asset) -> bool {
        match self {
            Filter::All => true,
            Filter::Video => a.kind == MediaKind::Video,
            Filter::Image => a.kind == MediaKind::Image,
            Filter::Audio => a.kind == MediaKind::Audio,
            Filter::Generated => a.is_generated(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Grid,
    List,
}

/// Grid tiles are at least this wide.
const TILE_MIN: f32 = 118.;
const GAP: f32 = 8.;
const PAD: f32 = 10.;

pub struct MediaPanel {
    store: Entity<Store>,
    filter: Filter,
    layout: Layout,
    search: Entity<TextInput>,
    /// Width of the list area, measured each frame (the grid's column count follows it).
    width: Rc<Cell<f32>>,
    removal: Option<Removal>,
    confirm_focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl MediaPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search media"));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, input, e: &InputEvent, cx| match e {
                InputEvent::Changed(_) => cx.notify(),
                InputEvent::Cancel => input.update(cx, |i, cx| i.set_text("", cx)),
                _ => {}
            }),
        ];
        Self {
            store,
            filter: Filter::All,
            layout: Layout::Grid,
            search,
            width: Rc::new(Cell::new(300.)),
            removal: None,
            confirm_focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn ask_removal(&mut self, a: &Asset, window: &mut Window, cx: &mut Context<Self>) {
        let clips = self.store.read(cx).project.as_ref().map(|p| p.clips().filter(|(_, c)| c.asset_id() == Some(a.id)).count()).unwrap_or(0);
        self.removal = Some(Removal { asset: a.id, name: a.name.clone(), clips });
        window.focus(&self.confirm_focus, cx);
        cx.notify();
    }

    fn answer(&mut self, remove: bool, cx: &mut Context<Self>) {
        if let Some(r) = self.removal.take()
            && remove
        {
            self.store.update(cx, |s, cx| {
                s.run("media.remove", json!({ "assetId": r.asset }), cx);
                if s.selected_asset == Some(r.asset) {
                    s.select_asset(None, cx);
                }
            });
        }
        cx.notify();
    }

    fn open_menu(&mut self, a: &Asset, position: Point<Pixels>, cx: &mut Context<Self>) {
        let this = cx.entity();
        let id = a.id;
        let mut entries = vec![MenuItem::new("Insert at playhead", move |_, cx| insert_at_playhead(id, cx)).icon("plus").entry()];
        if a.kind == MediaKind::Image {
            let (a1, a2) = (a.clone(), a.clone());
            entries.push(MenuEntry::Separator);
            entries.push(MenuItem::new("Animate with AI", move |_, cx| ai::animate_image(&a1, cx)).icon("clapperboard").ai().entry());
            entries.push(MenuItem::new("Edit with AI", move |_, cx| ai::edit_image(&a2, cx)).icon("wand-sparkles").ai().entry());
        }
        if a.is_generated() {
            let (a1, a2) = (a.clone(), a.clone());
            entries.push(MenuEntry::Separator);
            entries.push(MenuItem::new("Regenerate", move |_, cx| ai::regenerate_asset(&a1, false, cx)).icon("refresh-cw").ai().entry());
            entries.push(MenuItem::new("Variation", move |_, cx| ai::regenerate_asset(&a2, true, cx)).icon("shuffle").ai().entry());
        }
        let path = a.path.clone();
        let doomed = a.clone();
        entries.push(MenuEntry::Separator);
        entries.push(MenuItem::new("Reveal in Finder", move |_, cx| cx.reveal_path(std::path::Path::new(&path))).icon("folder-search").entry());
        entries.push(MenuEntry::Separator);
        entries.push(MenuItem::new("Remove from project…", move |window, cx| this.update(cx, |p, cx| p.ask_removal(&doomed, window, cx))).icon("trash").danger().entry());
        self.store.update(cx, |s, cx| {
            s.select_asset(Some(id), cx);
            s.open_menu(position, entries, cx);
        });
    }

    /// Click selects; a double-click puts it on the timeline; right-click opens the menu; a drag
    /// carries it to the timeline.
    fn interactive(&self, el: Stateful<Div>, a: &Asset, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = a.id;
        let asset = a.clone();
        let drag = MediaDrag { asset_id: a.id, name: a.name.clone().into(), thumbnail: poster_path(a) };
        el.cursor_pointer()
            .on_click(move |e, _, cx| {
                if e.click_count() >= 2 {
                    insert_at_playhead(id, cx);
                } else {
                    cx.store().update(cx, |s, cx| s.select_asset(Some(id), cx));
                }
            })
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_menu(&asset, e.position, cx);
            }))
            .on_drag(drag, |d: &MediaDrag, _, _, cx| cx.new(|_| d.clone()))
            .tooltip({
                let name: SharedString = a.name.clone().into();
                move |_, cx| crate::ui::tooltip(name.clone(), cx)
            })
    }

    /// The picture part of a tile (thumbnail, kind and length, the generated mark).
    fn thumb(&self, a: &Asset, w: f32, h: f32, badges: bool, cx: &App) -> Div {
        let t = cx.theme().clone();
        let picture: AnyElement = match poster_path(a) {
            Some(p) => img(PathBuf::from(p)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
            None if a.kind == MediaKind::Audio => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.clip_audio)
                .child(icon("audio-lines").size(px(if badges { 22. } else { 14. })).text_color(t.success))
                .into_any_element(),
            // The thumbnail is still being made.
            None => div().size_full().flex().items_center().justify_center().bg(t.bg_sunken).child(icon("loader-circle").text_color(t.text_3)).into_any_element(),
        };
        let kind_icon = match a.kind {
            MediaKind::Video => "film",
            MediaKind::Image => "image",
            MediaKind::Audio => "audio-lines",
        };
        div()
            .relative()
            .flex_none()
            .w(px(w))
            .h(px(h))
            .rounded(px(if badges { sz::R_MD } else { sz::R_SM }))
            .overflow_hidden()
            .bg(t.bg_sunken)
            .child(picture)
            .when(badges, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(5.))
                        .bottom(px(5.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(sz::R_XS))
                        .bg(t.glass2.bg)
                        .text_color(t.text)
                        .text_size(px(10.))
                        .child(icon(kind_icon).size(px(10.)).text_color(t.text))
                        .when_some(a.meta.duration.filter(|_| a.kind != MediaKind::Image), |d, dur| d.child(div().font_family(MONO).child(short(dur)))),
                )
                .when(a.is_generated(), |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(5.))
                            .right(px(5.))
                            .size(px(18.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(t.accent)
                            .text_color(t.text_on_accent)
                            .child(icon("sparkles").size(px(10.)).text_color(t.text_on_accent)),
                    )
                })
            })
    }

    fn tile(&self, a: &Asset, selected: bool, w: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = cx.theme().clone();
        let h = (w / 1.6).round();
        let generated = a.is_generated();
        let frame = div()
            .rounded(px(sz::R_MD + 2.))
            .p(px(2.))
            .border_1()
            .border_color(if selected {
                t.accent
            } else if generated {
                t.accent.opacity(0.55)
            } else {
                t.line
            })
            .when(selected, |d| d.bg(t.accent_soft))
            .when(!selected, |d| d.group_hover("media-tile", |s| s.border_color(t.line_strong)))
            .child(self.thumb(a, w - 6., h, true, cx));
        let el = div()
            .id(ElementId::from(a.id))
            .group("media-tile")
            .w(px(w))
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(frame)
            .child(
                div()
                    .px(px(2.))
                    .truncate()
                    .text_size(px(sz::SM))
                    .text_color(if selected { t.text } else { t.text_2 })
                    .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(a.name.clone()),
            );
        self.interactive(el, a, cx)
    }

    fn row(&self, a: &Asset, selected: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = cx.theme().clone();
        let mut facts = vec![match a.kind {
            MediaKind::Video => "Video".to_string(),
            MediaKind::Image => "Image".to_string(),
            MediaKind::Audio => "Audio".to_string(),
        }];
        if let Some(d) = a.meta.duration.filter(|_| a.kind != MediaKind::Image) {
            facts.push(short(d));
        }
        if let (Some(w), Some(h)) = (a.meta.width, a.meta.height) {
            facts.push(format!("{w}×{h}"));
        }
        let el = div()
            .id(ElementId::from(a.id))
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(4.))
            .rounded(px(sz::R_SM))
            .border_1()
            .border_color(if selected { t.accent_ring } else { gpui::transparent_black() })
            .when(selected, |d| d.bg(t.accent_soft))
            .when(!selected, |d| d.hover(|s| s.bg(t.hover)))
            .child(self.thumb(a, 56., 35., false, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().text_size(px(sz::BASE)).text_color(t.text).child(a.name.clone()))
                    .child(div().truncate().font_family(MONO).text_size(px(sz::XS)).text_color(t.text_2).child(facts.join(" · "))),
            )
            .when(a.is_generated(), |d| d.child(icon("sparkles").text_color(t.accent_text)));
        self.interactive(el, a, cx)
    }

    fn empty(&self, searching: bool, cx: &App) -> AnyElement {
        let t = cx.theme().clone();
        let (title, hint) = if searching {
            ("Nothing matches".to_string(), "Try another word, or another filter.".to_string())
        } else if self.filter == Filter::Generated {
            ("Nothing generated yet".into(), "Generations land here and on your timeline.".into())
        } else {
            ("Drop files anywhere".into(), "or click to browse — video, images and audio.".into())
        };
        let importing = !searching && self.filter != Filter::Generated;
        div()
            .id("media-empty")
            .mt(px(8.))
            .px(px(16.))
            .py(px(28.))
            .rounded(px(sz::R_LG))
            .border_1()
            .border_dashed()
            .border_color(t.line_strong)
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .text_color(t.text_2)
            .when(importing, |d| d.cursor_pointer().hover(|s| s.border_color(t.accent_ring).bg(t.accent_soft)).on_click(|_, _, cx| crate::app::import_dialog(cx)))
            .child(
                div()
                    .size(px(40.))
                    .mb(px(4.))
                    .rounded(px(sz::R_MD))
                    .bg(t.hover)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(if searching { "search" } else if importing { "import" } else { "sparkles" }).size(px(20.)).text_color(t.text)),
            )
            .child(div().text_color(t.text).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_size(px(sz::SM)).text_center().child(hint))
            .into_any_element()
    }
}

impl Render for MediaPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (assets, selected): (Vec<Asset>, Option<Id>) = {
            let s = self.store.read(cx);
            (s.project.as_ref().map(|p| p.assets.iter().rev().cloned().collect()).unwrap_or_default(), s.selected_asset)
        };
        let generated = assets.iter().filter(|a| a.is_generated()).count();
        let query = self.search.read(cx).text().trim().to_lowercase();
        let shown: Vec<&Asset> = assets.iter().filter(|a| self.filter.keeps(a) && (query.is_empty() || a.name.to_lowercase().contains(&query))).collect();

        let layout = self.layout;
        let header = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(14.))
            .pt(px(12.))
            .pb(px(8.))
            .child(div().flex_1().text_size(px(sz::BASE)).font_weight(FontWeight::SEMIBOLD).child("Media"))
            .child(
                Button::icon("media-grid", "layout-grid", "Show as a grid")
                    .small()
                    .selected(layout == Layout::Grid)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.layout = Layout::Grid;
                        cx.notify();
                    })),
            )
            .child(
                Button::icon("media-list", "list", "Show as a list")
                    .small()
                    .selected(layout == Layout::List)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.layout = Layout::List;
                        cx.notify();
                    })),
            )
            .child(Button::new("media-import", "Import").small().with_icon("import").tooltip("Import files (⌘I)").on_click(|_, _, cx| crate::app::import_dialog(cx)));

        let filters = div().flex().flex_wrap().gap(px(2.)).px(px(PAD)).pb(px(8.)).children(Filter::ALL.iter().map(|&(f, label)| {
            let on = self.filter == f;
            div()
                .id(label)
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(24.))
                .px(px(8.))
                .rounded_full()
                .text_size(px(sz::SM))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |d| d.bg(t.accent_soft).text_color(t.accent_text))
                .when(!on, |d| d.text_color(t.text_2).hover(|s| s.bg(t.hover).text_color(t.text)))
                .when(f == Filter::Generated, |d| d.child(icon("sparkles").size(px(11.)).text_color(if on { t.accent_text } else { t.text_2 })))
                .child(label)
                .when(f == Filter::Generated && generated > 0, |d| d.child(div().font_family(MONO).text_size(px(10.)).text_color(t.accent_text).child(generated.to_string())))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter = f;
                    cx.notify();
                }))
        }));

        // Columns from the list's width measured last frame.
        let inner = self.width.get().max(TILE_MIN);
        let cols = (((inner + GAP) / (TILE_MIN + GAP)).floor() as usize).max(1);
        let tile_w = ((inner - GAP * (cols as f32 - 1.)) / cols as f32).floor();

        let list: AnyElement = if shown.is_empty() {
            self.empty(!assets.is_empty() && !query.is_empty(), cx)
        } else {
            match layout {
                Layout::Grid => {
                    let tiles: Vec<_> = shown.iter().map(|a| self.tile(a, selected == Some(a.id), tile_w, cx)).collect();
                    div().flex().flex_wrap().gap_x(px(GAP)).gap_y(px(12.)).children(tiles).into_any_element()
                }
                Layout::List => {
                    let rows: Vec<_> = shown.iter().map(|a| self.row(a, selected == Some(a.id), cx)).collect();
                    div().flex().flex_col().gap(px(2.)).children(rows).into_any_element()
                }
            }
        };
        let cell = self.width.clone();
        let measure = canvas(
            move |bounds, window, _| {
                let w = f32::from(bounds.size.width);
                if (cell.get() - w).abs() > 0.5 {
                    cell.set(w);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        let removal = self.removal.clone().map(|r| confirm::dialog(&r, &self.confirm_focus, cx.entity(), |p: &mut Self, yes, cx| p.answer(yes, cx), window, cx));

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().px(px(PAD)).pb(px(8.)).child(self.search.clone()))
            .child(filters)
            .child(
                div()
                    .id("media-scroll")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(PAD))
                    .pb(px(14.))
                    .child(div().relative().w_full().child(measure).child(list))
                    .when(!shown.is_empty(), |d| {
                        d.child(
                            div()
                                .pt(px(14.))
                                .text_size(px(sz::XS))
                                .text_color(t.text_3)
                                .child("Drag onto the timeline, or double-click to insert at the playhead. Drop files anywhere to import."),
                        )
                    }),
            )
            .children(removal)
    }
}
