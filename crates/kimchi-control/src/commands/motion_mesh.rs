//! Mesh modelling commands: `motion.convertToMesh`, `motion.applyModifier`, `motion.editMesh`.

use std::sync::Arc;

use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(_s: &Arc<Session>, cx: &Ctx, _a: Args) -> CmdResult {
    Err(crate::commands::unhandled(cx))
}
