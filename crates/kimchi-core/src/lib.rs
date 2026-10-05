//! kimchi-core: the editing model. No I/O beyond reading and writing project files.

pub mod anim;
pub mod audio;
pub mod edit;
pub mod effects;
pub mod expr;
pub mod history;
pub mod mesh;
pub mod model;
pub mod motion;
pub mod path;
pub mod presets;
pub mod store;
pub mod templates;
pub mod transition;

pub use edit::{ClipMove, ClipPatch, Edge, Edit, EditError, EditOutcome, TrackClip, TrackPatch};
pub use history::Editor;
pub use model::*;
pub use store::{Library, ProjectSummary};

pub use audio::{Beats, Bus, Channels, ClipAudio, Duck, FadeCurve, Insert, Master, Mixer, SongRef, TrackMix};
pub use anim::{Easing, KeyValue, Keyframe, Keyframes};
pub use effects::{ChromaKey, Effects, Lut};
pub use motion::{Scene, Scene2d, Scene3d, TemplateRef};
pub use transition::{Transition, TransitionKind};

/// The candidate closest to a mistyped `word` (edit distance, or one being a prefix of the
/// other), for "did you mean" hints.
pub fn closest<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let w = word.to_lowercase();
    if let Some(c) = candidates.iter().find(|c| {
        let c = c.to_lowercase();
        c.len() >= 3 && w.len() >= 3 && (w.starts_with(&c) || c.starts_with(&w))
    }) {
        return Some(c);
    }
    candidates
        .iter()
        .map(|c| (levenshtein(&w, &c.to_lowercase()), *c))
        .filter(|(d, c)| *d <= (c.len().max(3) / 3).max(2))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = if ca == b[j] { prev } else { 1 + prev.min(row[j]).min(row[j + 1]) };
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests;
