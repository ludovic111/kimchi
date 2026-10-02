use gpui::{AnyElement, App, Context, Render, Window, div, prelude::*, px, svg};

use crate::assets::icon_path;
use crate::theme::{ActiveTheme, parse_color};

/// The kimchi mark: a napa stem with a chili leaf and a green-onion leaf.
pub fn mark(size: f32, cx: &App) -> AnyElement {
    let t = cx.theme();
    let stem = if t.is_dark() { parse_color("#FFF4E6") } else { parse_color("#e9dccb") };
    let layer = |name: &str, c| svg().path(icon_path(name)).absolute().inset_0().size(px(size)).text_color(c);
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(layer("mark-stem", stem))
        .child(layer("mark-chili", t.accent))
        .child(layer("mark-leaf", parse_color("#A6CF5E")))
        .into_any_element()
}

pub struct Home;

impl Home {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().text_color(cx.theme().text_3).child("Home")
    }
}
