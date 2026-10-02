use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, div, prelude::*, px, size};
use gpui_platform::application;

struct Hello;
impl Render for Hello {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().child("kimchi")
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        cx.open_window(WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() }, |_, cx| cx.new(|_| Hello)).unwrap();
    });
}
