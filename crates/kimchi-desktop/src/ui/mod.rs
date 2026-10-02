//! Small building blocks shared by every view: buttons, icons, glass
//! surfaces, segmented controls, switches, tooltips, numeric fields.

pub mod drag;
pub mod input;
pub mod scrub;

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Stateful, Styled, Svg, Window,
    div, prelude::*, px, svg,
};

use crate::assets::icon_path;
use crate::theme::{ActiveTheme, Glass, MONO, size as sz};

pub type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A lucide icon, 14 px, in the surrounding text colour unless given its own.
pub fn icon(name: &str) -> Icon {
    Icon(svg().path(icon_path(name)).size(px(14.)).flex_none())
}

/// An icon that takes the inherited text colour (GPUI's `svg` only paints with
/// a colour set on itself).
#[derive(IntoElement)]
pub struct Icon(Svg);

impl Icon {
    /// Rotates or scales the drawing (spinners).
    pub fn with_transformation(self, t: gpui::Transformation) -> Self {
        Self(self.0.with_transformation(t))
    }
}

impl Styled for Icon {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.0.style()
    }
}

impl RenderOnce for Icon {
    fn render(mut self, window: &mut Window, _: &mut App) -> impl IntoElement {
        if self.0.style().text.color.is_none() {
            let color = window.text_style().color;
            self.0 = self.0.text_color(color);
        }
        self.0
    }
}

/// One of the three glass tiers: fill, 1 px edge, top highlight; tier 2 and 3 float with a shadow.
pub trait GlassExt: Styled + Sized {
    fn glass(self, g: Glass) -> Self {
        self.bg(g.bg).border_1().border_color(g.edge)
    }
}
impl<T: Styled> GlassExt for T {}

/// Section heading in caps (IBM Plex Mono, as the design system asks for labels in caps).
pub fn caps(text: impl Into<SharedString>, cx: &App) -> Div {
    div().font_family(MONO).text_size(px(10.5)).text_color(cx.theme().text_2).child(text.into().to_uppercase())
}

pub fn label(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_size(px(sz::SM)).text_color(cx.theme().text_2).child(text.into())
}

/// A keyboard shortcut chip.
pub fn kbd(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = cx.theme();
    div()
        .font_family(MONO)
        .text_size(px(10.5))
        .px(px(5.))
        .py(px(1.))
        .rounded(px(sz::R_XS))
        .border_1()
        .border_color(t.line_strong)
        .text_color(t.text_2)
        .child(text.into())
}

/// Hairline separator.
pub fn hairline(cx: &App) -> Div {
    div().h(px(1.)).w_full().bg(cx.theme().line)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    /// The accent fill: the one primary action of a surface.
    Primary,
    /// Quiet outline.
    Secondary,
    /// No chrome until hovered.
    Ghost,
    Danger,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<&'static str>,
    icon_after: Option<&'static str>,
    variant: Variant,
    small: bool,
    disabled: bool,
    selected: bool,
    full: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
    color: Option<Hsla>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: Some(label.into()),
            icon: None,
            icon_after: None,
            variant: Variant::Secondary,
            small: false,
            disabled: false,
            selected: false,
            full: false,
            tooltip: None,
            on_click: None,
            color: None,
        }
    }

    /// An icon-only button; `tooltip` names it for people and screen readers.
    pub fn icon(id: impl Into<ElementId>, icon: &'static str, tooltip: impl Into<SharedString>) -> Self {
        let mut b = Self::new(id, "");
        b.label = None;
        b.icon = Some(icon);
        b.variant = Variant::Ghost;
        b.tooltip = Some(tooltip.into());
        b
    }

    pub fn with_icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn icon_after(mut self, icon: &'static str) -> Self {
        self.icon_after = Some(icon);
        self
    }
    pub fn variant(mut self, v: Variant) -> Self {
        self.variant = v;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }
    pub fn ghost(self) -> Self {
        self.variant(Variant::Ghost)
    }
    pub fn danger(self) -> Self {
        self.variant(Variant::Danger)
    }
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn full_width(mut self) -> Self {
        self.full = true;
        self
    }
    pub fn tooltip(mut self, t: impl Into<SharedString>) -> Self {
        self.tooltip = Some(t.into());
        self
    }
    /// Colours the icon and label (e.g. the accent for AI actions).
    pub fn color(mut self, c: Hsla) -> Self {
        self.color = Some(c);
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = cx.theme().clone();
        let icon_only = self.label.is_none();
        let h = if self.small { 24. } else { 30. };
        let (bg, fg, border, hover) = match self.variant {
            Variant::Primary => (t.accent, t.text_on_accent, t.accent, t.accent_hover),
            Variant::Secondary => (t.hover, t.text, t.line_strong, t.pressed),
            Variant::Ghost => (gpui::transparent_black(), t.text_2, gpui::transparent_black(), t.hover),
            Variant::Danger => (t.danger.opacity(0.14), t.danger, t.danger.opacity(0.35), t.danger.opacity(0.22)),
        };
        let fg = if self.selected && self.variant == Variant::Ghost { t.accent_text } else { self.color.unwrap_or(fg) };
        let bg = if self.selected { t.accent_soft } else { bg };
        let mut b = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(h))
            .when(icon_only, |d| d.w(px(h)))
            .when(!icon_only, |d| d.px(px(if self.small { 8. } else { 12. })))
            .when(self.full, |d| d.w_full())
            .rounded(px(sz::R_SM))
            .bg(bg)
            .border_1()
            .border_color(border)
            .text_color(fg)
            .text_size(px(if self.small { sz::SM } else { sz::BASE }))
            .font_weight(if self.variant == Variant::Primary { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .when_some(self.icon, |d, i| d.child(icon(i).text_color(fg)))
            .when_some(self.label, |d, l| d.child(l))
            .when_some(self.icon_after, |d, i| d.child(icon(i).text_color(fg)));
        if self.disabled {
            b = b.opacity(0.45).cursor_not_allowed();
        } else {
            b = b.cursor_pointer().hover(move |s| s.bg(hover)).active(|s| s.opacity(0.85));
            if let Some(f) = self.on_click {
                b = b.on_click(move |e, w, cx| {
                    cx.stop_propagation();
                    f(e, w, cx)
                });
            }
        }
        if let Some(tip) = self.tooltip {
            b = b.tooltip(move |_, cx| tooltip(tip.clone(), cx));
        }
        b
    }
}

pub struct Tooltip {
    text: SharedString,
}

impl gpui::Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(sz::R_SM))
            .glass(t.glass2)
            .shadow(t.glass_shadow())
            .text_size(px(sz::SM))
            .text_color(t.text)
            .child(self.text.clone())
    }
}

pub fn tooltip(text: SharedString, cx: &mut App) -> AnyView {
    cx.new(|_| Tooltip { text }).into()
}

/// A row of mutually exclusive choices.
pub fn segmented<T: Clone + PartialEq + 'static>(
    id: impl Into<SharedString>,
    options: Vec<(T, SharedString)>,
    value: T,
    on_change: impl Fn(&T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.theme().clone();
    let id: SharedString = id.into();
    let on_change = Rc::new(on_change);
    div()
        .id(ElementId::Name(id.clone()))
        .flex()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(sz::R_SM + 2.))
        .bg(t.bg_sunken.opacity(0.6))
        .border_1()
        .border_color(t.line)
        .children(options.into_iter().enumerate().map(move |(i, (v, label))| {
            let selected = v == value;
            let on_change = on_change.clone();
            div()
                .id((id.clone(), i))
                .flex_1()
                .flex()
                .justify_center()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(sz::R_SM))
                .text_size(px(sz::SM))
                .cursor_pointer()
                .when(selected, |d| d.bg(t.accent_soft).text_color(t.accent_text).font_weight(FontWeight::SEMIBOLD))
                .when(!selected, |d| d.text_color(t.text_2).hover(|s| s.bg(t.hover)))
                .child(label)
                .on_click(move |_, w, cx| on_change(&v, w, cx))
        }))
}

/// An on/off switch with a label.
pub fn switch(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool, on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static, cx: &App) -> Stateful<Div> {
    let t = cx.theme().clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .cursor_pointer()
        .text_size(px(sz::BASE))
        .text_color(t.text)
        .child(label.into())
        .child(
            div()
                .flex_none()
                .w(px(30.))
                .h(px(18.))
                .rounded_full()
                .p(px(2.))
                .bg(if on { t.accent } else { t.line_strong })
                .child(div().size(px(14.)).rounded_full().bg(gpui::white()).when(on, |d| d.ml(px(12.)))),
        )
        .on_click(move |_, w, cx| on_toggle(!on, w, cx))
}

/// A labelled row: label on the left, control on the right.
pub fn field_row(label_text: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    div().flex().items_center().justify_between().gap(px(12.)).min_h(px(28.)).child(label(label_text, cx)).child(control)
}

/// Wraps a child so clicks inside don't reach what is behind it.
pub fn stop(el: impl IntoElement) -> AnyElement {
    div().on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(el).into_any_element()
}

/// `1:05.25` style time (minutes:seconds.hundredths).
pub fn timecode(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - m as f64 * 60.0;
    format!("{m}:{s:05.2}")
}

/// `00:01:05:12` style timecode at `fps`.
pub fn smpte(t: f64, fps: f64) -> String {
    let fps = fps.max(1.0);
    let total = (t.max(0.0) * fps).round() as u64;
    let f = total % fps.round() as u64;
    let secs = total / fps.round() as u64;
    format!("{:02}:{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60, f)
}
