//! Snapshot-based undo/redo. Projects are small (a few KB of JSON), so cloning
//! the whole document per step is cheap and impossible to get subtly wrong.
//!
//! Every client (the window, the agent, the CLI, MCP) edits through one
//! [`Editor`], so they all share this history. Each step remembers a label
//! (the command that made it) and who made it, so the agent panel can list
//! what an agent changed and `history.list` can show it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::edit::{Edit, EditOutcome, EditResult};
use crate::model::Project;

const MAX_HISTORY: usize = 300;
const MAX_CHECKPOINTS: usize = 64;
const COALESCE_WINDOW: Duration = Duration::from_millis(1200);

/// What one undo step did and who did it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StepInfo {
    /// The command that made the step, e.g. `clip.split`.
    pub label: String,
    /// `window`, `agent`, `cli` or `mcp`.
    pub source: String,
}

#[derive(Debug, Clone)]
struct Step {
    /// The project as it was before this step.
    before: Project,
    info: StepInfo,
}

/// Open batch: everything applied until [`Editor::end_batch`] is one undo step.
#[derive(Debug, Clone)]
struct Batch {
    depth: usize,
    /// Project and history length when the outermost batch began, for rollback.
    start: Project,
    undo_len: usize,
    redo: Vec<Step>,
    pushed: bool,
    info: StepInfo,
}

pub struct Editor {
    project: Project,
    undo: Vec<Step>,
    redo: Vec<Step>,
    last_coalesce: Option<(String, Instant)>,
    dirty: bool,
    batch: Option<Batch>,
    checkpoints: BTreeMap<u64, Project>,
    next_checkpoint: u64,
    /// Label and source for the next steps, set by the command layer.
    current: StepInfo,
}

impl Editor {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            undo: vec![],
            redo: vec![],
            last_coalesce: None,
            dirty: false,
            batch: None,
            checkpoints: BTreeMap::new(),
            next_checkpoint: 1,
            current: StepInfo { label: "edit".into(), source: "window".into() },
        }
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

    /// Labels of the undo steps, oldest first.
    pub fn undo_steps(&self) -> Vec<StepInfo> {
        self.undo.iter().map(|s| s.info.clone()).collect()
    }

    /// Labels of the redo steps, the next redo first.
    pub fn redo_steps(&self) -> Vec<StepInfo> {
        self.redo.iter().rev().map(|s| s.info.clone()).collect()
    }

    /// Whether there are changes not yet written to disk.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Sets the label and source recorded with the next undo steps.
    pub fn set_step_info(&mut self, label: impl Into<String>, source: impl Into<String>) {
        self.current = StepInfo { label: label.into(), source: source.into() };
    }

    /// Applies an edit. Edits sharing a `coalesce` key within a short window
    /// (e.g. dragging a slider) collapse into a single undo step.
    pub fn apply(&mut self, edit: &Edit, coalesce: Option<&str>) -> EditResult<EditOutcome> {
        self.dirty = true;
        if edit.is_background() {
            let out = self.project.apply(edit)?;
            // Background edits describe facts (a render finished, a thumbnail
            // exists), so they hold in every past and future state too.
            self.apply_everywhere(edit);
            return Ok(out);
        }

        let before = self.project.clone();
        let out = self.project.apply(edit)?;
        // `apply` stamps `updated_at`; an edit that changed nothing else (a slider's final value,
        // the same as its last) is no step.
        self.project.updated_at = before.updated_at;
        if before == self.project {
            return Ok(out);
        }
        self.project.updated_at = chrono::Utc::now();
        self.record(before, coalesce);
        Ok(out)
    }

    /// Replaces the whole project as one undo step (used to revert to a checkpoint).
    pub fn replace(&mut self, project: Project) {
        if project == self.project {
            return;
        }
        let before = std::mem::replace(&mut self.project, project);
        self.dirty = true;
        self.record(before, None);
    }

    fn record(&mut self, before: Project, coalesce: Option<&str>) {
        self.redo.clear();
        if let Some(batch) = &mut self.batch {
            if !batch.pushed {
                batch.pushed = true;
                self.undo.push(Step { before, info: batch.info.clone() });
            }
            self.last_coalesce = None;
            self.trim();
            return;
        }
        let now = Instant::now();
        let merge = match (coalesce, &self.last_coalesce) {
            (Some(key), Some((last, at))) => key == last && now.duration_since(*at) < COALESCE_WINDOW,
            _ => false,
        };
        if !merge {
            self.undo.push(Step { before, info: self.current.clone() });
        }
        self.last_coalesce = coalesce.map(|k| (k.to_string(), now));
        self.trim();
    }

    fn trim(&mut self) {
        if self.undo.len() > MAX_HISTORY && self.batch.is_none() {
            let extra = self.undo.len() - MAX_HISTORY;
            self.undo.drain(..extra);
        }
    }

    fn apply_everywhere(&mut self, edit: &Edit) {
        for s in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            let _ = s.before.apply(edit);
        }
        for p in self.checkpoints.values_mut() {
            let _ = p.apply(edit);
        }
        if let Some(b) = &mut self.batch {
            let _ = b.start.apply(edit);
            for s in &mut b.redo {
                let _ = s.before.apply(edit);
            }
        }
    }

    /// Starts a batch: every change until the matching [`end_batch`](Self::end_batch)
    /// becomes one undo step labelled `label`. Batches nest; the outermost wins.
    pub fn begin_batch(&mut self, label: impl Into<String>, source: impl Into<String>) {
        if let Some(b) = &mut self.batch {
            b.depth += 1;
            return;
        }
        self.batch = Some(Batch {
            depth: 1,
            start: self.project.clone(),
            undo_len: self.undo.len(),
            redo: self.redo.clone(),
            pushed: false,
            info: StepInfo { label: label.into(), source: source.into() },
        });
    }

    pub fn end_batch(&mut self) {
        if let Some(b) = &mut self.batch {
            b.depth -= 1;
            if b.depth == 0 {
                self.batch = None;
                self.last_coalesce = None;
                self.trim();
            }
        }
    }

    /// Ends the outermost batch and puts everything back as it was when it began.
    pub fn rollback_batch(&mut self) {
        if let Some(b) = self.batch.take() {
            self.project = b.start;
            self.undo.truncate(b.undo_len);
            self.redo = b.redo;
            self.last_coalesce = None;
        }
    }

    pub fn in_batch(&self) -> bool {
        self.batch.is_some()
    }

    /// Remembers the current project so it can be restored with [`revert_to`](Self::revert_to).
    pub fn checkpoint(&mut self) -> u64 {
        let id = self.next_checkpoint;
        self.next_checkpoint += 1;
        self.checkpoints.insert(id, self.project.clone());
        while self.checkpoints.len() > MAX_CHECKPOINTS {
            let oldest = *self.checkpoints.keys().next().expect("not empty");
            self.checkpoints.remove(&oldest);
        }
        id
    }

    /// Puts the project back as it was at `checkpoint`, as one new undo step
    /// (so the revert itself can be undone). Returns false for an unknown checkpoint.
    pub fn revert_to(&mut self, checkpoint: u64) -> bool {
        let Some(p) = self.checkpoints.get(&checkpoint).cloned() else { return false };
        self.replace(p);
        true
    }

    pub fn has_checkpoint(&self, checkpoint: u64) -> bool {
        self.checkpoints.contains_key(&checkpoint)
    }

    pub fn undo(&mut self) -> bool {
        if self.batch.is_some() {
            return false;
        }
        let Some(step) = self.undo.pop() else { return false };
        let after = std::mem::replace(&mut self.project, step.before);
        self.redo.push(Step { before: after, info: step.info });
        self.last_coalesce = None;
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        if self.batch.is_some() {
            return false;
        }
        let Some(step) = self.redo.pop() else { return false };
        let before = std::mem::replace(&mut self.project, step.before);
        self.undo.push(Step { before, info: step.info });
        self.last_coalesce = None;
        self.dirty = true;
        true
    }
}
