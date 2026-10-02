//! Drag payloads the timeline accepts. The media panel starts a drag with
//! `.on_drag(MediaDrag { .. }, |drag, _offset, _window, cx| cx.new(|_| drag.clone()))`
//! (MediaDrag renders its own floating preview); the timeline's tracks take it
//! with `on_drop::<MediaDrag>` and place the asset with `clip.insertMedia` at the
//! drop position.

use gpui::{Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, img, px};

use crate::theme::{ActiveTheme, size};
use crate::ui::GlassExt;

/// A media asset being dragged from the media panel.
#[derive(Clone, Debug)]
pub struct MediaDrag {
    pub asset_id: kimchi_core::Id,
    pub name: SharedString,
    /// A thumbnail path (poster image), drawn in the drag preview.
    pub thumbnail: Option<String>,
}

impl Render for MediaDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .p_1()
            .pr_2()
            .rounded(px(size::R_SM))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .border_1()
            .border_color(t.accent)
            .text_size(px(size::SM))
            .text_color(t.text)
            .opacity(0.92)
            .children(self.thumbnail.clone().map(|p| img(std::path::PathBuf::from(p)).w(px(48.)).h(px(27.)).rounded(px(size::R_XS))))
            .child(div().max_w(px(180.)).truncate().child(self.name.clone()))
    }
}
