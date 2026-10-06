//! What survived a trip between kimchi and another app, said plainly so a person can check the
//! parts that changed.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The format read or written (`timeline::Format::id`, `looks` format id).
    pub format: String,
    /// Counts of what came through as it was: "12 clips on 3 tracks", "4 markers".
    pub kept: Vec<String>,
    /// What came through close but not the same: "Cross dissolve 'Iris' became a dissolve".
    pub approximated: Vec<String>,
    /// What couldn't come at all: "2 Lumetri curves (kimchi has no curves)".
    pub dropped: Vec<String>,
    /// Media files the project names that aren't where it says (relink them with `media.relink`).
    pub missing_media: Vec<String>,
}

impl Report {
    pub fn new(format: &str) -> Self {
        Self { format: format.into(), ..Self::default() }
    }

    pub fn kept(&mut self, what: impl Into<String>) {
        self.kept.push(what.into());
    }

    pub fn approximated(&mut self, what: impl Into<String>) {
        push_unique(&mut self.approximated, what.into());
    }

    pub fn dropped(&mut self, what: impl Into<String>) {
        push_unique(&mut self.dropped, what.into());
    }

    /// Nothing was lost or changed.
    pub fn is_exact(&self) -> bool {
        self.approximated.is_empty() && self.dropped.is_empty() && self.missing_media.is_empty()
    }
}

/// The same note twice ("a transition kimchi doesn't have") says it once.
fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}
