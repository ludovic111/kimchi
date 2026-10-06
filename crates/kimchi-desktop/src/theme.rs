//! kimchi's look: black and white, hard edges, grain.
//!
//! Everything is a grey between black and white; the accent is white in the dark mode and black
//! in the light one, and the only colour left is red for what destroys or records. Corners are
//! square (the radii below are zero or close to it), floating surfaces cast a hard offset shadow
//! instead of a soft one, and the page behind the chrome is film grain and dithered light
//! (`ui::grain`). The chrome sits on three tiers over that page; the work (preview canvas,
//! timeline) stays solid. GPUI has no per-element backdrop blur, so tier 1 is translucent over the
//! window's own backdrop and tiers 2–3 are their tint over the raised surface; with transparency
//! reduced every tier is opaque.
//!
//! `assets/tokens.json` is the lsuite copy; kimchi's palette no longer reads its colours.

use gpui::{App, BoxShadow, Global, Hsla, Rgba, WindowAppearance, point, px};

/// `#rrggbb`, `#rrggbbaa` or `rgba(r,g,b,a)`.
pub fn parse_color(s: &str) -> Hsla {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let v = u32::from_str_radix(hex, 16).unwrap_or(0);
        let rgba = match hex.len() {
            6 => Rgba { r: ((v >> 16) & 255) as f32 / 255.0, g: ((v >> 8) & 255) as f32 / 255.0, b: (v & 255) as f32 / 255.0, a: 1.0 },
            8 => Rgba { r: ((v >> 24) & 255) as f32 / 255.0, g: ((v >> 16) & 255) as f32 / 255.0, b: ((v >> 8) & 255) as f32 / 255.0, a: (v & 255) as f32 / 255.0 },
            _ => Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
        };
        return rgba.into();
    }
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let p: Vec<f32> = inner.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if p.len() == 4 {
            return Rgba { r: p[0] / 255.0, g: p[1] / 255.0, b: p[2] / 255.0, a: p[3] }.into();
        }
    }
    Rgba { r: 1.0, g: 0.0, b: 1.0, a: 1.0 }.into()
}

/// A grey: 0 is black, 1 is white.
pub fn grey(v: f32) -> Hsla {
    Rgba { r: v, g: v, b: v, a: 1.0 }.into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

/// One tier: fill, plus the edge and highlight every tier shares.
#[derive(Clone, Copy, Debug)]
pub struct Glass {
    pub bg: Hsla,
    pub edge: Hsla,
    pub highlight: Hsla,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub mode: Mode,
    /// Tiers are drawn translucent (false: every tier opaque).
    pub transparent: bool,

    pub bg: Hsla,
    pub bg_raised: Hsla,
    pub bg_sunken: Hsla,
    pub text: Hsla,
    pub text_2: Hsla,
    pub text_3: Hsla,
    pub text_on_accent: Hsla,
    pub line: Hsla,
    pub line_strong: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,

    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_text: Hsla,
    pub accent_soft: Hsla,
    pub accent_ring: Hsla,
    /// The ink of the grain and the dithered light on the page (white or black), and how strong.
    pub ink: Hsla,
    pub grain: f32,
    pub dither: f32,

    pub glass1: Glass,
    pub glass2: Glass,
    pub glass3: Glass,
    pub scrim: Hsla,
    pub opaque: Hsla,
    /// The hard offset shadow under floating surfaces.
    pub drop: Hsla,

    /// Hover and pressed fills for quiet controls.
    pub hover: Hsla,
    pub pressed: Hsla,
    /// Timeline clip greys by content.
    pub clip_video: Hsla,
    pub clip_audio: Hsla,
    pub clip_text: Hsla,
    pub clip_generated: Hsla,
    /// Motion graphics and 3D clips.
    pub clip_motion: Hsla,
}

impl Global for Theme {}

pub mod size {
    //! Type scale, radii and spacing, in pixels. Corners are square: the radii are kept as names
    //! so every surface stays on the scale, but they are (almost) zero.
    pub const XS: f32 = 11.0;
    pub const SM: f32 = 12.0;
    pub const BASE: f32 = 13.0;
    pub const MD: f32 = 15.0;
    pub const LG: f32 = 17.0;
    pub const XL: f32 = 22.0;
    pub const XXL: f32 = 28.0;

    pub const R_XS: f32 = 0.0;
    pub const R_SM: f32 = 0.0;
    pub const R_MD: f32 = 0.0;
    pub const R_LG: f32 = 0.0;
    pub const R_XL: f32 = 0.0;
}

/// The interface face: Chakra Petch, cut corners on every letter (bundled, OFL).
pub const SANS: &str = "Chakra Petch";
pub const MONO: &str = "IBM Plex Mono";

impl Theme {
    pub fn new(mode: Mode, transparent: bool) -> Self {
        let c = parse_color;
        let dark = mode == Mode::Dark;
        let (ink, paper) = if dark { (gpui::white(), gpui::black()) } else { (gpui::black(), gpui::white()) };
        let edge = ink.opacity(if dark { 0.16 } else { 0.22 });
        let glass = |bg: &str| Glass { bg: c(bg), edge, highlight: gpui::transparent_black() };
        let mut t = if dark {
            Theme {
                mode,
                transparent,
                bg: grey(0.02),
                bg_raised: grey(0.055),
                bg_sunken: grey(0.0),
                text: grey(0.95),
                text_2: grey(0.66),
                text_3: grey(0.44),
                text_on_accent: paper,
                line: ink.opacity(0.10),
                line_strong: ink.opacity(0.20),
                danger: c("#ff5b4d"),
                warning: grey(0.85),
                success: grey(0.95),
                accent: ink,
                accent_hover: grey(0.82),
                accent_text: ink,
                accent_soft: ink.opacity(0.12),
                accent_ring: ink.opacity(0.62),
                ink,
                grain: 0.07,
                dither: 0.13,
                glass1: glass("rgba(8,8,8,0.62)"),
                glass2: glass("rgba(14,14,14,0.92)"),
                glass3: glass("rgba(18,18,18,0.96)"),
                scrim: c("rgba(0,0,0,0.62)"),
                opaque: grey(0.06),
                drop: ink.opacity(0.11),
                hover: ink.opacity(0.07),
                pressed: ink.opacity(0.12),
                clip_video: grey(0.24),
                clip_audio: grey(0.13),
                clip_text: grey(0.32),
                clip_generated: grey(0.40),
                clip_motion: grey(0.19),
            }
        } else {
            Theme {
                mode,
                transparent,
                bg: grey(0.94),
                bg_raised: grey(0.985),
                bg_sunken: grey(0.89),
                text: grey(0.04),
                text_2: grey(0.30),
                text_3: grey(0.50),
                text_on_accent: paper,
                line: ink.opacity(0.12),
                line_strong: ink.opacity(0.26),
                danger: c("#c8291c"),
                warning: grey(0.20),
                success: grey(0.04),
                accent: ink,
                accent_hover: grey(0.22),
                accent_text: ink,
                accent_soft: ink.opacity(0.09),
                accent_ring: ink.opacity(0.62),
                ink,
                grain: 0.09,
                dither: 0.11,
                glass1: glass("rgba(250,250,250,0.62)"),
                glass2: glass("rgba(252,252,252,0.94)"),
                glass3: glass("rgba(255,255,255,0.97)"),
                scrim: c("rgba(235,235,235,0.62)"),
                opaque: grey(0.97),
                drop: ink.opacity(0.85),
                hover: ink.opacity(0.05),
                pressed: ink.opacity(0.10),
                clip_video: grey(0.74),
                clip_audio: grey(0.84),
                clip_text: grey(0.66),
                clip_generated: grey(0.58),
                clip_motion: grey(0.79),
            }
        };
        if transparent {
            // GPUI can't blur what is behind an element, so floating tiers (menus, popovers,
            // dialogs) would let busy content show through. Their tint is laid over the raised
            // surface instead: the same colour, readable over anything. The chrome (tier 1) sits
            // on the window's own backdrop and stays translucent.
            for tier in [&mut t.glass2, &mut t.glass3] {
                tier.bg = over(tier.bg, t.bg_raised);
            }
        } else {
            for tier in [&mut t.glass1, &mut t.glass2, &mut t.glass3] {
                tier.bg = t.opaque;
            }
        }
        t
    }

    pub fn is_dark(&self) -> bool {
        self.mode == Mode::Dark
    }

    /// Under floating surfaces: a hard shadow, offset down and right, no blur (and a soft dark one
    /// in the dark mode, where the hard one is light and would not separate the surface alone).
    pub fn glass_shadow(&self) -> Vec<BoxShadow> {
        let hard = BoxShadow { color: self.drop, offset: point(px(4.), px(4.)), blur_radius: px(0.), spread_radius: px(0.), inset: false };
        if self.is_dark() {
            vec![BoxShadow { color: gpui::black().opacity(0.7), offset: point(px(0.), px(10.)), blur_radius: px(30.), spread_radius: px(0.), inset: false }, hard]
        } else {
            vec![hard]
        }
    }

    /// The smaller hard shadow under buttons and chips that stand out (the primary action).
    pub fn chip_shadow(&self) -> Vec<BoxShadow> {
        let color = if self.is_dark() { self.ink.opacity(0.22) } else { self.ink.opacity(0.9) };
        vec![BoxShadow { color, offset: point(px(2.), px(2.)), blur_radius: px(0.), spread_radius: px(0.), inset: false }]
    }

    /// Mode from the setting (`system`, `dark`, `light`) and the OS appearance.
    pub fn mode_for(setting: &str, appearance: WindowAppearance) -> Mode {
        match setting {
            "dark" => Mode::Dark,
            "light" => Mode::Light,
            _ => match appearance {
                WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
            },
        }
    }
}

/// `top` composited over an opaque `bottom`.
pub fn over(top: Hsla, bottom: Hsla) -> Hsla {
    let (t, b): (Rgba, Rgba) = (top.into(), bottom.into());
    let a = t.a;
    Rgba { r: t.r * a + b.r * (1.0 - a), g: t.g * a + b.g * (1.0 - a), b: t.b * a + b.b * (1.0 - a), a: 1.0 }.into()
}

/// The theme in use (`cx.theme()`).
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

/// Reads the OS "reduce transparency" preference (macOS).
pub fn os_reduces_transparency() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.universalaccess", "reduceTransparency"])
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Whether the person asked the OS for less motion (animations then show their end state).
pub fn os_reduces_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.universalaccess", "reduceMotion"])
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "false")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Hsla) -> f32 {
        let c: Rgba = c.into();
        let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }


    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Text contrast on every surface, including each glass tier over the
    /// brightest and darkest backdrop the window can show, in both modes
    /// (lsuite DESIGN.md › Accessibility). Fix the tier or the colour step,
    /// never the threshold.
    #[test]
    fn text_contrast_holds_on_every_surface() {
        for mode in [Mode::Dark, Mode::Light] {
            for transparent in [true, false] {
                let t = Theme::new(mode, transparent);
                // The backdrop: the page colour, and the page under the densest dither and grain.
                let glow = over(t.ink.opacity(t.dither + t.grain), t.bg);
                let backdrops = [t.bg, glow, t.bg_raised, t.bg_sunken];
                let mut surfaces = vec![t.bg, t.bg_raised, t.bg_sunken];
                for b in backdrops {
                    for g in [t.glass1, t.glass2, t.glass3] {
                        surfaces.push(over(g.bg, b));
                    }
                }
                for s in &surfaces {
                    assert!(contrast(t.text, *s) >= 4.5, "{mode:?} text on {s:?}: {}", contrast(t.text, *s));
                    assert!(contrast(t.text_2, *s) >= 4.5, "{mode:?} text-2 on {s:?}: {}", contrast(t.text_2, *s));
                    // Accent text and icons: at least 3:1 (large text, icons), 4.5 on the page.
                    assert!(contrast(t.accent_text, *s) >= 3.0, "{mode:?} accent on {s:?}: {}", contrast(t.accent_text, *s));
                }
                assert!(contrast(t.accent_text, t.bg) >= 4.5, "{mode:?} accent text on the page");
                assert!(contrast(t.text_on_accent, t.accent) >= 4.5, "{mode:?} text on accent: {}", contrast(t.text_on_accent, t.accent));
            }
        }
    }
}
