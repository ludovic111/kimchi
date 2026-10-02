//! The font families a title can use (the bundled ones first, then the system's), read once
//! off the main thread, and the picker the inspector shows them in.

use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, MouseButton, SharedString, Subscription, Window, anchored, deferred, div, point,
    prelude::*, px, uniform_list,
};

use crate::theme::{ActiveTheme, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{GlassExt, icon};

#[derive(Default)]
struct FontList {
    list: Option<Arc<Vec<SharedString>>>,
    loading: bool,
}

impl Global for FontList {}

/// The families, or `None` while they load (the first call starts loading them; windows
/// refresh when they are in).
pub fn families(cx: &mut App) -> Option<Arc<Vec<SharedString>>> {
    if !cx.has_global::<FontList>() {
        cx.set_global(FontList::default());
    }
    let state = cx.global::<FontList>();
    if state.list.is_some() || state.loading {
        return state.list.clone();
    }
    cx.global_mut::<FontList>().loading = true;
    let task = cx.background_spawn(async { kimchi_media::text::font_families() });
    cx.spawn(async move |cx| {
        let list: Vec<SharedString> = task.await.into_iter().map(SharedString::from).collect();
        cx.update(|cx| {
            *cx.global_mut::<FontList>() = FontList { list: Some(Arc::new(list)), loading: false };
            cx.refresh_windows();
        });
    })
    .detach();
    None
}

/// The families whose name contains `query` (case-insensitive).
pub fn filter(list: &[SharedString], query: &str) -> Vec<SharedString> {
    let q = query.trim().to_lowercase();
    list.iter().filter(|f| q.is_empty() || f.to_lowercase().contains(&q)).cloned().collect()
}

/// One row of a font list: the name in the interface font, a sample in the family itself
/// (symbol fonts would make their own name unreadable).
pub fn font_row(id: impl Into<gpui::ElementId>, family: SharedString, selected: bool, cx: &App) -> gpui::Stateful<gpui::Div> {
    let t = cx.theme();
    div()
        .id(id)
        .w_full()
        .h(px(30.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(sz::R_SM))
        .cursor_pointer()
        .when(selected, |d| d.bg(t.accent_soft).text_color(t.accent_text))
        .when(!selected, |d| d.text_color(t.text).hover(|s| s.bg(t.hover)))
        .child(div().flex_1().min_w_0().truncate().text_size(px(sz::BASE)).child(family.clone()))
        .child(div().flex_none().text_size(px(sz::MD)).font_family(family).child("Aa"))
}

/// A family was picked.
pub struct FontPicked(pub SharedString);

/// A button showing the family, opening a searchable list.
pub struct FontPicker {
    value: SharedString,
    open: bool,
    search: Entity<TextInput>,
    focus: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<FontPicked> for FontPicker {}

impl Focusable for FontPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FontPicker {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search fonts"));
        let sub = cx.subscribe(&search, |this: &mut Self, _, e: &InputEvent, cx| match e {
            InputEvent::Changed(_) => cx.notify(),
            InputEvent::Cancel => this.close(cx),
            InputEvent::Submit => {
                // Enter picks the first match.
                let q = this.search.read(cx).text().to_string();
                if let Some(first) = families(cx).and_then(|l| filter(&l, &q).into_iter().next()) {
                    this.pick(first, cx);
                }
            }
            InputEvent::Blur => {}
        });
        Self { value: "".into(), open: false, search, focus: cx.focus_handle(), _sub: sub }
    }

    pub fn set_value(&mut self, v: &str) {
        if self.value.as_ref() != v {
            self.value = v.to_string().into();
        }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open {
            self.search.update(cx, |s, cx| s.set_text("", cx));
            crate::ui::input::focus(&self.search, window, cx);
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn pick(&mut self, family: SharedString, cx: &mut Context<Self>) {
        self.value = family.clone();
        self.open = false;
        cx.emit(FontPicked(family));
        cx.notify();
    }
}

impl gpui::Render for FontPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let open = self.open;
        let viewport = window.viewport_size();
        let button = div()
            .id("font-button")
            .track_focus(&self.focus)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(8.))
            .rounded(px(sz::R_SM))
            .bg(t.bg_sunken.opacity(0.6))
            .border_1()
            .border_color(if open { t.accent_ring } else { t.line })
            .cursor_pointer()
            .hover(|s| s.border_color(t.line_strong))
            .child(div().flex_1().min_w_0().truncate().text_size(px(sz::BASE)).font_family(self.value.clone()).child(self.value.clone()))
            .child(icon("chevron-down").text_color(t.text_2))
            .tooltip(|_, cx| crate::ui::tooltip("Font".into(), cx))
            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)));

        let popover = open.then(|| {
            let query = self.search.read(cx).text().to_string();
            let list = families(cx);
            let items = list.as_ref().map(|l| filter(l, &query)).unwrap_or_default();
            let count = items.len();
            let current = self.value.clone();
            let this = cx.entity();
            let body = match list {
                None => div().p(px(12.)).text_size(px(sz::SM)).text_color(t.text_2).child("Loading fonts…").into_any_element(),
                Some(_) if count == 0 => div().p(px(12.)).text_size(px(sz::SM)).text_color(t.text_2).child("No font matches.").into_any_element(),
                Some(_) => uniform_list("font-list", count, move |range, _, cx| {
                    range
                        .map(|i| {
                            let f = items[i].clone();
                            let this = this.clone();
                            font_row(("font", i), f.clone(), f == current, cx).on_click(move |_, _, cx| this.update(cx, |p, cx| p.pick(f.clone(), cx)))
                        })
                        .collect()
                })
                .h(px(260.))
                .into_any_element(),
            };
            // A clear layer over the window closes the list on a click anywhere else (and keeps
            // that click from reaching what is under it, the button included).
            let closer = deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("font-closer")
                        .occlude()
                        .w(viewport.width)
                        .h(viewport.height)
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.close(cx))),
                ),
            )
            .with_priority(2);
            let list = deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id("font-popover")
                        .occlude()
                        .mt(px(4.))
                        .w(px(260.))
                        .rounded(px(sz::R_MD))
                        // Without a backdrop blur, the tier alone would let the inspector show
                        // through the list: lay it over the raised surface.
                        .bg(t.bg_raised)
                        .shadow(t.glass_shadow())
                        .child(
                            div()
                                .size_full()
                                .p(px(6.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .rounded(px(sz::R_MD))
                                .glass(t.glass2)
                                .child(self.search.clone())
                                .child(body),
                        ),
                ),
            )
            .with_priority(3);
            [closer, list]
        });

        div().relative().flex_1().min_w_0().child(button).children(popover.into_iter().flatten())
    }
}
