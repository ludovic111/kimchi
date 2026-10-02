use gpui::{Context, Render, Window, div, prelude::*};

use crate::theme::ActiveTheme;

pub struct PreviewView;

impl PreviewView {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for PreviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().text_color(cx.theme().text_3).child("Preview")
    }
}
