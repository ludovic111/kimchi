//! `agent.*`: the built-in agent of the Agent panel. The app installs it (`kimchi-agent` builds on
//! this crate, so it is reached through [`AgentHost`](crate::session::AgentHost)); the window and
//! every other client drive the same conversation and runs.

use std::sync::Arc;

use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    let host = s
        .agent_host()
        .ok_or_else(|| format!("`{}` needs the kimchi app, where the built-in agent runs. Start it and use kimchi-cli without --file, or kimchi-mcp --live.", cx.spec.name))?;
    if let Some(t) = a.opt_f64("timeout")
        && t <= 0.0
    {
        return Err("timeout is a number of seconds above 0".into());
    }
    host.call(s.clone(), cx.source, cx.spec.name, a).await
}
