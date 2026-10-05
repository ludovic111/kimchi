//! The audio controls' drawing: level meters (peak, RMS, hold, clip light), the fader's law
//! and scale, and the knob. They only paint; the view that shows them handles the pointer
//! (its drags go through `ui::drag::track`), so a drag is the view's own retained state.

use gpui::{Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled, canvas, fill, point, px, size};
use kimchi_core::audio::{MAX_DB, MIN_DB, db_to_gain, gain_to_db};

use crate::theme::Theme;

/// Where a fader sits for a level: a cube-root law over the gain, so the travel is spread
/// like a console's (0 dB about two thirds up, -6 dB half way, -48 dB near the bottom).
pub fn db_to_pos(db: f64) -> f32 {
    if db <= MIN_DB {
        return 0.0;
    }
    (db_to_gain(db).cbrt() / db_to_gain(MAX_DB).cbrt()).clamp(0.0, 1.0) as f32
}

/// The level of a fader position, to a tenth of a dB.
pub fn pos_to_db(pos: f32) -> f64 {
    if pos <= 0.002 {
        return MIN_DB;
    }
    let g = (pos.clamp(0.0, 1.0) as f64 * db_to_gain(MAX_DB).cbrt()).powi(3);
    (gain_to_db(g) * 10.0).round() / 10.0
}

/// The fader's scale marks.
pub const SCALE: [f64; 8] = [12.0, 6.0, 0.0, -6.0, -12.0, -24.0, -48.0, MIN_DB];

/// "+3.0", "0.0", "-12.5", "-∞".
pub fn db_text(db: f64) -> String {
    if db <= MIN_DB + 0.05 {
        "-∞".into()
    } else if db > 0.05 {
        format!("+{db:.1}")
    } else {
        format!("{:.1}", if db.abs() < 0.05 { 0.0 } else { db })
    }
}

/// A scale mark's label ("+12", "0", "-∞").
pub fn scale_text(db: f64) -> String {
    if db <= MIN_DB {
        "-∞".into()
    } else if db > 0.0 {
        format!("+{db:.0}")
    } else {
        format!("{db:.0}")
    }
}

/// Lowest level a meter shows.
pub const METER_FLOOR: f64 = -60.0;
/// Highest (over full scale shows as clipping).
pub const METER_TOP: f64 = 6.0;

/// Where a level sits on a meter, 0 (floor) to 1 (+6 dBFS).
pub fn meter_pos(db: f64) -> f32 {
    ((db - METER_FLOOR) / (METER_TOP - METER_FLOOR)).clamp(0.0, 1.0) as f32
}

/// "-12 LUFS" style text for a loudness reading ("—" for silence).
pub fn lufs_text(l: f64) -> String {
    if l <= -70.0 { "—".into() } else { format!("{l:.1}") }
}

/// One channel's reading, in dBFS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub peak: f64,
    pub rms: f64,
    /// The highest peak of the last moments (drawn as a line that lingers).
    pub hold: f64,
}

impl Default for Reading {
    fn default() -> Self {
        Self { peak: MIN_DB, rms: MIN_DB, hold: MIN_DB }
    }
}

/// The colour bands of a meter: safe, loud, too loud.
fn bands(t: &Theme) -> [(f64, f64, Hsla); 3] {
    [(METER_FLOOR, -18.0, t.success), (-18.0, -6.0, t.warning), (-6.0, METER_TOP, t.danger)]
}

/// A level meter, one bar per channel: the RMS as the coloured fill, the peak as a bright
/// line above it, the hold as a short line that lingers. Vertical (strips) or horizontal
/// (the toolbar, track headers). `ducking` (dB, negative) draws a mark from the top.
pub fn meter(readings: Vec<Reading>, vertical: bool, t: &Theme) -> impl IntoElement + Styled {
    let t = t.clone();
    canvas(
        |_, _, _| (),
        move |b: Bounds<Pixels>, _, window, _| {
            let n = readings.len().max(1);
            let gap = 1.0;
            let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
            for (ch, r) in readings.iter().enumerate() {
                // The channel's own lane.
                let lane = if vertical {
                    let lw = (w - gap * (n - 1) as f32) / n as f32;
                    Bounds::new(point(b.origin.x + px(ch as f32 * (lw + gap)), b.origin.y), size(px(lw), px(h)))
                } else {
                    let lh = (h - gap * (n - 1) as f32) / n as f32;
                    Bounds::new(point(b.origin.x, b.origin.y + px(ch as f32 * (lh + gap))), size(px(w), px(lh)))
                };
                window.paint_quad(fill(lane, t.bg_sunken).corner_radii(px(1.5)));
                let span = |from: f32, to: f32| -> Bounds<Pixels> {
                    // `from`..`to` along the meter, 0 at the floor.
                    if vertical {
                        let lh = f32::from(lane.size.height);
                        Bounds::new(point(lane.origin.x, lane.origin.y + px(lh * (1.0 - to))), size(lane.size.width, px(lh * (to - from))))
                    } else {
                        let lw = f32::from(lane.size.width);
                        Bounds::new(point(lane.origin.x + px(lw * from), lane.origin.y), size(px(lw * (to - from)), lane.size.height))
                    }
                };
                let rms = meter_pos(r.rms);
                let peak = meter_pos(r.peak);
                for (lo, hi, color) in bands(&t) {
                    let (a, z) = (meter_pos(lo), meter_pos(hi));
                    // The peak's reach, dim; the RMS, full.
                    if peak > a {
                        window.paint_quad(fill(span(a, peak.min(z)), color.opacity(0.35)));
                    }
                    if rms > a {
                        window.paint_quad(fill(span(a, rms.min(z)), color));
                    }
                }
                let line = |pos: f32, color: Hsla, window: &mut gpui::Window| {
                    if pos > 0.0 {
                        let thick = 1.5 / if vertical { f32::from(lane.size.height) } else { f32::from(lane.size.width) }.max(1.0);
                        window.paint_quad(fill(span((pos - thick).max(0.0), pos), color));
                    }
                };
                line(meter_pos(r.hold), t.text.opacity(0.8), window);
            }
        },
    )
}

/// The clip light over a meter: lit once a level went over full scale.
pub fn clip_color(clipped: bool, t: &Theme) -> Hsla {
    if clipped { t.danger } else { t.bg_sunken }
}

/// A rotary control's face: 270° of travel, the value as a lit arc (from the middle when
/// `bipolar`: pan), the pointer in the text colour.
pub fn knob(value: f32, bipolar: bool, active: bool, t: &Theme) -> impl IntoElement + Styled {
    let t = t.clone();
    canvas(
        |_, _, _| (),
        move |b: Bounds<Pixels>, _, window, _| {
            let c = b.center();
            let r = f32::from(b.size.width.min(b.size.height)) / 2.0;
            let start = std::f32::consts::PI * 0.75;
            let sweep = std::f32::consts::PI * 1.5;
            stroke(window, &arc(c, r - 1.5, start, start + sweep), 2.0, t.line_strong);
            let v = value.clamp(0.0, 1.0);
            let (a, z) = if bipolar {
                let mid = start + sweep * 0.5;
                let at = start + sweep * v;
                (mid.min(at), mid.max(at))
            } else {
                (start, start + sweep * v)
            };
            if z - a > 0.01 {
                stroke(window, &arc(c, r - 1.5, a, z), 2.0, if active { t.accent_hover } else { t.accent });
            }
            let cap = r - 4.5;
            window.paint_quad(fill(Bounds::centered_at(c, size(px(cap * 2.0), px(cap * 2.0))), t.bg_raised).corner_radii(px(cap)).border_widths(px(1.0)).border_color(t.line_strong));
            let angle = start + sweep * v;
            let at = |k: f32| point(c.x + px(k * angle.cos()), c.y + px(k * angle.sin()));
            stroke(window, &[at(cap * 0.25), at(cap - 1.0)], 1.8, t.text);
        },
    )
}

fn arc(center: Point<Pixels>, r: f32, from: f32, to: f32) -> Vec<Point<Pixels>> {
    let steps = (((to - from).abs() / 0.08).ceil() as usize).max(2);
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * i as f32 / steps as f32;
            point(center.x + px(r * a.cos()), center.y + px(r * a.sin()))
        })
        .collect()
}

fn stroke(window: &mut gpui::Window, points: &[Point<Pixels>], width: f32, color: Hsla) {
    if points.len() < 2 {
        return;
    }
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(points[0]);
    for p in &points[1..] {
        path.line_to(*p);
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// "L 30", "C", "R 100" for a pan from -1 to 1.
pub fn pan_text(pan: f64) -> String {
    let n = (pan * 100.0).round() as i64;
    match n {
        0 => "C".into(),
        n if n < 0 => format!("L{}", -n),
        n => format!("R{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fader_law_round_trips_and_spreads_the_travel() {
        assert_eq!(db_to_pos(MIN_DB), 0.0);
        assert!((db_to_pos(MAX_DB) - 1.0).abs() < 1e-6);
        let zero = db_to_pos(0.0);
        assert!(zero > 0.55 && zero < 0.7, "{zero}");
        for db in [-48.0, -24.0, -6.0, 0.0, 3.5, 12.0] {
            assert!((pos_to_db(db_to_pos(db)) - db).abs() < 0.11, "{db}");
        }
        assert_eq!(pos_to_db(0.0), MIN_DB);
    }

    #[test]
    fn texts() {
        assert_eq!(db_text(MIN_DB), "-∞");
        assert_eq!(db_text(0.02), "0.0");
        assert_eq!(db_text(3.0), "+3.0");
        assert_eq!(pan_text(-0.3), "L30");
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(meter_pos(METER_FLOOR - 10.0), 0.0);
        assert_eq!(meter_pos(10.0), 1.0);
    }
}
