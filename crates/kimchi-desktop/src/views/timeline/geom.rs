//! Timeline geometry: sizes, track rows, ruler ticks, snapping and the local
//! preview of a trim (mirrors `Project::trim` in kimchi-core so the clip under
//! the pointer looks exactly like what the command will make).

use kimchi_core::{Asset, Clip, Id, MIN_CLIP, Project, Track, TrackKind};

pub const HEADER_W: f32 = 172.;
pub const RULER_H: f32 = 30.;
pub const TOOLBAR_H: f32 = 44.;
/// Space between two lanes (each lane sits 2 px into its row).
pub const TRACK_GAP: f32 = 4.;
/// Snap distance, in screen pixels.
pub const SNAP_PX: f64 = 8.;
/// Room after the last clip, in seconds.
pub const TAIL: f64 = 30.;

pub fn track_h(kind: TrackKind) -> f32 {
    match kind {
        TrackKind::Video => 66.,
        TrackKind::Audio => 50.,
    }
}

/// Top of each track row (content coordinates) and the height of them all.
pub struct Rows {
    pub tops: Vec<f32>,
    pub total: f32,
}

pub fn rows(tracks: &[Track]) -> Rows {
    let mut y = 0.;
    let tops = tracks
        .iter()
        .map(|t| {
            let top = y;
            y += track_h(t.kind) + TRACK_GAP;
            top
        })
        .collect();
    Rows { tops, total: y }
}

/// The track row under `y` (content coordinates).
pub fn row_at(tracks: &[Track], rows: &Rows, y: f32) -> Option<usize> {
    tracks.iter().enumerate().position(|(i, t)| y >= rows.tops[i] && y < rows.tops[i] + track_h(t.kind) + TRACK_GAP)
}

/// Where a track dragged by its header lands: the slot boundary nearest to `y`.
pub fn insertion_at(tracks: &[Track], rows: &Rows, y: f32) -> usize {
    for (i, t) in tracks.iter().enumerate() {
        if y < rows.tops[i] + (track_h(t.kind) + TRACK_GAP) / 2. {
            return i;
        }
    }
    tracks.len()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    pub t: f64,
    pub major: bool,
}

/// Ruler ticks for the visible range: a labelled major tick at least ~90 px apart, 4 or 5 minor ones between.
pub fn ticks(pps: f64, scroll_x: f64, view_w: f64) -> (Vec<Tick>, f64) {
    const STEPS: [f64; 14] = [1. / 30., 0.1, 0.25, 0.5, 1., 2., 5., 10., 15., 30., 60., 120., 300., 600.];
    let major = STEPS.iter().copied().find(|s| s * pps >= 90.).unwrap_or(600.);
    let minor = major / if major >= 1. { 5. } else { 4. };
    let from = (scroll_x / pps / major).floor().max(0.) * major;
    let to = (scroll_x + view_w) / pps + major;
    let mut out = vec![];
    let mut k = 0u32;
    loop {
        let t = from + k as f64 * minor;
        if t > to || k > 4000 {
            break;
        }
        let q = t / major;
        out.push(Tick { t, major: (q - q.round()).abs() < 1e-6 });
        k += 1;
    }
    (out, major)
}

/// `12s`, `1:05`, or frames (`15f`) when zoomed in far enough to see them.
pub fn tick_label(t: f64, pps: f64, fps: f64) -> String {
    let frac = t - t.floor();
    if pps >= 300. && frac > 1e-6 && (1. - frac) > 1e-6 {
        return format!("{}f", (frac * fps).round() as i64);
    }
    // Half seconds (majors every 0.5 s) keep their decimal so labels don't repeat.
    let tenths = (t * 10.).round() as i64;
    let (m, s10) = (tenths / 600, tenths % 600);
    let s = if s10 % 10 == 0 { format!("{}", s10 / 10) } else { format!("{}.{}", s10 / 10, s10 % 10) };
    if m > 0 { format!("{m}:{}{s}", if s10 < 100 { "0" } else { "" }) } else { format!("{s}s") }
}

/// Compact human duration: `4.2s`, `12s`, `1:05`.
pub fn short(t: f64) -> String {
    if t < 9.95 {
        return format!("{:.1}s", t.max(0.));
    }
    let total = t.round() as i64;
    if total < 60 { format!("{total}s") } else { format!("{}:{:02}", total / 60, total % 60) }
}

/// Times a moving edge sticks to: zero, the playhead, markers and every other clip's edges.
pub fn snap_points(p: &Project, exclude: &[Id], playhead: Option<f64>) -> Vec<f64> {
    let mut pts = vec![0.];
    pts.extend(playhead);
    pts.extend(p.markers.iter().map(|m| m.time));
    for t in &p.tracks {
        for c in t.clips.iter().filter(|c| !exclude.contains(&c.id)) {
            pts.push(c.start);
            pts.push(c.end());
        }
    }
    // The beats of music, when Settings › Audio asks for them.
    pts.extend(super::body::beat_points(p));
    pts
}

/// Snaps one time; returns it and the point it stuck to.
pub fn snap_time(t: f64, points: &[f64], pps: f64) -> (f64, Option<f64>) {
    let mut best = (t, None);
    let mut dist = SNAP_PX / pps;
    for &p in points {
        if (p - t).abs() < dist {
            dist = (p - t).abs();
            best = (p, Some(p));
        }
    }
    best
}

/// Snaps a moving span by whichever of its edges is closest to a snap point.
pub fn snap_span(start: f64, len: f64, points: &[f64], pps: f64) -> (f64, Option<f64>) {
    let mut best = (start, None);
    let mut dist = SNAP_PX / pps;
    for &p in points {
        for (edge, offset) in [(start, 0.), (start + len, len)] {
            if (p - edge).abs() < dist {
                dist = (p - edge).abs();
                best = (p - offset, Some(p));
            }
        }
    }
    best
}

/// The clip as `clip.trim` would leave it with `edge` at `time` (same bounds as kimchi-core).
pub fn trimmed(track: &Track, clip: &Clip, assets: &[Asset], start_edge: bool, time: f64) -> Clip {
    let mut c = clip.clone();
    let ci = track.clips.iter().position(|x| x.id == clip.id).unwrap_or(0);
    let source_len = clip.asset_id().and_then(|id| assets.iter().find(|a| a.id == id)).and_then(Asset::duration);
    let prev_end = if ci > 0 { track.clips[ci - 1].end() } else { 0. };
    let next_start = track.clips.get(ci + 1).map(|c| c.start).unwrap_or(f64::INFINITY);
    if start_edge {
        let mut lo = prev_end;
        if source_len.is_some() {
            lo = lo.max(c.start - c.in_point / c.speed);
        }
        let new_start = time.clamp(lo, (c.end() - MIN_CLIP).max(lo));
        let delta = new_start - c.start;
        c.in_point = (c.in_point + delta * c.speed).max(0.);
        c.start = new_start;
        c.duration -= delta;
    } else {
        let mut hi = next_start;
        if let Some(len) = source_len {
            hi = hi.min(c.start + (len - c.in_point) / c.speed);
        }
        let new_end = time.clamp(c.start + MIN_CLIP, hi.max(c.start + MIN_CLIP));
        c.duration = new_end - c.start;
    }
    c
}

/// The gap around `t` on a track: (start, length, is a real gap between two clips).
pub fn gap_at(track: &Track, t: f64) -> (f64, f64, bool) {
    let before = track.clips.iter().filter(|c| c.end() <= t + 1e-9).map(Clip::end).fold(0., f64::max);
    let after = track.clips.iter().filter(|c| c.start >= t - 1e-9).map(|c| c.start).fold(f64::INFINITY, f64::min);
    let in_gap = after.is_finite() && after > before + 1e-9;
    let start = if in_gap || before > 0. { before } else { t };
    let len = if in_gap { after - before } else { 5. };
    (start, len, in_gap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::{ClipContent, TextStyle};

    fn text(start: f64, d: f64) -> Clip {
        Clip::new("t", start, d, ClipContent::Text { style: TextStyle::default() })
    }

    #[test]
    fn ticks_have_labelled_majors_about_90px_apart() {
        let (ticks, major) = ticks(60., 0., 600.);
        assert_eq!(major, 2.);
        assert!(ticks[0].major && ticks[0].t == 0.);
        assert_eq!(ticks.iter().filter(|t| t.major).count(), 7);
        assert!(ticks.iter().all(|t| t.t <= 12.));
    }

    #[test]
    fn labels() {
        assert_eq!(tick_label(65., 60., 30.), "1:05");
        assert_eq!(tick_label(4., 60., 30.), "4s");
        assert_eq!(tick_label(4.5, 400., 30.), "15f");
        assert_eq!(short(4.25), "4.2s");
        assert_eq!(short(75.), "1:15");
    }

    #[test]
    fn snapping_prefers_the_closest_edge() {
        let pts = [0., 5., 10.];
        assert_eq!(snap_time(5.05, &pts, 100.), (5., Some(5.)));
        assert_eq!(snap_time(5.2, &pts, 100.), (5.2, None));
        // A 3 s span whose end lands near 10 sticks there.
        assert_eq!(snap_span(6.98, 3., &pts, 100.), (7., Some(10.)));
    }

    #[test]
    fn trim_preview_respects_neighbours() {
        let mut track = Track::new(TrackKind::Video, "V");
        track.clips = vec![text(0., 2.), text(4., 2.), text(8., 2.)];
        let mid = track.clips[1].clone();
        let c = trimmed(&track, &mid, &[], true, 1.);
        assert_eq!((c.start, c.duration), (2., 4.));
        let c = trimmed(&track, &mid, &[], false, 9.);
        assert_eq!(c.end(), 8.);
    }

    #[test]
    fn gaps() {
        let mut track = Track::new(TrackKind::Video, "V");
        track.clips = vec![text(0., 2.), text(5., 2.)];
        assert_eq!(gap_at(&track, 3.), (2., 3., true));
        assert_eq!(gap_at(&track, 9.), (7., 5., false));
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn labels_carry_and_keep_half_seconds() {
        assert_eq!(tick_label(1.5, 200., 30.), "1.5s");
        assert_eq!(tick_label(1.0, 200., 30.), "1s");
        assert_eq!(tick_label(65.0, 50., 30.), "1:05");
        assert_eq!(tick_label(90.5, 200., 30.), "1:30.5");
        assert_eq!(tick_label(119.99, 50., 30.), "2:00");
        assert_eq!(short(119.6), "2:00");
        assert_eq!(short(4.21), "4.2s");
        assert_eq!(short(9.97), "10s");
        assert_eq!(crate::ui::timecode(59.996), "1:00.00");
        assert_eq!(crate::ui::timecode(61.25), "1:01.25");
    }
}
