//! The text panel (left column): title presets that drop at the playhead, and the fonts a
//! title can use (click one to set it on the selected titles, or to start a title in it).

use gpui::{App, Context, Entity, FontWeight, Render, SharedString, Subscription, Window, div, prelude::*, px, uniform_list};
use kimchi_core::ClipContent;
use serde_json::{Value, json};

use crate::store::{Store, StoreExt};
use crate::theme::{ActiveTheme, grey, parse_color, size as sz};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Button, caps};
use crate::views::inspector::fonts;

/// One title preset: its style (the `clip.addText` style fields) and where it sits.
struct Preset {
    name: &'static str,
    content: &'static str,
    family: &'static str,
    size: f64,
    weight: f32,
    italic: bool,
    color: &'static str,
    background: Option<&'static str>,
    align: &'static str,
    line_height: f64,
    letter_spacing: f64,
    shadow: bool,
    /// Vertical offset as a fraction of the canvas height (positive is down).
    y: f64,
}

impl Preset {
    fn style(&self, height: f64) -> Value {
        json!({
            "content": self.content,
            "fontFamily": self.family,
            "fontSize": self.size,
            "fontWeight": self.weight,
            "italic": self.italic,
            "color": self.color,
            "background": self.background,
            "align": self.align,
            "lineHeight": self.line_height,
            "letterSpacing": self.letter_spacing,
            "shadow": self.shadow,
            "y": (self.y * height).round(),
        })
    }
}

/// The colours here are picture colours (what the title looks like in the video).
const PRESETS: [Preset; 6] = [
    Preset {
        name: "Title",
        content: "Your title",
        family: "Manrope",
        size: 140.,
        weight: 700.,
        italic: false,
        color: "#ffffff",
        background: None,
        align: "center",
        line_height: 1.1,
        letter_spacing: -3.,
        shadow: true,
        y: 0.,
    },
    Preset {
        name: "Editorial",
        content: "A quiet morning",
        family: "Instrument Serif",
        size: 150.,
        weight: 400.,
        italic: true,
        color: "#ffffff",
        background: None,
        align: "center",
        line_height: 1.1,
        letter_spacing: -2.,
        shadow: true,
        y: 0.,
    },
    Preset {
        name: "Lower third",
        content: "Name Surname\nRole, Company",
        family: "Manrope",
        size: 52.,
        weight: 600.,
        italic: false,
        color: "#ffffff",
        background: None,
        align: "left",
        line_height: 1.25,
        letter_spacing: 0.,
        shadow: true,
        y: 0.32,
    },
    Preset {
        name: "Caption",
        content: "and that's when it clicked",
        family: "Manrope",
        size: 60.,
        weight: 650.,
        italic: false,
        color: "#ffffff",
        background: Some("#000000cc"),
        align: "center",
        line_height: 1.1,
        letter_spacing: 0.,
        shadow: false,
        y: 0.36,
    },
    Preset {
        name: "Label",
        content: "CHAPTER 01",
        family: "IBM Plex Mono",
        size: 44.,
        weight: 500.,
        italic: false,
        color: "#f7806a",
        background: None,
        align: "center",
        line_height: 1.1,
        letter_spacing: 6.,
        shadow: false,
        y: -0.3,
    },
    Preset {
        name: "Shout",
        content: "WAIT FOR IT",
        family: "Manrope",
        size: 180.,
        weight: 800.,
        italic: false,
        color: "#f0b44c",
        background: None,
        align: "center",
        line_height: 1.1,
        letter_spacing: -4.,
        shadow: true,
        y: 0.,
    },
];

/// Height of the font list (it scrolls on its own).
const FONT_LIST_H: f32 = 300.;

pub struct TextPanel {
    store: Entity<Store>,
    search: Entity<TextInput>,
    _subs: Vec<Subscription>,
}

impl TextPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = cx.store();
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search fonts"));
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, input, e: &InputEvent, cx| match e {
                InputEvent::Changed(_) => cx.notify(),
                InputEvent::Cancel => input.update(cx, |i, cx| i.set_text("", cx)),
                _ => {}
            }),
        ];
        Self { store, search, _subs: subs }
    }
}

/// The selected text clips.
fn selected_titles(cx: &App) -> Vec<(kimchi_core::Id, String)> {
    let s = cx.store().read(cx);
    s.selected_clips()
        .into_iter()
        .filter_map(|c| match &c.content {
            ClipContent::Text { style } => Some((c.id, style.font_family.clone())),
            _ => None,
        })
        .collect()
}

/// Sets `family` on the selected titles (one undo step), or starts a title in it.
fn use_font(family: SharedString, cx: &mut App) {
    let titles = selected_titles(cx);
    if titles.is_empty() {
        crate::app::add_text(json!({ "fontFamily": family.to_string() }), cx);
        return;
    }
    let commands: Vec<Value> = titles.iter().map(|(id, _)| json!({ "command": "clip.update", "params": { "clipId": id, "style": { "fontFamily": family.to_string() } } })).collect();
    cx.store().update(cx, |s, cx| s.run("project.batch", json!({ "commands": commands, "label": "font" }), cx));
}

impl Render for TextPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let height = self.store.read(cx).project.as_ref().map(|p| p.settings.height as f64).unwrap_or(1080.0);
        let titles = selected_titles(cx);
        let current: Option<String> = titles.first().map(|(_, f)| f.clone());

        // A stand-in for the picture: near black in both modes, since titles are mostly light
        // over footage.
        let canvas_bg = grey(0.05);
        let tiles = PRESETS.iter().enumerate().map(|(i, p)| {
            let first = p.content.lines().next().unwrap_or("").to_string();
            let sample = div()
                .px(px(6.))
                .py(px(2.))
                .rounded(px(sz::R_XS))
                .font_family(p.family)
                .font_weight(FontWeight(p.weight))
                .text_size(px(17.))
                .text_color(parse_color(p.color))
                .max_w_full()
                .truncate()
                .child(first);
            let sample = sample.when(p.italic, |d| d.italic()).when_some(p.background, |d, bg| d.bg(parse_color(bg)));
            let style = p.style(height);
            div()
                .id(("preset", i))
                .flex()
                .flex_col()
                .gap(px(6.))
                .cursor_pointer()
                .group("preset")
                .child(
                    div()
                        .h(px(74.))
                        .rounded(px(sz::R_MD))
                        .bg(canvas_bg)
                        .border_1()
                        .border_color(t.line)
                        .group_hover("preset", |s| s.border_color(t.accent_ring))
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(8.))
                        .child(sample),
                )
                .child(div().px(px(2.)).text_size(px(sz::SM)).text_color(t.text_2).child(p.name))
                .tooltip(move |_, cx| crate::ui::tooltip(format!("Add a {} at the playhead", p.name.to_lowercase()).into(), cx))
                .on_click(move |_, _, cx| crate::app::add_text(style.clone(), cx))
        });

        let query = self.search.read(cx).text().to_string();
        let list = fonts::families(cx);
        let items = list.as_ref().map(|l| fonts::filter(l, &query)).unwrap_or_default();
        let count = items.len();
        let fonts_list = match list {
            None => div().p(px(12.)).text_size(px(sz::SM)).text_color(t.text_2).child("Loading fonts…").into_any_element(),
            Some(_) if count == 0 => div().p(px(12.)).text_size(px(sz::SM)).text_color(t.text_2).child("No font matches.").into_any_element(),
            Some(_) => uniform_list("text-fonts", count, move |range, _, cx| {
                range
                    .map(|i| {
                        let f = items[i].clone();
                        let selected = current.as_deref() == Some(f.as_ref());
                        fonts::font_row(("text-font", i), f.clone(), selected, cx).on_click(move |_, _, cx| use_font(f.clone(), cx))
                    })
                    .collect()
            })
            .h(px(FONT_LIST_H))
            .into_any_element(),
        };
        let hint = if titles.is_empty() { "Click a font to start a title in it." } else { "Click a font to use it for the selected titles." };

        // The whole panel scrolls; the font list scrolls inside it, at a fixed height.
        div()
            .id("text-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .child(div().flex_1().text_size(px(sz::BASE)).font_weight(FontWeight::SEMIBOLD).child("Text"))
                    .child(Button::new("text-add", "Add title").small().with_icon("plus").tooltip("Add a title at the playhead (T)").on_click(|_, _, cx| crate::app::add_text(json!({}), cx))),
            )
            .child(div().px(px(10.)).grid().grid_cols(2).gap(px(8.)).children(tiles))
            .child(div().px(px(14.)).pt(px(10.)).pb(px(4.)).text_size(px(sz::SM)).text_color(t.text_2).child("Click to drop at the playhead. Edit the words and style in the inspector."))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .mt(px(8.))
                    .pt(px(12.))
                    .px(px(10.))
                    .pb(px(10.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(div().flex().items_center().justify_between().px(px(4.)).child(caps("Fonts", cx)).child(caps(if count > 0 { count.to_string() } else { String::new() }, cx)))
                    .child(self.search.clone())
                    .child(div().px(px(4.)).text_size(px(sz::XS)).text_color(t.text_2).child(hint))
                    .child(fonts_list),
            )
    }
}
