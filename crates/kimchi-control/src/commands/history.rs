use std::sync::Arc;

use serde_json::json;

use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "history.list" => s.read(|ed| {
            json!({
                "undo": ed.undo_steps().iter().rev().collect::<Vec<_>>(),
                "redo": ed.redo_steps(),
                "canUndo": ed.can_undo(),
                "canRedo": ed.can_redo(),
            })
        }),
        "history.undo" => {
            let done = s.edit(cx.label(), cx.source, |ed| Ok(ed.undo()))?;
            if !done {
                return Err("Nothing to undo.".into());
            }
            // The step just undone is now the next redo.
            stepped(s, |ed| ed.redo_steps().first().cloned())
        }
        "history.redo" => {
            let done = s.edit(cx.label(), cx.source, |ed| Ok(ed.redo()))?;
            if !done {
                return Err("Nothing to redo.".into());
            }
            stepped(s, |ed| ed.undo_steps().last().cloned())
        }
        "history.checkpoint" => {
            let id = s.edit(cx.label(), cx.source, |ed| Ok(ed.checkpoint()))?;
            Ok(json!({ "checkpoint": id }))
        }
        "history.revertTo" => {
            let cp = a.opt_i64("checkpoint").unwrap_or(-1);
            if cp < 0 {
                return Err("checkpoint should be an id from history.checkpoint".into());
            }
            let ok = s.edit(cx.label(), cx.source, |ed| Ok(ed.revert_to(cp as u64)))?;
            if !ok {
                return Err(format!("Checkpoint {cp} is unknown or too old (only the last 64 are kept)."));
            }
            state(s)
        }
        _ => Err(crate::commands::unhandled(cx)),
    }
}

/// The history state and the step that was undone or redone (`step`: the command that made it).
fn stepped(s: &Session, step: impl FnOnce(&kimchi_core::Editor) -> Option<kimchi_core::history::StepInfo>) -> CmdResult {
    s.read(|ed| {
        let info = step(ed).unwrap_or_default();
        json!({ "canUndo": ed.can_undo(), "canRedo": ed.can_redo(), "step": info.label, "source": info.source })
    })
}

fn state(s: &Session) -> CmdResult {
    s.read(|ed| json!({ "canUndo": ed.can_undo(), "canRedo": ed.can_redo() }))
}
