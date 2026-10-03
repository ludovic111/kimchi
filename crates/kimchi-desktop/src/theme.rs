//! The lsuite design system, read from `assets/tokens.json` (a copy of
//! `lsuite/design/tokens.json`; never hard-code a value that lives there).
//!
//! kimchi's signature colour is chili coral (hue 32°): accent `#f7806a` in the
//! dark mode, `#c3513d` in the light one. The chrome sits on three glass tiers
//! over a backdrop tinted with that colour; the work (preview canvas, timeline)
//! stays solid. GPUI has no per-element backdrop blur, so the tiers are the
//! token translucencies over the window's own backdrop, on top of the native
//! window blur on macOS; with transparency reduced every tier is opaque.

use std::sync::OnceLock;

use gpui::{App, BoxShadow, Global, Hsla, Rgba, WindowAppearance, point, px};
use serde_json::Value;

const TOKENS: &str = include_str!("../assets/tokens.json");

fn tokens() -> &'static Value {
    static T: OnceLock<Value> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str(TOKENS).expect("tokens.json"))
}

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

fn tok(path: &str) -> &'static str {
    let mut v = tokens();
    for p in path.split('.') {
        v = &v[p];
    }
    v.as_str().unwrap_or_else(|| panic!("token {path}"))
}

fn color(path: &str) -> Hsla {
    parse_color(tok(path))
}


/// The kimchi scale (`--ls-kimchi-50 … -950`).
pub fn kimchi(step: &str) -> Hsla {
    color(&format!("apps.kimchi.scale.{step}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

/// One glass tier: fill, plus the edge, highlight and shadow every tier shares.
#[derive(Clone, Copy, Debug)]
pub struct Glass {
    pub bg: Hsla,
    pub edge: Hsla,
    pub highlight: Hsla,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub mode: Mode,
    /// Glass tiers are drawn translucent (false: every tier opaque).
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
    pub aurora_a: Hsla,
    pub aurora_b: Hsla,
    pub aurora_strength: f32,

    pub glass1: Glass,
    pub glass2: Glass,
    pub glass3: Glass,
    pub scrim: Hsla,
    pub opaque: Hsla,

    /// Hover and pressed fills for quiet controls.
    pub hover: Hsla,
    pub pressed: Hsla,
    /// Timeline clip colours by content.
    pub clip_video: Hsla,
    pub clip_audio: Hsla,
    pub clip_text: Hsla,
    pub clip_generated: Hsla,
    /// Motion graphics and 3D clips.
    pub clip_motion: Hsla,
}

impl Global for Theme {}

pub mod size {
    //! Type scale, radii and spacing (`--ls-*`), in pixels.
    pub const XS: f32 = 11.0;
    pub const SM: f32 = 12.0;
    pub const BASE: f32 = 13.0;
    pub const MD: f32 = 15.0;
    pub const LG: f32 = 17.0;
    pub const XL: f32 = 22.0;
    pub const XXL: f32 = 28.0;

    pub const R_XS: f32 = 4.0;
    pub const R_SM: f32 = 6.0;
    pub const R_MD: f32 = 10.0;
    pub const R_LG: f32 = 14.0;
    pub const R_XL: f32 = 20.0;
}

pub const SANS: &str = "Manrope";
pub const MONO: &str = "IBM Plex Mono";

impl Theme {
    pub fn new(mode: Mode, transparent: bool) -> Self {
        let m = match mode {
            Mode::Dark => "dark",
            Mode::Light => "light",
        };
        let n = |k: &str| color(&format!("neutral.{m}.{k}"));
        let g = |k: &str| color(&format!("glass.{m}.{k}"));
        let glass = |tier: &str| Glass { bg: color(&format!("glass.{m}.{tier}.bg")), edge: g("edge"), highlight: g("highlight") };
        let (accent, hover, text, soft_a, ring_src, ring_a, aurora_b) = match mode {
            Mode::Dark => (kimchi("400"), kimchi("300"), kimchi("300"), 0.18, kimchi("300"), 0.60, kimchi("700")),
            Mode::Light => (kimchi("600"), kimchi("700"), kimchi("700"), 0.14, kimchi("600"), 0.50, kimchi("200")),
        };
        let opaque = g("opaque");
        let (hover_fill, pressed_fill) = match mode {
            Mode::Dark => (parse_color("rgba(255,255,255,0.06)"), parse_color("rgba(255,255,255,0.10)")),
            Mode::Light => (parse_color("rgba(15,20,30,0.05)"), parse_color("rgba(15,20,30,0.09)")),
        };
        let mut t = Theme {
            mode,
            transparent,
            bg: n("bg"),
            bg_raised: n("bg-raised"),
            bg_sunken: n("bg-sunken"),
            text: n("text"),
            text_2: n("text-2"),
            text_3: n("text-3"),
            text_on_accent: n("text-on-accent"),
            line: n("line"),
            line_strong: n("line-strong"),
            danger: n("danger"),
            warning: n("warning"),
            success: n("success"),
            accent,
            accent_hover: hover,
            accent_text: text,
            accent_soft: accent.opacity(soft_a),
            accent_ring: ring_src.opacity(ring_a),
            aurora_a: kimchi("400"),
            aurora_b,
            aurora_strength: tok(&format!("glass.{m}.aurora")).parse().unwrap_or(0.16),
            glass1: glass("1"),
            glass2: glass("2"),
            glass3: glass("3"),
            scrim: g("scrim"),
            opaque,
            hover: hover_fill,
            pressed: pressed_fill,
            // Track content colours: neutrals derived from the lsuite ones, the
            // accent only for what the person or the agent made (generated).
            clip_video: parse_color(if mode == Mode::Dark { "#2b3140" } else { "#cdd5e4" }),
            clip_audio: parse_color(if mode == Mode::Dark { "#23362f" } else { "#cfe6dc" }),
            clip_text: parse_color(if mode == Mode::Dark { "#3a3045" } else { "#e2d8ee" }),
            clip_generated: kimchi(if mode == Mode::Dark { "800" } else { "200" }),
            clip_motion: parse_color(if mode == Mode::Dark { "#20393d" } else { "#cde5e6" }),
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

    /// `--ls-glass-shadow`.
    pub fn glass_shadow(&self) -> Vec<BoxShadow> {
        let (a, b) = match self.mode {
            Mode::Dark => (parse_color("rgba(0,0,0,0.45)"), parse_color("rgba(0,0,0,0.4)")),
            Mode::Light => (parse_color("rgba(20,30,50,0.14)"), parse_color("rgba(20,30,50,0.10)")),
        };
        vec![
            self.glass_highlight(),
            BoxShadow { color: a, offset: point(px(0.), px(12.)), blur_radius: px(40.), spread_radius: px(0.), inset: false },
            BoxShadow { color: b, offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false },
        ]
    }

    /// `--ls-glass-highlight`: the 1 px light along a glass surface's top edge.
    pub fn glass_highlight(&self) -> BoxShadow {
        BoxShadow { color: self.glass1.highlight, offset: point(px(0.), px(1.)), blur_radius: px(0.), spread_radius: px(0.), inset: true }
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
                // The backdrop: the page colour, and the page under the strongest aurora glow.
                let glow = over(t.aurora_a.opacity(t.aurora_strength), t.bg);
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
