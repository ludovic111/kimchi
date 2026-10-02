use gpui::{Context, Render, Window, div, prelude::*};

use crate::theme::ActiveTheme;

pub struct JobsPopover;

impl JobsPopover {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for JobsPopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().text_color(cx.theme().text_3).child("JobsPopover")
    }
}
