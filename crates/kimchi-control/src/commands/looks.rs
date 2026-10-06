use std::sync::Arc;

use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    let _ = (s, a);
    Err(super::unhandled(cx))
}
