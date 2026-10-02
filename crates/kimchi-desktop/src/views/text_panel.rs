use gpui::{Context, Render, Window, div, prelude::*};

use crate::theme::ActiveTheme;

pub struct TextPanel;

impl TextPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for TextPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().text_color(cx.theme().text_3).child("TextPanel")
    }
}
