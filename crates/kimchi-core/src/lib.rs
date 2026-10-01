//! kimchi-core: the editing model. No I/O beyond reading and writing project files.

pub mod edit;
pub mod history;
pub mod model;
pub mod store;

pub use edit::{ClipMove, ClipPatch, Edge, Edit, EditError, EditOutcome, TrackPatch};
pub use history::Editor;
pub use model::*;
pub use store::{Library, ProjectSummary};

#[cfg(test)]
mod tests;
