//! The built-in agent's conversation, shared by every client.
//!
//! The app installs one [`Host`] on its session. It owns what the Agent panel shows (the thread,
//! the runs, the requests and replies, one card per command and how each run ended), and the
//! `agent.*` commands drive it, so the panel, `kimchi-cli` and MCP clients send to, watch, stop
//! and revert the same runs. The panel only draws [`Host::snapshot`], again whenever
//! [`Host::subscribe`] ticks.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use futures::future::BoxFuture;
use kimchi_control::registry::Args;
use kimchi_control::session::{AgentHost, Event};
use kimchi_control::{CmdResult, CommandRecord, Session, Source};
use kimchi_core::Id;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::{Agent, AgentConfig, AgentEvent, Conversation, ProviderKind, RunHandle};

/// Entries kept in the conversation; older ones go (indices keep counting).
const MAX_ENTRIES: usize = 2000;

/// How long `wait` waits by default.
const DEFAULT_WAIT: f64 = 900.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Running,
    Done,
    Error,
    Cancelled,
}

/// One request to the agent and what came of it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    /// Counts up for as long as the app runs.
    pub id: u64,
    pub prompt: String,
    pub provider: ProviderKind,
    /// The model asked for (empty: the provider's default).
    pub model: String,
    /// Who sent the request: window, cli, mcp or agent.
    pub source: Source,
    pub state: RunState,
    /// What it is doing now, while it runs ("Running clip.addText…").
    pub activity: Option<String>,
    /// The agent's reply, as streamed.
    pub reply: String,
    pub error: Option<String>,
    /// Taken before its first change; what agent.revert goes back to.
    pub checkpoint: Option<u64>,
    /// Successful commands that could change the project.
    pub changes: usize,
    /// Every command it ran.
    pub commands: usize,
    pub reverted: bool,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl RunInfo {
    pub fn finished(&self) -> bool {
        self.state != RunState::Running
    }

    pub fn can_revert(&self) -> bool {
        self.finished() && self.checkpoint.is_some() && !self.reverted
    }

    /// Seconds from the request to the end (or to now, while it runs).
    pub fn seconds(&self) -> f64 {
        (self.finished_at.unwrap_or_else(Utc::now) - self.started_at).num_milliseconds().max(0) as f64 / 1000.0
    }

    fn json(&self) -> Value {
        let mut v = json!(self);
        v["canRevert"] = json!(self.can_revert());
        v["seconds"] = json!((self.seconds() * 10.0).round() / 10.0);
        v
    }
}

/// One thing the conversation shows, in the order it happened.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Entry {
    /// A request, from the panel or another client (`source`).
    #[serde(rename_all = "camelCase")]
    User { text: String, run: u64, source: Source },
    /// The agent's reply text.
    Assistant { text: String, run: u64 },
    /// One command, run by the agent (`run`) or by an MCP client or the CLI (`run` null).
    Command { record: Box<CommandRecord>, result: Option<Value>, run: Option<u64> },
    /// How a run ended.
    #[serde(rename_all = "camelCase")]
    Outcome { run: u64, state: RunState, error: Option<String>, changes: usize, seconds: f64, tokens: u64 },
}

/// What the panel draws.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub runs: Vec<RunInfo>,
    /// The run in progress.
    pub running: Option<u64>,
}

impl Snapshot {
    pub fn run(&self, id: u64) -> Option<&RunInfo> {
        self.runs.iter().find(|r| r.id == id)
    }
}

#[derive(Default)]
struct State {
    /// The project the conversation is about.
    project: Option<Id>,
    conversation: Conversation,
    entries: Vec<Entry>,
    /// Index of `entries[0]` since the conversation began.
    base: usize,
    /// Command seq → absolute index in the conversation.
    seen: HashMap<u64, usize>,
    runs: Vec<RunInfo>,
    active: Option<(u64, RunHandle)>,
    /// Reply text arrived for the active run.
    streamed: bool,
    next_run: u64,
    /// Runs reverted, the latest last; and those whose revert was undone, for a redo.
    reverts: Vec<u64>,
    unreverts: Vec<u64>,
}

impl State {
    fn run_mut(&mut self, id: u64) -> Option<&mut RunInfo> {
        self.runs.iter_mut().find(|r| r.id == id)
    }

    fn push(&mut self, e: Entry) {
        self.entries.push(e);
        if self.entries.len() > MAX_ENTRIES {
            let drop = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..drop);
            self.base += drop;
            let base = self.base;
            self.seen.retain(|_, i| *i >= base);
        }
    }

    /// Shows a command once, whoever reports it first (the run or the session's stream).
    fn command(&mut self, record: CommandRecord, result: Option<Value>, run: Option<u64>) {
        // `agent.*` calls (a client following a run) aren't part of the conversation.
        if record.command.starts_with("agent.") {
            return;
        }
        let seq = record.seq;
        if let Some(&i) = self.seen.get(&seq) {
            let base = self.base;
            let mut newly = false;
            if let Some(Entry::Command { result: slot, run: owner, .. }) = self.entries.get_mut(i - base) {
                if result.is_some() {
                    *slot = result;
                }
                if run.is_some() && owner.is_none() {
                    *owner = run;
                    newly = true;
                }
            }
            if newly && let Some(r) = run.and_then(|id| self.run_mut(id)) {
                r.commands += 1;
            }
            return;
        }
        if let Some(r) = run.and_then(|id| self.run_mut(id)) {
            r.commands += 1;
        }
        self.seen.insert(seq, self.base + self.entries.len());
        self.push(Entry::Command { record: Box::new(record), result, run });
    }

    /// A revert was undone (`undo`) or redone: the latest reverted run can be reverted again, or
    /// the latest one brought back is reverted again, when the project shows it.
    fn revert_undone(&mut self, session: &Session, undo: bool) {
        let id = if undo { self.reverts.last() } else { self.unreverts.last() }.copied();
        let Some(id) = id else { return };
        let Some(cp) = self.runs.iter().find(|r| r.id == id).and_then(|r| r.checkpoint) else { return };
        // After an undo the project has left the checkpoint; after a redo it is back there.
        if session.read(|ed| ed.is_at_checkpoint(cp)).unwrap_or(false) == undo {
            return;
        }
        if undo {
            self.reverts.pop();
            self.unreverts.push(id);
        } else {
            self.unreverts.pop();
            self.reverts.push(id);
        }
        if let Some(r) = self.run_mut(id) {
            r.reverted = !undo;
        }
    }

    fn clear_thread(&mut self) {
        self.base += self.entries.len();
        self.entries.clear();
        self.seen.clear();
        self.conversation = Conversation::new();
    }
}

pub struct Host {
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

impl Host {
    /// Makes the host, listens to the session and answers its `agent.*` commands. Call once,
    /// inside the session's runtime or not.
    pub fn install(session: &Arc<Session>) -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        let host = Arc::new(Self {
            state: Mutex::new(State { project: session.current_id(), next_run: 1, ..Default::default() }),
            changed,
        });
        let mut events = session.subscribe();
        let (weak, weak_session) = (Arc::downgrade(&host), Arc::downgrade(session));
        session.runtime().spawn(async move {
            loop {
                let event = match events.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                let (Some(host), Some(session)) = (weak.upgrade(), weak_session.upgrade()) else { break };
                host.on_session_event(&session, event);
            }
        });
        session.set_agent_host(host.clone());
        host
    }

    /// Ticks after every change of what [`snapshot`](Self::snapshot) returns.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = self.state.lock();
        Snapshot { entries: st.entries.clone(), runs: st.runs.clone(), running: st.active.as_ref().map(|(id, _)| *id) }
    }

    /// The run in progress, if any.
    pub fn running(&self) -> Option<u64> {
        self.state.lock().active.as_ref().map(|(id, _)| *id)
    }

    fn notify(&self) {
        self.changed.send_modify(|v| *v += 1);
    }

    fn on_session_event(&self, session: &Session, event: Event) {
        match event {
            Event::Command { record } => {
                let mut st = self.state.lock();
                if record.ok && record.result.as_ref().is_some_and(|r| r["step"] == "history.revertTo") {
                    match record.command.as_str() {
                        "history.undo" => st.revert_undone(session, true),
                        "history.redo" => st.revert_undone(session, false),
                        _ => {}
                    }
                }
                // Commands of MCP clients and the CLI show as cards; the window's own don't.
                if record.source != Source::Window {
                    st.command(record, None, None);
                }
                drop(st);
                self.notify();
            }
            Event::ProjectSwitched { project_id } => {
                let mut st = self.state.lock();
                if st.project != project_id {
                    // Another project: a run still going would edit it, and "Revert this run"
                    // would apply to it. Stop, and start the conversation afresh.
                    if st.project.is_some() {
                        if let Some((_, handle)) = st.active.take() {
                            handle.cancel();
                        }
                        st.runs.clear();
                        st.reverts.clear();
                        st.unreverts.clear();
                        st.clear_thread();
                    }
                    st.project = project_id;
                    drop(st);
                    self.notify();
                }
            }
            _ => {}
        }
    }

    // ---- runs -----------------------------------------------------------------

    /// Starts a run with the provider in `settings.agent`, continuing the conversation.
    pub fn send(self: &Arc<Self>, session: &Arc<Session>, source: Source, prompt: &str) -> CmdResult<RunInfo> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("Write a request for the agent first.".into());
        }
        let mut st = self.state.lock();
        if let Some((id, _)) = &st.active {
            let what = st.runs.iter().find(|r| r.id == *id).map(|r| format!(" (\"{}\")", crate::tools::bounded(&r.prompt, 80))).unwrap_or_default();
            return Err(format!("The agent is still working on run {id}{what}. Wait for it (agent.status wait=true) or stop it (agent.stop)."));
        }
        let config = AgentConfig::from_settings(&session.settings().agent);
        let id = st.next_run;
        st.next_run += 1;
        let info = RunInfo {
            id,
            prompt: prompt.clone(),
            provider: config.provider,
            model: config.model(),
            source,
            state: RunState::Running,
            activity: Some(format!("Starting {}…", config.provider.label())),
            reply: String::new(),
            error: None,
            checkpoint: None,
            changes: 0,
            commands: 0,
            reverted: false,
            started_at: Utc::now(),
            finished_at: None,
            input_tokens: 0,
            output_tokens: 0,
        };
        st.runs.push(info.clone());
        st.push(Entry::User { text: prompt.clone(), run: id, source });
        st.streamed = false;
        // The run lives on the session's runtime; this task only reads its events.
        let mut run = Agent::start(session, config, prompt, st.conversation.clone());
        st.active = Some((id, run.handle()));
        drop(st);
        self.notify();
        let Some(mut events) = run.take_events() else { return Ok(info) };
        let host = self.clone();
        session.runtime().spawn(async move {
            while let Some(ev) = events.next().await {
                host.on_run_event(id, ev);
            }
        });
        Ok(info)
    }

    fn on_run_event(&self, id: u64, ev: AgentEvent) {
        let mut st = self.state.lock();
        // A run stopped by a project switch still reports its end: it belongs to no conversation.
        let current = st.active.as_ref().is_some_and(|(a, _)| *a == id);
        match ev {
            AgentEvent::Status { message } => {
                if let Some(r) = st.run_mut(id).filter(|r| !r.finished()) {
                    r.activity = Some(message);
                }
            }
            AgentEvent::Text { delta } if current => {
                st.streamed = true;
                if let Some(r) = st.run_mut(id) {
                    r.reply.push_str(&delta);
                }
                match st.entries.last_mut() {
                    Some(Entry::Assistant { text, run }) if *run == id => text.push_str(&delta),
                    _ => st.push(Entry::Assistant { text: delta.trim_start().to_string(), run: id }),
                }
            }
            AgentEvent::Command { record, result } if current => st.command(*record, result, Some(id)),
            AgentEvent::Usage { input_tokens, output_tokens } => {
                if let Some(r) = st.run_mut(id) {
                    r.input_tokens += input_tokens;
                    r.output_tokens += output_tokens;
                }
            }
            AgentEvent::Done { summary, checkpoint, changes, conversation } if current => {
                st.conversation = conversation;
                if !st.streamed && !summary.trim().is_empty() {
                    if let Some(r) = st.run_mut(id) {
                        r.reply = summary.clone();
                    }
                    st.push(Entry::Assistant { text: summary, run: id });
                }
                finish(&mut st, id, RunState::Done, None, checkpoint, changes);
            }
            AgentEvent::Error { message, checkpoint, changes } if current => {
                if let Some((_, h)) = &st.active {
                    st.conversation = h.conversation();
                }
                finish(&mut st, id, RunState::Error, Some(message), checkpoint, changes);
            }
            AgentEvent::Cancelled { checkpoint, changes } if current => {
                if let Some((_, h)) = &st.active {
                    st.conversation = h.conversation();
                }
                finish(&mut st, id, RunState::Cancelled, None, checkpoint, changes);
            }
            _ => return,
        }
        drop(st);
        self.notify();
    }

    /// Stops the run in progress; its finished edits stay.
    pub fn stop(&self) -> CmdResult<u64> {
        let mut st = self.state.lock();
        let (id, handle) = st.active.clone().ok_or("The agent isn't working on anything.")?;
        handle.cancel();
        if let Some(r) = st.run_mut(id) {
            r.activity = Some("Stopping…".into());
        }
        drop(st);
        self.notify();
        Ok(id)
    }

    /// Puts the project back as it was before a run (the latest revertible one by default).
    pub async fn revert(&self, session: &Arc<Session>, source: Source, run: Option<u64>) -> CmdResult {
        let (id, checkpoint) = {
            let st = self.state.lock();
            let r = match run {
                Some(id) => st.runs.iter().find(|r| r.id == id).ok_or_else(|| format!("No run {id} in this conversation. agent.runs lists them."))?,
                None => st.runs.iter().rev().find(|r| r.can_revert()).ok_or("No agent run to revert: none changed the project, or they are reverted already.")?,
            };
            if !r.finished() {
                return Err(format!("Run {} is still going: stop it first (agent.stop).", r.id));
            }
            if r.reverted {
                return Err(format!("Run {} is reverted already (history.undo brings it back).", r.id));
            }
            (r.id, r.checkpoint.ok_or_else(|| format!("Run {} changed nothing: there is nothing to revert.", r.id))?)
        };
        let result = kimchi_control::call(session, source, "history.revertTo", json!({ "checkpoint": checkpoint })).await?;
        let mut st = self.state.lock();
        if let Some(r) = st.run_mut(id) {
            r.reverted = true;
        }
        st.reverts.push(id);
        st.unreverts.clear();
        drop(st);
        self.notify();
        Ok(json!({ "run": id, "checkpoint": checkpoint, "result": result }))
    }

    /// A fresh thread (runs stay, and can still be reverted).
    pub fn new_conversation(&self) -> CmdResult<()> {
        let mut st = self.state.lock();
        if let Some((id, _)) = &st.active {
            return Err(format!("The agent is working on run {id}: stop it first (agent.stop)."));
        }
        st.clear_thread();
        drop(st);
        self.notify();
        Ok(())
    }

    /// The run once it has ended, or as it is when `timeout` passes.
    pub async fn wait(&self, id: u64, timeout: Duration) -> CmdResult<(RunInfo, bool)> {
        let mut rx = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let run = self.state.lock().runs.iter().find(|r| r.id == id).cloned().ok_or_else(|| format!("Run {id} is gone: the project was switched."))?;
            if run.finished() {
                return Ok((run, false));
            }
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Ok((run, true)),
                Err(_) => return Ok((run, true)),
            }
        }
    }

    /// A run with the commands it ran, for `agent.status` and `agent.send wait=true`.
    fn run_report(&self, run: &RunInfo, timed_out: bool) -> Value {
        let st = self.state.lock();
        let commands: Vec<Value> = st
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Command { record, result, run: Some(r) } if *r == run.id => Some(json!({
                    "seq": record.seq,
                    "command": record.command,
                    "params": record.params,
                    "ok": record.ok,
                    "error": record.error,
                    "result": result.as_ref().or(record.result.as_ref()).filter(|v| v.to_string().len() <= 4096),
                })),
                _ => None,
            })
            .collect();
        let mut v = run.json();
        v["commandList"] = json!(commands);
        if timed_out {
            v["waitTimedOut"] = json!(true);
        }
        v
    }

    fn pick(&self, run: Option<u64>) -> CmdResult<RunInfo> {
        let st = self.state.lock();
        match run {
            Some(id) => st.runs.iter().find(|r| r.id == id).cloned().ok_or_else(|| format!("No run {id} in this conversation. agent.runs lists them.")),
            None => st.runs.last().cloned().ok_or_else(|| "The agent hasn't been asked anything yet on this project. agent.send asks it.".into()),
        }
    }

    async fn command(self: Arc<Self>, session: Arc<Session>, source: Source, command: &'static str, a: Args) -> CmdResult {
        let timeout = Duration::from_secs_f64(a.opt_f64("timeout").unwrap_or(DEFAULT_WAIT).clamp(0.1, 86_400.0));
        match command {
            "agent.providers" => {
                let config = AgentConfig::from_settings(&session.settings().agent);
                let list = crate::provider_status(&session).await;
                Ok(json!({ "provider": config.provider, "model": config.model(), "baseUrl": config.base_url(), "providers": list }))
            }
            "agent.setProvider" => {
                let id = a.str("provider")?;
                let kind = ProviderKind::parse(id).ok_or_else(|| {
                    let ids: Vec<&str> = ProviderKind::ALL.iter().map(|k| k.id()).collect();
                    format!("Unknown agent provider `{id}`. Providers: {}.", ids.join(", "))
                })?;
                let (model, base) = (a.opt_str("model").map(str::trim).map(str::to_string), a.opt_str("baseUrl").map(str::trim).map(str::to_string));
                let s = session.update_settings(|st| {
                    let changed = st.agent.provider != kind.id();
                    st.agent.provider = kind.id().to_string();
                    // A model or address belongs to the provider it was chosen for.
                    if let Some(m) = model.clone().or_else(|| changed.then(String::new)) {
                        st.agent.model = m;
                    }
                    if let Some(b) = base.clone().or_else(|| changed.then(String::new)) {
                        st.agent.base_url = b;
                    }
                })?;
                Ok(json!({ "provider": s.agent.provider, "model": s.agent.model, "baseUrl": s.agent.base_url }))
            }
            "agent.send" => {
                let info = self.send(&session, source, a.str("prompt")?)?;
                if !a.bool_or("wait", false) {
                    return Ok(self.run_report(&info, false));
                }
                let (run, timed_out) = self.wait(info.id, timeout).await?;
                Ok(self.run_report(&run, timed_out))
            }
            "agent.status" => {
                let mut run = self.pick(a.opt_i64("run").map(|v| v.max(0) as u64))?;
                let mut timed_out = false;
                if a.bool_or("wait", false) {
                    (run, timed_out) = self.wait(run.id, timeout).await?;
                }
                let mut v = self.run_report(&run, timed_out);
                v["running"] = json!(self.running());
                Ok(v)
            }
            "agent.runs" => {
                let st = self.state.lock();
                Ok(json!({ "runs": st.runs.iter().map(RunInfo::json).collect::<Vec<_>>(), "running": st.active.as_ref().map(|(id, _)| *id) }))
            }
            "agent.conversation" => {
                let st = self.state.lock();
                let since = a.opt_i64("since").map(|v| v.max(0) as usize).unwrap_or(st.base).max(st.base);
                let entries: Vec<&Entry> = st.entries.iter().skip(since - st.base).collect();
                Ok(json!({ "from": since, "next": st.base + st.entries.len(), "entries": entries, "running": st.active.as_ref().map(|(id, _)| *id) }))
            }
            "agent.stop" => {
                let id = self.stop()?;
                Ok(json!({ "stopping": id }))
            }
            "agent.revert" => self.revert(&session, source, a.opt_i64("run").map(|v| v.max(0) as u64)).await,
            "agent.newConversation" => {
                self.new_conversation()?;
                Ok(json!({ "conversation": "new" }))
            }
            other => Err(format!("`{other}` is not implemented")),
        }
    }
}

fn finish(st: &mut State, id: u64, state: RunState, error: Option<String>, checkpoint: Option<u64>, changes: usize) {
    st.active = None;
    let Some(r) = st.run_mut(id) else { return };
    r.state = state;
    r.activity = None;
    r.error = error.clone();
    r.checkpoint = checkpoint;
    r.changes = changes;
    r.finished_at = Some(Utc::now());
    let (seconds, tokens) = (r.seconds(), r.input_tokens + r.output_tokens);
    st.push(Entry::Outcome { run: id, state, error, changes, seconds, tokens });
}

impl AgentHost for Host {
    fn call(self: Arc<Self>, session: Arc<Session>, source: Source, command: &'static str, args: Args) -> BoxFuture<'static, CmdResult> {
        Box::pin(self.command(session, source, command, args))
    }
}
