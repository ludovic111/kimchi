//! Snapshot-based undo/redo. Projects are small (a few KB of JSON), so cloning
//! the whole document per step is cheap and impossible to get subtly wrong.

use std::time::{Duration, Instant};

use crate::edit::{Edit, EditOutcome, EditResult};
use crate::model::Project;

const MAX_HISTORY: usize = 300;
const COALESCE_WINDOW: Duration = Duration::from_millis(1200);

pub struct Editor {
    project: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
    last_coalesce: Option<(String, Instant)>,
    dirty: bool,
}

impl Editor {
    pub fn new(project: Project) -> Self {
        Self { project, undo: vec![], redo: vec![], last_coalesce: None, dirty: false }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Whether there are changes not yet written to disk.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Applies an edit. Edits sharing a `coalesce` key within a short window
    /// (e.g. dragging a slider) collapse into a single undo step.
    pub fn apply(&mut self, edit: &Edit, coalesce: Option<&str>) -> EditResult<EditOutcome> {
        self.dirty = true;
        if edit.is_background() {
            let out = self.project.apply(edit)?;
            // Background edits describe facts (a render finished, a thumbnail
            // exists), so they hold in every past and future state too.
            for p in self.undo.iter_mut().chain(self.redo.iter_mut()) {
                let _ = p.apply(edit);
            }
            return Ok(out);
        }

        let before = self.project.clone();
        let out = self.project.apply(edit)?;
        if before == self.project {
            return Ok(out);
        }

        let now = Instant::now();
        let merge = match (coalesce, &self.last_coalesce) {
            (Some(key), Some((last, at))) => key == last && now.duration_since(*at) < COALESCE_WINDOW,
            _ => false,
        };
        if !merge {
            self.undo.push(before);
            if self.undo.len() > MAX_HISTORY {
                self.undo.remove(0);
            }
        }
        self.last_coalesce = coalesce.map(|k| (k.to_string(), now));
        self.redo.clear();
        Ok(out)
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.project, prev));
        self.last_coalesce = None;
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.project, next));
        self.last_coalesce = None;
        self.dirty = true;
        true
    }
}
