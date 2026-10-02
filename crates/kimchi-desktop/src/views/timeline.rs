use gpui::{App, Context, Pixels, Render, Window, div, prelude::*, px};

use crate::theme::ActiveTheme;

pub struct Timeline;

impl Timeline {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }

    /// Width of the track area, for zoom to fit.
    pub fn tracks_width(&self) -> Pixels {
        px(900.)
    }
}

impl Render for Timeline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _: &App = cx;
        div().size_full().p_4().text_color(cx.theme().text_3).child("Timeline")
    }
}
