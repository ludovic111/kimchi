//! kimchi-agent: the engine behind the Agent panel.
//!
//! The panel runs the model the person already has: their Claude Code or Codex
//! CLI, an Anthropic or OpenAI API key, or a local model in Ollama. Whatever the
//! provider, the agent acts only through kimchi's command registry
//! (`kimchi_control::call`), so permissions (`settings.agent.permissions`),
//! validation and the one undo history behave exactly as for MCP and the CLI.
//!
//! * API and local providers get every registry command as a tool
//!   (`family_verb`) and are run here, as [`Source::Agent`].
//! * Claude Code and Codex run as child processes with `kimchi-mcp --live`
//!   attached; their commands reach the app through the bridge as
//!   [`Source::Mcp`] and are picked up from the session's event stream.
//!
//! A run is started with [`Agent::start`] and reports through [`AgentEvent`]s:
//! streamed text, one card per command, then `Done`/`Error`/`Cancelled` with the
//! checkpoint taken before the first change, which [`revert`] puts back.

mod api;
mod cli;
mod host;
mod http;
mod status;
mod tools;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::channel::mpsc;
use kimchi_control::session::Event;
use kimchi_control::settings::AgentSettings;
use kimchi_control::{CmdResult, CommandRecord, Session, Source};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub use cli::{cli_executable, mcp_executable};
pub use host::{Entry, Host, RunInfo, RunState, Snapshot};
pub use status::{ProviderStatus, provider_status};
pub use tools::{SYSTEM_PROMPT, TOOL_OUTPUT_LIMIT, ToolDef, tool_defs};

/// Most model round trips in one run before it stops and says so.
pub const MAX_STEPS: usize = 40;

/// Messages kept when a conversation continues; older ones are dropped at the
/// start of a turn (never in the middle of one).
pub const MAX_HISTORY: usize = 80;

// ---- configuration --------------------------------------------------------

/// Which model runs the agent (`settings.agent.provider`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "ollama")]
    Ollama,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 5] = [ProviderKind::ClaudeCode, ProviderKind::Codex, ProviderKind::Anthropic, ProviderKind::OpenAi, ProviderKind::Ollama];

    /// The id used in settings.
    pub fn id(self) -> &'static str {
        match self {
            ProviderKind::ClaudeCode => "claude-code",
            ProviderKind::Codex => "codex",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Ollama => "ollama",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::ClaudeCode => "Claude Code",
            ProviderKind::Codex => "Codex",
            ProviderKind::Anthropic => "Anthropic API",
            ProviderKind::OpenAi => "OpenAI API",
            ProviderKind::Ollama => "Ollama (on this computer)",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase();
        Some(match id.as_str() {
            "claude-code" | "claude" | "claudecode" => ProviderKind::ClaudeCode,
            "codex" => ProviderKind::Codex,
            "anthropic" => ProviderKind::Anthropic,
            "openai" => ProviderKind::OpenAi,
            "ollama" | "local" => ProviderKind::Ollama,
            _ => return None,
        })
    }

    /// Model used when `settings.agent.model` is empty. Empty for the CLIs (their own
    /// default) and Ollama (the first installed model).
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "claude-sonnet-5-5",
            ProviderKind::OpenAi => "gpt-5",
            ProviderKind::ClaudeCode | ProviderKind::Codex | ProviderKind::Ollama => "",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            ProviderKind::Ollama => "http://127.0.0.1:11434",
            ProviderKind::ClaudeCode | ProviderKind::Codex => "",
        }
    }

    /// Keychain id of the API key, and the environment variable read when it is missing.
    pub fn key_source(self) -> Option<(&'static str, &'static str)> {
        match self {
            ProviderKind::Anthropic => Some(("anthropic", "ANTHROPIC_API_KEY")),
            ProviderKind::OpenAi => Some(("openai", "OPENAI_API_KEY")),
            _ => None,
        }
    }

    /// Runs as the person's installed CLI, connected back through `kimchi-mcp --live`.
    pub fn is_cli(self) -> bool {
        matches!(self, ProviderKind::ClaudeCode | ProviderKind::Codex)
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// What a run uses, read from `settings.agent`.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentConfig {
    pub provider: ProviderKind,
    /// Empty: the provider's default.
    pub model: String,
    /// Empty: the provider's default (API providers and Ollama; also lets an
    /// OpenAI-compatible server or a proxy stand in).
    pub base_url: String,
    /// Model round trips before the run stops (API and local providers).
    pub max_steps: usize,
}

impl AgentConfig {
    pub fn new(provider: ProviderKind) -> Self {
        Self { provider, model: String::new(), base_url: String::new(), max_steps: MAX_STEPS }
    }

    /// An unknown provider id falls back to Claude Code, the settings default.
    pub fn from_settings(s: &AgentSettings) -> Self {
        Self {
            provider: ProviderKind::parse(&s.provider).unwrap_or(ProviderKind::ClaudeCode),
            model: s.model.trim().to_string(),
            base_url: s.base_url.trim().to_string(),
            max_steps: MAX_STEPS,
        }
    }

    /// The configured model, else the provider's default (may be empty, see [`ProviderKind::default_model`]).
    pub fn model(&self) -> String {
        if self.model.trim().is_empty() { self.provider.default_model().to_string() } else { self.model.trim().to_string() }
    }

    /// The configured base URL without a trailing slash, else the provider's default.
    pub fn base_url(&self) -> String {
        let b = self.base_url.trim();
        let b = if b.is_empty() { self.provider.default_base_url() } else { b };
        b.trim_end_matches('/').to_string()
    }

    /// The API key: the keychain (`anthropic` / `openai`), else the environment.
    pub fn api_key(&self, session: &Session) -> Option<String> {
        let (id, env) = self.provider.key_source()?;
        session.secret(id).or_else(|| std::env::var(env).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()))
    }
}

// ---- conversation ----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Provider-neutral message content, so a conversation survives switching provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    /// `name` is the tool name (`clip_addText`).
    ToolUse { id: String, name: String, input: Value },
    ToolResult { id: String, name: String, output: String, is_error: bool },
    /// A provider block replayed verbatim to the same provider only (Anthropic thinking blocks).
    Opaque { provider: ProviderKind, block: Value },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, parts: vec![Part::Text { text: text.into() }] }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, parts: vec![Part::Text { text: text.into() }] }
    }

    /// The message's text parts, joined.
    pub fn text(&self) -> String {
        let texts: Vec<&str> = self.parts.iter().filter_map(|p| if let Part::Text { text } = p { Some(text.as_str()) } else { None }).collect();
        texts.join("\n\n")
    }
}

/// A CLI's own conversation, continued with `--resume` (Claude Code) or `exec resume` (Codex).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CliSession {
    pub provider: ProviderKind,
    pub id: String,
}

/// The panel's thread. Pass it back to [`Agent::start`] for a follow-up; a run
/// returns the updated one in [`AgentEvent::Done`] and [`RunHandle::conversation`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub cli_session: Option<CliSession>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.cli_session.is_none()
    }

    /// Readies the thread for a new turn: answers tool calls a stopped run left
    /// open, and drops the oldest messages beyond [`MAX_HISTORY`]. History is
    /// otherwise only appended to, which keeps prompt caches and Anthropic's
    /// thinking blocks valid; when it must be trimmed, earlier thinking blocks are
    /// dropped with it so no block is replayed after a changed prefix.
    pub(crate) fn prepare_turn(&mut self) {
        if let Some(ai) = self.messages.iter().rposition(|m| m.role == Role::Assistant) {
            let answers = self.messages.get(ai + 1).filter(|m| m.role == Role::User && m.parts.iter().all(|p| matches!(p, Part::ToolResult { .. })));
            let has_answers = answers.is_some();
            let answered: Vec<&str> = answers.into_iter().flat_map(|m| &m.parts).filter_map(|p| if let Part::ToolResult { id, .. } = p { Some(id.as_str()) } else { None }).collect();
            let open: Vec<Part> = self.messages[ai]
                .parts
                .iter()
                .filter_map(|p| match p {
                    Part::ToolUse { id, name, .. } if !answered.contains(&id.as_str()) => {
                        Some(Part::ToolResult { id: id.clone(), name: name.clone(), output: "Stopped by the person before this ran.".into(), is_error: true })
                    }
                    _ => None,
                })
                .collect();
            if !open.is_empty() {
                if has_answers {
                    self.messages[ai + 1].parts.extend(open);
                } else {
                    self.messages.insert(ai + 1, Message { role: Role::User, parts: open });
                }
            }
        }
        if self.messages.len() > MAX_HISTORY {
            // Start on a person's prompt, never on tool results or a reply.
            let cut = (self.messages.len() - MAX_HISTORY..self.messages.len())
                .find(|&i| self.messages[i].role == Role::User && self.messages[i].parts.iter().any(|p| matches!(p, Part::Text { .. })))
                .unwrap_or(self.messages.len());
            self.messages.drain(..cut);
            for m in &mut self.messages {
                m.parts.retain(|p| !matches!(p, Part::Opaque { .. }));
            }
        }
    }
}

// ---- events -----------------------------------------------------------------

/// What a run reports, in order. Exactly one of `Done`, `Error` or `Cancelled` ends it.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A short progress line for the panel's status ("Thinking with claude-sonnet-5-5…").
    Status { message: String },
    /// Streamed reply text, to append to the current assistant bubble.
    Text { delta: String },
    /// One command the agent ran (a card): who, what, parameters, ok or error.
    /// `result` is the command's JSON answer when the agent ran it in-process
    /// (API and local providers); the CLI providers' answers stay in the CLI.
    Command { record: Box<CommandRecord>, result: Option<Value> },
    /// Tokens used by one model request, when the provider reports them.
    Usage { input_tokens: u64, output_tokens: u64 },
    /// The run finished. `summary` is the final reply; `checkpoint` (set when the
    /// run changed something) is what "Revert this run" passes to [`revert`].
    Done { summary: String, checkpoint: Option<u64>, changes: usize, conversation: Conversation },
    /// The run failed; finished edits stay (and `checkpoint` can revert them).
    Error { message: String, checkpoint: Option<u64>, changes: usize },
    /// Stopped by [`AgentRun::cancel`]; finished edits stay.
    Cancelled { checkpoint: Option<u64>, changes: usize },
}

impl AgentEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentEvent::Done { .. } | AgentEvent::Error { .. } | AgentEvent::Cancelled { .. })
    }
}

// ---- runs -------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    checkpoint: Mutex<Option<u64>>,
    changes: AtomicUsize,
    conversation: Mutex<Conversation>,
    finished: AtomicBool,
}

/// A cloneable handle on a run: cancel it from anywhere, read its state.
#[derive(Clone)]
pub struct RunHandle {
    cancel: CancellationToken,
    shared: Arc<Shared>,
}

impl RunHandle {
    /// Stops the run: the model request is dropped and CLI processes (with their
    /// `kimchi-mcp`) are killed. Finished edits stay in the undo history.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }

    /// The checkpoint taken before the run's first change, if any.
    pub fn checkpoint(&self) -> Option<u64> {
        *self.shared.checkpoint.lock()
    }

    /// Successful commands that could change the project, so far.
    pub fn changes(&self) -> usize {
        self.shared.changes.load(Ordering::Acquire)
    }

    /// The thread as it stands (complete once the run is finished).
    pub fn conversation(&self) -> Conversation {
        self.shared.conversation.lock().clone()
    }
}

/// One agent run. Read its events with [`next_event`](Self::next_event) or move
/// the receiver elsewhere with [`take_events`](Self::take_events).
pub struct AgentRun {
    handle: RunHandle,
    events: Option<mpsc::UnboundedReceiver<AgentEvent>>,
}

impl AgentRun {
    pub fn handle(&self) -> RunHandle {
        self.handle.clone()
    }

    pub fn cancel(&self) {
        self.handle.cancel();
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub fn checkpoint(&self) -> Option<u64> {
        self.handle.checkpoint()
    }

    pub fn conversation(&self) -> Conversation {
        self.handle.conversation()
    }

    /// The event stream (a `futures` stream, usable from any executor). `None` once taken.
    pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<AgentEvent>> {
        self.events.take()
    }

    /// The next event; `None` after the terminal one (or once the receiver was taken).
    pub async fn next_event(&mut self) -> Option<AgentEvent> {
        use futures::StreamExt;
        self.events.as_mut()?.next().await
    }
}

pub struct Agent;

impl Agent {
    /// Starts a run on the session's Tokio runtime and returns at once. Call from
    /// any thread. `conversation` is the thread so far (`Conversation::new()` for
    /// a fresh one).
    pub fn start(session: &Arc<Session>, config: AgentConfig, prompt: impl Into<String>, conversation: Conversation) -> AgentRun {
        let (mut run, run_handle) = Run::new(session, config, &conversation);
        let cancel = run.cancel.clone();
        let prompt = prompt.into();
        session.runtime().spawn(async move {
            let outcome = tokio::select! {
                r = run.execute(prompt, conversation) => Some(r),
                _ = cancel.cancelled() => None,
            };
            run.finish(outcome);
        });
        run_handle
    }
}

/// "Revert this run": puts the project back as it was at `checkpoint`, as one
/// new undo step (the revert can be undone too). The person's action, so it runs
/// as the window and is not subject to agent permissions.
pub async fn revert(session: &Arc<Session>, checkpoint: u64) -> CmdResult {
    kimchi_control::call(session, Source::Window, "history.revertTo", json!({ "checkpoint": checkpoint })).await
}

/// The state a provider works with while a run is going.
pub(crate) struct Run {
    pub session: Arc<Session>,
    pub config: AgentConfig,
    events: mpsc::UnboundedSender<AgentEvent>,
    pub cancel: CancellationToken,
    shared: Arc<Shared>,
    /// Session events, for the command records of this run.
    pub commands: broadcast::Receiver<Event>,
    /// Text was streamed since the last break.
    text_started: AtomicBool,
    /// The next text starts a new paragraph (a new model message).
    text_break: AtomicBool,
}

impl Run {
    pub(crate) fn new(session: &Arc<Session>, config: AgentConfig, conversation: &Conversation) -> (Self, AgentRun) {
        let (tx, rx) = mpsc::unbounded();
        let cancel = CancellationToken::new();
        let shared = Arc::new(Shared { conversation: Mutex::new(conversation.clone()), ..Default::default() });
        let handle = RunHandle { cancel: cancel.clone(), shared: shared.clone() };
        let run = Run {
            session: session.clone(),
            config,
            events: tx,
            cancel,
            shared,
            commands: session.subscribe(),
            text_started: AtomicBool::new(false),
            text_break: AtomicBool::new(false),
        };
        (run, AgentRun { handle, events: Some(rx) })
    }

    pub fn emit(&self, event: AgentEvent) {
        let _ = self.events.unbounded_send(event);
    }

    pub fn status(&self, message: impl Into<String>) {
        self.emit(AgentEvent::Status { message: message.into() });
    }

    pub fn text(&self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let delta = if self.text_break.swap(false, Ordering::AcqRel) { format!("\n\n{delta}") } else { delta.to_string() };
        self.text_started.store(true, Ordering::Release);
        self.emit(AgentEvent::Text { delta });
    }

    /// A new model message begins: its text is a new paragraph in the reply bubble.
    pub fn break_text(&self) {
        if self.text_started.load(Ordering::Acquire) {
            self.text_break.store(true, Ordering::Release);
        }
    }

    pub fn text_break_pending(&self) -> bool {
        self.text_break.load(Ordering::Acquire)
    }

    pub fn usage(&self, input_tokens: u64, output_tokens: u64) {
        if input_tokens > 0 || output_tokens > 0 {
            self.emit(AgentEvent::Usage { input_tokens, output_tokens });
        }
    }

    pub fn set_conversation(&self, c: &Conversation) {
        *self.shared.conversation.lock() = c.clone();
    }

    /// Takes the run's checkpoint once, before its first change, if a project is open.
    pub async fn ensure_checkpoint(&self) {
        if self.shared.checkpoint.lock().is_some() || !self.session.is_open() {
            return;
        }
        // The panel takes it for the person (a checkpoint changes nothing), so it
        // works whatever the agent permissions say.
        if let Ok(v) = kimchi_control::call(&self.session, Source::Window, "history.checkpoint", json!({})).await {
            *self.shared.checkpoint.lock() = v["checkpoint"].as_u64();
        }
    }

    /// A command record from the session's stream: counts changes and shows the card.
    pub fn command(&self, record: CommandRecord, result: Option<Value>) {
        if record.ok && record.mutates {
            self.shared.changes.fetch_add(1, Ordering::AcqRel);
        }
        self.emit(AgentEvent::Command { record: Box::new(record), result });
    }

    /// Forwards every queued command record from `source` (the CLI providers' MCP calls).
    pub fn drain_commands(&mut self, source: Source) {
        loop {
            match self.commands.try_recv() {
                Ok(Event::Command { record }) if record.source == source => self.command(record, None),
                Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    }

    async fn execute(&mut self, prompt: String, conversation: Conversation) -> Result<String, String> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("Type a request first.".into());
        }
        if !self.session.settings().agent.permissions.enabled {
            return Err("Agents are turned off in Settings › Agent › Permissions.".into());
        }
        // Commands from before this run are not its own.
        while !matches!(self.commands.try_recv(), Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed)) {}
        if self.config.provider.is_cli() { cli::run(self, prompt, conversation).await } else { api::run(self, prompt, conversation).await }
    }

    fn finish(&mut self, outcome: Option<Result<String, String>>) {
        if self.config.provider.is_cli() {
            self.drain_commands(Source::Mcp);
        }
        let checkpoint = *self.shared.checkpoint.lock();
        let changes = self.shared.changes.load(Ordering::Acquire);
        // A checkpoint with nothing after it is nothing to revert.
        let checkpoint = checkpoint.filter(|_| changes > 0);
        let event = match outcome {
            None => AgentEvent::Cancelled { checkpoint, changes },
            Some(Ok(summary)) => AgentEvent::Done { summary, checkpoint, changes, conversation: self.shared.conversation.lock().clone() },
            Some(Err(message)) => AgentEvent::Error { message, checkpoint, changes },
        };
        self.shared.finished.store(true, Ordering::Release);
        self.emit(event);
        self.events.close_channel();
    }
}
