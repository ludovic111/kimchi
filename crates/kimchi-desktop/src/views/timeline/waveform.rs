//! Audio waveforms: the peaks file (little-endian f32 in 0..1, `peaks_per_second`
//! values a second) is read once per path off the UI thread, then drawn as bars scaled by
//! the clip's level (`body::sound::wave`).

use std::collections::HashMap;
use std::sync::Arc;

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
    Some(Arc::new(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()))
}
