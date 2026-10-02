//! A colour field: a swatch, the hex code to type into, and a row of ready colours.

use gpui::{Context, Entity, EventEmitter, Subscription, Window, div, prelude::*, px};

use crate::theme::{ActiveTheme, parse_color, size as sz};
use crate::ui::input::{InputEvent, TextInput};

/// Ready colours for titles, cards and backgrounds. These are picture colours (what ends up in
/// the video), not interface colours, so they don't come from the design tokens.
pub const SWATCHES: [&str; 12] =
    ["#ffffff", "#d9d4cf", "#8a8580", "#1c1a19", "#000000", "#f7806a", "#e5484d", "#f0b44c", "#ffe066", "#7bd88f", "#5cc8ff", "#9b8cff"];

/// The colour changed (`#rrggbb`, or `#rrggbbaa` when the field keeps alpha).
pub struct ColorChange(pub String);

pub struct ColorField {
    value: String,
    input: Entity<TextInput>,
    /// Typed or picked colours get this alpha suffix when they have none (e.g. `cc` for a text box).
    alpha: Option<&'static str>,
    _sub: Subscription,
}

impl EventEmitter<ColorChange> for ColorField {}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` (with or without `#`), lowercase with `#`; `None` if it isn't one.
pub fn normalize(s: &str) -> Option<String> {
    let hex = s.trim().trim_start_matches('#');
    if !matches!(hex.len(), 3 | 6 | 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let hex = if hex.len() == 3 { hex.chars().flat_map(|c| [c, c]).collect() } else { hex.to_string() };
    Some(format!("#{}", hex.to_lowercase()))
}

impl ColorField {
    pub fn new(alpha: Option<&'static str>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx).placeholder("#rrggbb");
            i.mono = true;
            i
        });
        let sub = cx.subscribe(&input, |this: &mut Self, input, e: &InputEvent, cx| match e {
            InputEvent::Submit | InputEvent::Blur => {
                let text = input.read(cx).text().to_string();
                match normalize(&text) {
                    Some(c) => this.commit(c, cx),
                    None => {
                        // Not a colour: show the current one again.
                        let v = this.value.clone();
                        input.update(cx, |i, cx| i.set_text(v, cx));
                    }
                }
            }
            InputEvent::Cancel => {
                let v = this.value.clone();
                input.update(cx, |i, cx| i.set_text(v, cx));
            }
            InputEvent::Changed(_) => {}
        });
        Self { value: String::new(), input, alpha, _sub: sub }
    }

    /// Shows `v` (from the project) unless the code is being typed.
    pub fn set_value(&mut self, v: &str, window: &Window, cx: &mut Context<Self>) {
        if self.value != v {
            self.value = v.to_string();
            cx.notify();
        }
        if !self.input.read(cx).is_focused(window) {
            let v = v.to_string();
            self.input.update(cx, |i, cx| i.set_text(v, cx));
        }
    }

    fn commit(&mut self, mut c: String, cx: &mut Context<Self>) {
        if let Some(a) = self.alpha
            && c.len() == 7
        {
            c.push_str(a);
        }
        if c != self.value {
            self.value = c.clone();
            cx.emit(ColorChange(c.clone()));
        }
        self.input.update(cx, |i, cx| i.set_text(c, cx));
        cx.notify();
    }
}

impl gpui::Render for ColorField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let current = normalize(&self.value).unwrap_or_default();
        let rgb = current.get(..7).unwrap_or("").to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        // Checkerboard-free swatch: the colour over the sunken surface, edged so white and black both show.
                        div().flex_none().size(px(28.)).rounded(px(sz::R_SM)).border_1().border_color(t.line_strong).bg(t.bg_sunken).child(
                            div().size_full().rounded(px(sz::R_SM - 1.)).when(!current.is_empty(), |d| d.bg(parse_color(&current))),
                        ),
                    )
                    .child(div().flex_1().min_w_0().child(self.input.clone())),
            )
            .child(div().flex().flex_wrap().gap(px(4.)).children(SWATCHES.iter().enumerate().map(|(i, s)| {
                let selected = rgb == *s;
                let c = s.to_string();
                div()
                    .id(("swatch", i))
                    .size(px(18.))
                    .rounded_full()
                    .bg(parse_color(s))
                    .border_1()
                    .border_color(t.line_strong)
                    .cursor_pointer()
                    .when(selected, |d| d.border_2().border_color(t.accent))
                    .hover(|d| d.border_color(t.accent_ring))
                    .tooltip({
                        let s: gpui::SharedString = (*s).into();
                        move |_, cx| crate::ui::tooltip(s.clone(), cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.commit(c.clone(), cx)))
            })))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalizes_hex() {
        assert_eq!(normalize("#FFF").as_deref(), Some("#ffffff"));
        assert_eq!(normalize("12ab34").as_deref(), Some("#12ab34"));
        assert_eq!(normalize("#000000cc").as_deref(), Some("#000000cc"));
        assert_eq!(normalize("red"), None);
        assert_eq!(normalize("#12345"), None);
    }
}
