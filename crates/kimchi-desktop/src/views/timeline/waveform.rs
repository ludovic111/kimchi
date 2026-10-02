//! Audio waveforms: the peaks file (little-endian f32 in 0..1, `peaks_per_second`
//! values a second) is read once per path off the UI thread, then drawn as bars.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Bounds, Hsla, IntoElement, Styled, canvas, fill, point, px, size};

pub type Peaks = Arc<Vec<f32>>;

/// Peaks by file path; `None` while a read is in flight (or after it failed).
#[derive(Default)]
pub struct PeaksCache {
    map: HashMap<String, Option<Peaks>>,
}

impl PeaksCache {
    /// The peaks if loaded; `true` in the second field when a read must be started.
    pub fn get(&mut self, path: &str) -> (Option<Peaks>, bool) {
        match self.map.get(path) {
            Some(p) => (p.clone(), false),
            None => {
                self.map.insert(path.to_string(), None);
                (None, true)
            }
        }
    }

    pub fn put(&mut self, path: String, peaks: Option<Peaks>) {
        self.map.insert(path, peaks);
    }
}

/// Reads a peaks file (blocking: call it on a background thread).
pub fn read(path: &str) -> Option<Peaks> {
    let bytes = std::fs::read(path).ok()?;
    Some(Arc::new(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()))
}

/// Bars for the part of a clip between `vis.0` and `vis.1` (clip-local pixels; the clip is `full_w`
/// wide and shows source seconds `from..to`). The element is `vis.1 - vis.0` wide and `h` tall,
/// placed `left` pixels into its parent.
#[allow(clippy::too_many_arguments)]
pub fn bars(peaks: Peaks, per_second: u32, from: f64, to: f64, full_w: f32, vis: (f32, f32), left: f32, h: f32, color: Hsla) -> impl IntoElement {
    let pps = per_second.max(1) as f64;
    let span = (to - from).max(1e-6);
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            let mid = h / 2.;
            // Columns on even clip-local pixels, so bars don't shimmer while scrolling.
            let mut x = (vis.0 / 2.).floor() * 2.;
            while x < vis.1 {
                let i0 = ((from + (x / full_w) as f64 * span) * pps).floor().max(0.) as usize;
                let i1 = (((from + ((x + 2.) / full_w) as f64 * span) * pps).floor() as usize).max(i0 + 1);
                let peak = peaks.get(i0..i1.min(peaks.len())).map(|s| s.iter().copied().fold(0f32, f32::max)).unwrap_or(0.);
                let bar = (peak.clamp(0., 1.) * (h - 2.)).max(1.);
                let bx = bounds.origin.x + px(x - vis.0);
                window.paint_quad(fill(Bounds::new(point(bx, bounds.origin.y + px(mid - bar / 2.)), size(px(1.4), px(bar))), color));
                x += 2.;
            }
        },
    )
    .absolute()
    .bottom_0()
    .left(px(left))
    .w(px((vis.1 - vis.0).max(0.)))
    .h(px(h))
}
