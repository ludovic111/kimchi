//! Hand-offs between lsuite apps: kimchi ↔ ryolune.

use std::sync::Arc;

use serde_json::json;

use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    let _ = (s, &a);
    match cx.spec.name {
        "handoff.apps" => Ok(json!(crate::discovery::installed_apps())),
        _ => Err(crate::commands::unhandled(cx)),
    }
}
