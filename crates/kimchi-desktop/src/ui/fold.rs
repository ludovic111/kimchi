//! A titled group of controls that folds away: the inspector's sections. The header (a
//! chevron, the title in caps running into a hairline, and optional controls on the right) toggles it; while it is
//! folded, children added to it are dropped, so callers build it the same way either way.

use std::rc::Rc;

use gpui::{AnyElement, App, Div, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, prelude::*, px};

use crate::theme::{ActiveTheme, MONO};
use crate::ui::icon;

type Toggle = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Fold {
    id: ElementId,
    title: SharedString,
    open: bool,
    trailing: Vec<AnyElement>,
    on_toggle: Option<Toggle>,
    body: Div,
}

impl Fold {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>, open: bool) -> Self {
        Self { id: id.into(), title: title.into(), open, trailing: vec![], on_toggle: None, body: div().flex().flex_col().gap(px(10.)) }
    }

    /// Controls at the right of the header (a reset button, keyframe arrows…), shown folded too.
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing.push(el.into_any_element());
        self
    }

    pub fn on_toggle(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }
}

impl ParentElement for Fold {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if self.open {
            self.body.extend(elements);
        }
    }
}

impl RenderOnce for Fold {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = cx.theme().clone();
        let toggle = self.on_toggle.clone();
        let open = self.open;
        let header = div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(38.))
            .px(px(14.))
            .cursor_pointer()
            .text_color(t.text_2)
            .hover(|s| s.text_color(t.text))
            .role(gpui::Role::Button)
            .aria_label(self.title.clone())
            .child(icon(if open { "chevron-down" } else { "chevron-right" }).size(px(12.)))
            .child(div().flex_shrink(1.).min_w_0().truncate().font_family(MONO).text_size(px(10.5)).child(self.title.to_uppercase()))
            // The title runs into a hairline, as on a drawing.
            .child(div().flex_1().min_w(px(8.)).h(px(1.)).mx(px(4.)).bg(t.line))
            .child(div().flex().flex_none().items_center().gap(px(2.)).on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()).children(self.trailing))
            .when_some(toggle, |d, f| d.on_click(move |_, w, cx| f(w, cx)));
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(t.line)
            .child(header)
            .when(open, |d| d.child(self.body.px(px(14.)).pt(px(2.)).pb(px(16.))))
    }
}

impl Styled for Fold {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.body.style()
    }
}
