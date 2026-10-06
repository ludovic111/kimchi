//! Snapshot-based undo/redo. Projects are small (a few KB of JSON), so cloning
//! the whole document per step is cheap and impossible to get subtly wrong.
//!
//! Every client (the window, the agent, the CLI, MCP) edits through one
//! [`Editor`], so they all share this history. Each step remembers a label
//! (the command that made it) and who made it, so the agent panel can list
//! what an agent changed and `history.list` can show it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::edit::{Edit, EditError, EditOutcome, EditResult};
use crate::model::Project;

const MAX_HISTORY: usize = 300;
const MAX_CHECKPOINTS: usize = 64;
const COALESCE_WINDOW: Duration = Duration::from_millis(1200);
/// Checkpoint ids are unique in the process, so one taken on a project never names a state of
/// another (an agent run's "Revert" after the person switched projects).
static NEXT_CHECKPOINT: AtomicU64 = AtomicU64::new(1);

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
    /// One per nesting depth, innermost last: what a rollback at that depth goes back to.
    levels: Vec<Level>,
    pushed: bool,
    info: StepInfo,
}

#[derive(Debug, Clone)]
struct Level {
    start: Project,
    undo_len: usize,
    redo: Vec<Step>,
    pushed: bool,
}

pub struct Editor {
    project: Project,
    undo: Vec<Step>,
    redo: Vec<Step>,
    last_coalesce: Option<(String, Instant)>,
    dirty: bool,
    batch: Option<Batch>,
    checkpoints: BTreeMap<u64, Project>,
    /// Label and source for the next steps, set by the command layer.
    current: StepInfo,
    /// The caller isn't the one that opened the batch: its changes would fold into (and be
    /// rolled back with) someone else's step, so they are refused while a batch is open.
    outsider: bool,
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
            current: StepInfo { label: "edit".into(), source: "window".into() },
            outsider: false,
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

    /// Marks the caller as not owning an open batch (see [`busy`](Self::busy)).
    pub fn set_outsider(&mut self, outsider: bool) {
        self.outsider = outsider;
    }

    /// Refuses a change from an outsider while a batch is open.
    fn busy(&self) -> EditResult {
        match &self.batch {
            Some(b) if self.outsider => Err(EditError::Busy(b.info.source.clone())),
            _ => Ok(()),
        }
    }

    /// Applies an edit. Edits sharing a `coalesce` key within a short window
    /// (e.g. dragging a slider) collapse into a single undo step. A unique `gesture:` key
    /// keeps consecutive edits from the same source together without a time limit.
    /// An edit that fails changes nothing (multi-part edits are put back as they were).
    pub fn apply(&mut self, edit: &Edit, coalesce: Option<&str>) -> EditResult<EditOutcome> {
        if edit.is_background() {
            // They fail (if at all) before changing anything.
            let out = self.project.apply(edit)?;
            self.dirty = true;
            // Background edits describe facts (a render finished, a thumbnail
            // exists), so they hold in every past and future state too.
            self.apply_everywhere(edit);
            return Ok(out);
        }
        self.busy()?;

        let before = self.project.clone();
        let out = match self.project.apply(edit) {
            Ok(out) => out,
            Err(e) => {
                self.project = before;
                return Err(e);
            }
        };
        self.dirty = true;
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
    pub fn replace(&mut self, project: Project) -> EditResult {
        self.busy()?;
        if project == self.project {
            return Ok(());
        }
        let before = std::mem::replace(&mut self.project, project);
        self.dirty = true;
        self.record(before, None);
        Ok(())
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
            (Some(key), Some((last, at))) => {
                let gesture=key.strip_prefix("gesture:").is_some_and(|id| !id.is_empty());
                key==last && (gesture || now.duration_since(*at)<COALESCE_WINDOW)
                    && self.undo.last().is_some_and(|step| step.info.source==self.current.source)
            }
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
        for l in self.batch.iter_mut().flat_map(|b| b.levels.iter_mut()) {
            let _ = l.start.apply(edit);
            for s in &mut l.redo {
                let _ = s.before.apply(edit);
            }
        }
    }

    /// Starts a batch: every change until the matching [`end_batch`](Self::end_batch)
    /// becomes one undo step labelled `label`. Batches nest; the outermost names the step.
    pub fn begin_batch(&mut self, label: impl Into<String>, source: impl Into<String>) {
        let pushed = self.batch.as_ref().is_some_and(|b| b.pushed);
        let level = Level { start: self.project.clone(), undo_len: self.undo.len(), redo: self.redo.clone(), pushed };
        match &mut self.batch {
            Some(b) => b.levels.push(level),
            None => self.batch = Some(Batch { levels: vec![level], pushed: false, info: StepInfo { label: label.into(), source: source.into() } }),
        }
    }

    pub fn end_batch(&mut self) {
        if let Some(b) = &mut self.batch {
            b.levels.pop();
            if b.levels.is_empty() {
                self.batch = None;
                self.last_coalesce = None;
                self.trim();
            }
        }
    }

    /// Ends the innermost batch and puts everything back as it was when that one began (an
    /// outer batch carries on with what came before).
    pub fn rollback_batch(&mut self) {
        let Some(b) = &mut self.batch else { return };
        let Some(l) = b.levels.pop() else { return };
        b.pushed = l.pushed;
        if b.levels.is_empty() {
            self.batch = None;
        }
        self.project = l.start;
        self.undo.truncate(l.undo_len);
        self.redo = l.redo;
        self.last_coalesce = None;
    }

    pub fn in_batch(&self) -> bool {
        self.batch.is_some()
    }

    /// Remembers the current project so it can be restored with [`revert_to`](Self::revert_to).
    pub fn checkpoint(&mut self) -> u64 {
        let id = NEXT_CHECKPOINT.fetch_add(1, Ordering::Relaxed);
        self.checkpoints.insert(id, self.project.clone());
        while self.checkpoints.len() > MAX_CHECKPOINTS {
            let oldest = *self.checkpoints.keys().next().expect("not empty");
            self.checkpoints.remove(&oldest);
        }
        id
    }

    /// Puts the project back as it was at `checkpoint`, as one new undo step
    /// (so the revert itself can be undone). Returns false for a checkpoint this history
    /// doesn't have (another project's, or too old).
    pub fn revert_to(&mut self, checkpoint: u64) -> EditResult<bool> {
        let Some(p) = self.checkpoints.get(&checkpoint).cloned() else { return Ok(false) };
        self.replace(p)?;
        Ok(true)
    }

    pub fn has_checkpoint(&self, checkpoint: u64) -> bool {
        self.checkpoints.contains_key(&checkpoint)
    }

    /// Whether the project is as it was at `checkpoint` (after a revert to it, say).
    pub fn is_at_checkpoint(&self, checkpoint: u64) -> bool {
        self.checkpoints.get(&checkpoint).is_some_and(|p| *p == self.project)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip,ClipContent,ClipPatch,ProjectSettings};

    #[test]
    fn gesture_coalescing_survives_pauses_and_respects_history_boundaries() {
        let mut project=Project::new("Gestures",ProjectSettings::default());
        let clip=Clip::new("Solid",0.,4.,ClipContent::Solid {color:"#ffffff".into()});
        let id=clip.id;
        project.apply(&Edit::AddClip {track_id:None,clip}).unwrap();
        let edit=|volume| Edit::UpdateClip {clip_id:id,patch:ClipPatch {volume:Some(volume),..Default::default()}};
        let age=|ed:&mut Editor| ed.last_coalesce.as_mut().unwrap().1=Instant::now()-Duration::from_secs(5);

        let mut timed=Editor::new(project.clone());
        timed.apply(&edit(0.8),Some("volume")).unwrap();age(&mut timed);
        timed.apply(&edit(0.6),Some("volume")).unwrap();
        assert_eq!(timed.undo_steps().len(),2,"ordinary coalescing still expires");

        let mut gesture=Editor::new(project.clone());
        gesture.apply(&edit(0.8),Some("gesture:first")).unwrap();age(&mut gesture);
        gesture.apply(&edit(0.6),Some("gesture:first")).unwrap();
        assert_eq!(gesture.undo_steps().len(),1,"pauses do not split one gesture");
        assert!(gesture.undo());assert_eq!(gesture.project().clip(id).unwrap().volume,1.);
        assert!(gesture.redo());assert_eq!(gesture.project().clip(id).unwrap().volume,0.6);
        gesture.apply(&edit(0.4),Some("gesture:first")).unwrap();
        assert_eq!(gesture.undo_steps().len(),2,"redo ends the previous group");
        assert!(gesture.undo());
        gesture.apply(&edit(0.5),Some("gesture:first")).unwrap();
        assert_eq!(gesture.undo_steps().len(),2,"undo also ends the previous group");

        let mut separate=Editor::new(project);
        separate.apply(&edit(0.8),Some("gesture:first")).unwrap();
        separate.apply(&edit(0.7),Some("gesture:second")).unwrap();
        assert_eq!(separate.undo_steps().len(),2,"a new gesture starts its own step");
        separate.set_step_info("volume","mcp");
        separate.apply(&edit(0.6),Some("gesture:second")).unwrap();
        separate.set_step_info("volume","window");
        separate.apply(&edit(0.5),Some("gesture:second")).unwrap();
        assert_eq!(separate.undo_steps().len(),4,"other clients do not join the gesture");
        separate.apply(&edit(0.4),None).unwrap();
        separate.apply(&edit(0.3),Some("gesture:second")).unwrap();
        assert_eq!(separate.undo_steps().len(),6,"intervening edits split a gesture");
    }
}
