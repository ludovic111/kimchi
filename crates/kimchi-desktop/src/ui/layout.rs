//! The window's layout rules, in one place: the smallest window, the breakpoints, every
//! panel's limits, and [`solve`], which turns the person's panel sizes into the sizes the
//! editor can draw at the window's current size. Views ask here instead of hard-coding
//! widths, so the window works the same way at every size:
//!
//! - every panel has a minimum and a maximum; below its minimum a side panel leaves the
//!   row and becomes a drawer over the work (the left panel keeps its rail of tabs), and
//!   the Agent panel floats over the editor instead of squeezing it;
//! - the preview never gets less than [`PREVIEW_MIN_W`] × [`PREVIEW_MIN_H`], so splitters
//!   are clamped against the window, again each time it shrinks, and the sizes the person
//!   chose come back when it grows (they are kept apart from the drawn ones, and saved);
//! - the timeline's height is a share of the editor's height, so it scales with the window;
//! - dialogs are never larger than the window ([`dialog_size`]) and scroll inside;
//! - very large windows don't stretch reading text: dialogs and home cap their widths.

use std::path::{Path, PathBuf};

use gpui::{Pixels, Size};
use serde::{Deserialize, Serialize};

/// The smallest window: 720 × 480. At that size the editor still shows the top bar, the
/// rail of tabs, the inspector, a preview of about 360 × 200 with its transport, and a
/// timeline with two tracks; the left panel opens as a drawer over the work. 720 is half of a 1440-wide laptop
/// screen (two windows side by side) and 480 the height of the smallest common half-screen.
pub const WINDOW_MIN_W: f32 = 720.;
pub const WINDOW_MIN_H: f32 = 480.;

/// The top bar (title bar) of home and the editor.
pub const TOPBAR_H: f32 = 52.;
/// The left rail of tabs (media, generate, text, motion, captions).
pub const RAIL_W: f32 = 56.;

pub const LEFT_W: f32 = 320.;
pub const LEFT_MIN: f32 = 248.;
pub const LEFT_MAX: f32 = 520.;
pub const INSPECTOR_W: f32 = 300.;
pub const INSPECTOR_MIN: f32 = 256.;
pub const INSPECTOR_MAX: f32 = 460.;
pub const AGENT_W: f32 = 380.;
pub const AGENT_MIN: f32 = 300.;
pub const AGENT_MAX: f32 = 600.;
/// The timeline's share of the editor's height to start with (300 px of a 920 px window).
pub const TIMELINE_SHARE: f32 = 0.35;
pub const TIMELINE_MIN: f32 = 150.;
pub const TIMELINE_MAX: f32 = 900.;
/// The preview always keeps at least this much room (a 16:9 picture about 300 px wide).
pub const PREVIEW_MIN_W: f32 = 340.;
pub const PREVIEW_MIN_H: f32 = 190.;
/// The Agent panel docks only while the editor beside it keeps this much width; below,
/// it floats over the editor.
pub const EDITOR_MIN_BESIDE_AGENT: f32 = 760.;
/// What a splitter between two docked panels takes from the row (5 px wide, 2 px of it
/// over each neighbour).
pub const SPLITTER_W: f32 = 1.;
/// A drawer leaves this much of the window uncovered, so a click outside closes it.
pub const DRAWER_GUTTER: f32 = 48.;

/// Width classes. Views fold secondary controls into menus as the class goes down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Breakpoint {
    /// Under 960 px: side panels are drawers, the top bar keeps its essentials.
    Compact,
    /// 960 to 1279 px.
    Medium,
    /// 1280 to 1919 px.
    Wide,
    /// 1920 px and more.
    Huge,
}

pub fn breakpoint(width: f32) -> Breakpoint {
    match width {
        w if w < 960. => Breakpoint::Compact,
        w if w < 1280. => Breakpoint::Medium,
        w if w < 1920. => Breakpoint::Wide,
        _ => Breakpoint::Huge,
    }
}

/// Dialog widths and the margin they keep from the window's edges.
pub const DIALOG_MARGIN: f32 = 16.;
/// The tallest a dialog gets, however tall the window (reading comfort).
pub const DIALOG_MAX_H: f32 = 820.;

/// The size a dialog that wants `width` (and at most `max_h`) gets in a window of this
/// size: never wider or taller than the window less its margins.
pub fn dialog_size(width: f32, max_h: f32, window: Size<Pixels>) -> (f32, f32) {
    let (ww, wh) = (f32::from(window.width), f32::from(window.height));
    let w = width.min(ww - DIALOG_MARGIN * 2.).max(200.);
    let h = max_h.min(DIALOG_MAX_H).min(wh - DIALOG_MARGIN * 2.).max(160.);
    (w, h)
}

/// The tallest a popover (a list under a button) may be in this window: `want`, or less in a
/// short window, so it stays inside it once `anchored` snaps it to the edges.
pub fn popover_max_h(window: &gpui::Window, want: f32) -> f32 {
    want.min(f32::from(window.viewport_size().height) - 2. * DIALOG_MARGIN - 40.).max(160.)
}

/// What the person chose for the editor's panels: kept as they set them (the editor draws
/// them clamped to the window), saved in `<config>/window-layout.json`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    pub left: f32,
    pub inspector: f32,
    pub agent: f32,
    /// The timeline's height as a share of the editor's height (top bar excluded).
    pub timeline: f32,
    /// The left panel is open (false: only its rail shows).
    pub left_open: bool,
    pub inspector_open: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { left: LEFT_W, inspector: INSPECTOR_W, agent: AGENT_W, timeline: TIMELINE_SHARE, left_open: true, inspector_open: true }
    }
}

impl Prefs {
    fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("window-layout.json")
    }

    /// The saved panel sizes, or the defaults (sizes out of range are pulled back in).
    pub fn load(config_dir: &Path) -> Self {
        let p: Self = std::fs::read(Self::path(config_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        p.sanitized()
    }

    pub fn save(&self, config_dir: &Path) {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(config_dir)?;
            std::fs::write(Self::path(config_dir), serde_json::to_vec_pretty(self).unwrap_or_default())
        };
        if let Err(e) = write() {
            tracing::warn!("couldn't save the panel sizes: {e}");
        }
    }

    /// Every size within its limits (a hand-edited or old file can't break the layout).
    pub fn sanitized(mut self) -> Self {
        let ok = |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
        self.left = ok(self.left, LEFT_MIN, LEFT_MAX, LEFT_W);
        self.inspector = ok(self.inspector, INSPECTOR_MIN, INSPECTOR_MAX, INSPECTOR_W);
        self.agent = ok(self.agent, AGENT_MIN, AGENT_MAX, AGENT_W);
        self.timeline = ok(self.timeline, 0.1, 0.8, TIMELINE_SHARE);
        self
    }
}

/// How a side panel is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dock {
    /// In the row, this wide.
    Docked(f32),
    /// Over the work, this wide (the window is too narrow to dock it).
    Drawer(f32),
    /// Not shown.
    Hidden,
}

impl Dock {
    /// The width it takes in the row.
    pub fn row_width(self) -> f32 {
        match self {
            Dock::Docked(w) => w,
            _ => 0.,
        }
    }

    pub fn width(self) -> f32 {
        match self {
            Dock::Docked(w) | Dock::Drawer(w) => w,
            Dock::Hidden => 0.,
        }
    }

    pub fn is_drawer(self) -> bool {
        matches!(self, Dock::Drawer(_))
    }
}

/// The editor's layout at one window size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solved {
    pub window: (f32, f32),
    pub left: Dock,
    pub inspector: Dock,
    pub agent: Dock,
    pub timeline_h: f32,
    /// The preview column's width (between the docked panels).
    pub center_w: f32,
    /// Whether each side panel could dock at this size (else opening it makes a drawer).
    pub left_fits: bool,
    pub inspector_fits: bool,
}

/// What is open besides the sizes: the Agent panel, and side panels asked for as drawers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Open {
    pub agent: bool,
    pub left_drawer: bool,
    pub inspector_drawer: bool,
}

/// The editor's layout for a window of `window` size (the whole window, top bar included).
pub fn solve(window: Size<Pixels>, prefs: &Prefs, open: Open) -> Solved {
    let (ww, wh) = (f32::from(window.width).max(1.), f32::from(window.height).max(1.));
    let prefs = prefs.sanitized();

    // The Agent panel docks when the editor beside it stays usable, else it floats.
    let agent = if !open.agent {
        Dock::Hidden
    } else {
        let w = prefs.agent.min(ww - EDITOR_MIN_BESIDE_AGENT);
        if w >= AGENT_MIN { Dock::Docked(w) } else { Dock::Drawer(prefs.agent.min(ww - DRAWER_GUTTER).max(AGENT_MIN.min(ww))) }
    };

    // Left panel and inspector share what the rail, the agent and the preview leave.
    let agent_row = if agent.row_width() > 0. { agent.row_width() + SPLITTER_W } else { 0. };
    // Each docked side panel brings a splitter: counted in the room it needs.
    let room = ww - RAIL_W - agent_row - PREVIEW_MIN_W - 2. * SPLITTER_W;
    let mut left = if prefs.left_open { prefs.left } else { 0. };
    let mut insp = if prefs.inspector_open { prefs.inspector } else { 0. };
    let left_min = if prefs.left_open { LEFT_MIN } else { 0. };
    let insp_min = if prefs.inspector_open { INSPECTOR_MIN } else { 0. };
    let (mut left_fits, mut inspector_fits) = (true, true);
    if left + insp > room {
        // First shrink both toward their minimums, in proportion to what each has to give.
        let give = (left - left_min) + (insp - insp_min);
        let over = left + insp - room;
        if give > 0. {
            let k = (over / give).min(1.);
            left -= (left - left_min) * k;
            insp -= (insp - insp_min) * k;
        }
        // Still too wide: the left panel leaves the row first (its rail stays), then the inspector.
        if left + insp > room + 0.5 && prefs.left_open {
            left = 0.;
            left_fits = false;
            insp = if prefs.inspector_open { prefs.inspector.min(room) } else { 0. };
        }
        if prefs.inspector_open && (left + insp > room + 0.5 || insp < INSPECTOR_MIN - 0.5) {
            insp = 0.;
            inspector_fits = false;
        }
    }
    // Whether a closed panel would dock if opened now (else it opens as a drawer).
    if !prefs.left_open {
        left_fits = room - insp >= LEFT_MIN;
    }
    if !prefs.inspector_open {
        inspector_fits = room - left >= INSPECTOR_MIN;
    }
    let drawer_w = |want: f32, min: f32| want.min(ww - DRAWER_GUTTER).max(min.min(ww - DRAWER_GUTTER));
    let left = if prefs.left_open && left_fits {
        Dock::Docked(left)
    } else if open.left_drawer {
        Dock::Drawer(drawer_w(prefs.left, LEFT_MIN))
    } else {
        Dock::Hidden
    };
    let inspector = if prefs.inspector_open && inspector_fits {
        Dock::Docked(insp)
    } else if open.inspector_drawer {
        Dock::Drawer(drawer_w(prefs.inspector, INSPECTOR_MIN))
    } else {
        Dock::Hidden
    };
    let splitters = [left, inspector].iter().filter(|d| d.row_width() > 0.).count() as f32 * SPLITTER_W;
    let center_w = (ww - RAIL_W - agent_row - left.row_width() - inspector.row_width() - splitters).max(0.);

    // The timeline: its share of the height, within its limits and the preview's minimum.
    let body = (wh - TOPBAR_H).max(0.);
    let max_h = (body - PREVIEW_MIN_H).min(TIMELINE_MAX).max(TIMELINE_MIN.min(body * 0.5));
    let timeline_h = (prefs.timeline * body).clamp(TIMELINE_MIN.min(max_h), max_h);
    Solved { window: (ww, wh), left, inspector, agent, timeline_h, center_w, left_fits, inspector_fits }
}

/// The timeline share for a height in pixels, in an editor of this window height.
pub fn timeline_share(height: f32, window_h: f32) -> f32 {
    let body = (window_h - TOPBAR_H).max(1.);
    (height / body).clamp(0.1, 0.8)
}

/// The widest a docked side panel may be dragged in this window, given what else is docked.
pub fn max_side(solved: &Solved, other: f32, max: f32) -> f32 {
    (solved.window.0 - RAIL_W - solved.agent.row_width() - other - PREVIEW_MIN_W - 3. * SPLITTER_W).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, size};

    fn win(w: f32, h: f32) -> Size<Pixels> {
        size(px(w), px(h))
    }

    /// Nothing docked is narrower than its minimum, the row never exceeds the window and the
    /// preview keeps its minimum, at every size from the smallest window to 4K.
    #[test]
    fn panels_fit_at_every_size() {
        for w in (WINDOW_MIN_W as i32..=3840).step_by(37) {
            for h in [480., 600., 768., 1080., 2160.] {
                for agent in [false, true] {
                    for (lo, io) in [(true, true), (false, true), (true, false), (false, false)] {
                        let prefs = Prefs { left_open: lo, inspector_open: io, ..Default::default() };
                        let s = solve(win(w as f32, h), &prefs, Open { agent, ..Default::default() });
                        let docked = [s.left, s.inspector, s.agent].iter().filter(|d| d.row_width() > 0.).count() as f32;
                        let row = RAIL_W + s.left.row_width() + s.inspector.row_width() + s.agent.row_width() + docked * SPLITTER_W + s.center_w;
                        assert!((row - w as f32).abs() < 1., "{w}x{h}: the row is {row}");
                        assert!(s.center_w >= PREVIEW_MIN_W - 0.5, "{w}x{h} agent {agent}: preview {}", s.center_w);
                        if let Dock::Docked(l) = s.left {
                            assert!(l >= LEFT_MIN - 0.5, "{w}: left {l}");
                        }
                        if let Dock::Docked(i) = s.inspector {
                            assert!(i >= INSPECTOR_MIN - 0.5, "{w}: inspector {i}");
                        }
                        if let Dock::Docked(a) = s.agent {
                            assert!(a >= AGENT_MIN && w as f32 - a >= EDITOR_MIN_BESIDE_AGENT - 0.5);
                        }
                        assert!(s.timeline_h >= TIMELINE_MIN.min(h - TOPBAR_H) - 0.5);
                        assert!(h - TOPBAR_H - s.timeline_h >= PREVIEW_MIN_H - 0.5, "{h}: timeline {}", s.timeline_h);
                    }
                }
            }
        }
    }

    #[test]
    fn narrow_windows_turn_panels_into_drawers_and_wide_ones_dock_them() {
        let prefs = Prefs::default();
        let wide = solve(win(1480., 920.), &prefs, Open::default());
        assert_eq!((wide.left, wide.inspector), (Dock::Docked(LEFT_W), Dock::Docked(INSPECTOR_W)));
        let small = solve(win(720., 480.), &prefs, Open::default());
        assert_eq!((small.left, small.inspector), (Dock::Hidden, Dock::Docked(INSPECTOR_W)));
        assert!(!small.left_fits && small.inspector_fits);
        // With the agent docked beside nothing else fits: both side panels are drawers.
        let busy = solve(win(1180., 800.), &prefs, Open { agent: true, ..Default::default() });
        assert!(busy.agent.row_width() > 0. && busy.center_w >= PREVIEW_MIN_W);
        let drawer = solve(win(720., 480.), &prefs, Open { left_drawer: true, ..Default::default() });
        assert!(drawer.left.is_drawer() && drawer.left.width() <= 720. - DRAWER_GUTTER);
        // The agent floats over a small window and docks beside a large one.
        assert!(solve(win(1024., 700.), &prefs, Open { agent: true, ..Default::default() }).agent.is_drawer());
        assert_eq!(solve(win(1920., 1080.), &prefs, Open { agent: true, ..Default::default() }).agent, Dock::Docked(AGENT_W));
    }

    #[test]
    fn shrinking_then_growing_gives_back_the_chosen_sizes() {
        let prefs = Prefs { left: 480., inspector: 420., ..Default::default() };
        let small = solve(win(1100., 700.), &prefs, Open::default());
        assert!(small.left.row_width() < 480.);
        let big = solve(win(2560., 1440.), &prefs, Open::default());
        assert_eq!((big.left, big.inspector), (Dock::Docked(480.), Dock::Docked(420.)));
        // The timeline keeps its share.
        assert!((big.timeline_h - prefs.timeline * (1440. - TOPBAR_H)).abs() < 1.);
    }

    #[test]
    fn dialogs_stay_inside_the_window() {
        let (w, h) = dialog_size(900., 760., win(720., 480.));
        assert!(w <= 720. - 2. * DIALOG_MARGIN && h <= 480. - 2. * DIALOG_MARGIN);
        assert_eq!(dialog_size(520., 700., win(3840., 2160.)), (520., 700.));
    }

    #[test]
    fn saved_sizes_are_pulled_back_within_limits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("window-layout.json"), r#"{"left": 99999, "timeline": -3}"#).unwrap();
        let p = Prefs::load(dir.path());
        assert_eq!((p.left, p.timeline), (LEFT_MAX, 0.1));
        let mut q = p;
        q.inspector = 333.;
        q.save(dir.path());
        assert_eq!(Prefs::load(dir.path()).inspector, 333.);
    }
}
