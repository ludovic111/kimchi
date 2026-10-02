//! The tool loop for the API and local providers: ask the model, run the tools
//! it calls through the registry, hand back the results, until it answers.

mod anthropic;
mod ollama;
mod openai;

use serde_json::Value;

use crate::tools::{spec_for_tool, tool_defs};
use crate::{Conversation, Message, Part, ProviderKind, Role, Run, http};

/// One tool call the model asked for. `input` is an error when its JSON didn't parse.
pub(crate) struct Call {
    pub id: String,
    pub name: String,
    pub input: Result<Value, String>,
}

/// One model response: the assistant message to keep, and the calls to run.
pub(crate) struct Step {
    pub parts: Vec<Part>,
    pub calls: Vec<Call>,
}

/// A ready provider: key, model and address resolved.
pub(crate) struct Api {
    pub kind: ProviderKind,
    pub http: reqwest::Client,
    pub key: Option<String>,
    pub model: String,
    pub base: String,
}

impl Api {
    async fn prepare(run: &Run) -> Result<Self, String> {
        let c = &run.config;
        let http = http::client();
        let base = c.base_url();
        let key = c.api_key(&run.session);
        let mut model = c.model();
        match c.provider {
            ProviderKind::Anthropic if key.is_none() => {
                return Err("No Anthropic API key. Add one in Settings › Agent, or set ANTHROPIC_API_KEY.".into());
            }
            // A compatible server at another address may not need one.
            ProviderKind::OpenAi if key.is_none() && base == ProviderKind::OpenAi.default_base_url() => {
                return Err("No OpenAI API key. Add one in Settings › Agent, or set OPENAI_API_KEY.".into());
            }
            ProviderKind::Ollama if model.is_empty() => {
                let models = ollama::models(&http, &base).await?;
                model = models.into_iter().next().ok_or_else(|| {
                    "Ollama has no models yet. Pull one that supports tools (for example `ollama pull qwen3`), then choose it in Settings › Agent.".to_string()
                })?;
            }
            _ => {}
        }
        if model.is_empty() {
            return Err(format!("Choose a model for {} in Settings › Agent.", c.provider.label()));
        }
        Ok(Self { kind: c.provider, http, key, model, base })
    }

    async fn step(&self, run: &Run, tools: &[crate::ToolDef], messages: &[Message], round: usize) -> Result<Step, String> {
        match self.kind {
            ProviderKind::Anthropic => anthropic::step(self, run, tools, messages).await,
            ProviderKind::OpenAi => openai::step(self, run, tools, messages).await,
            ProviderKind::Ollama => ollama::step(self, run, tools, messages, round).await,
            ProviderKind::ClaudeCode | ProviderKind::Codex => Err("This provider runs as a CLI.".into()),
        }
    }
}

pub(crate) async fn run(run: &mut Run, prompt: String, mut conv: Conversation) -> Result<String, String> {
    let api = Api::prepare(run).await?;
    let tools = tool_defs();
    conv.prepare_turn();
    conv.messages.push(Message::user(prompt));
    run.set_conversation(&conv);
    let steps = run.config.max_steps.max(1);
    for round in 0..steps {
        run.status(format!("Thinking with {}…", api.model));
        run.break_text();
        let Step { parts, calls } = api.step(run, &tools, &conv.messages, round).await?;
        conv.messages.push(Message { role: Role::Assistant, parts });
        run.set_conversation(&conv);
        if calls.is_empty() {
            return Ok(conv.messages.last().map(Message::text).unwrap_or_default());
        }
        let mut results = Message { role: Role::User, parts: vec![] };
        for call in calls {
            let label = spec_for_tool(&call.name).map_or(call.name.as_str(), |s| s.name);
            run.status(format!("Running {label}…"));
            let (output, is_error) = run.run_tool(&call.name, call.input).await;
            results.parts.push(Part::ToolResult { id: call.id, name: call.name, output, is_error });
            // Keep what ran if the person stops the run between two calls.
            let mut partial = conv.clone();
            partial.messages.push(results.clone());
            run.set_conversation(&partial);
        }
        conv.messages.push(results);
    }
    Err(format!("Stopped after {steps} model steps without finishing. Finished edits stay; ask it to continue, or split the request."))
}

/// Parses streamed tool arguments; empty means no arguments.
pub(crate) fn parse_args(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() { Ok(Value::Object(Default::default())) } else { serde_json::from_str(raw).map_err(|e| e.to_string()) }
}

/// Models installed in Ollama at `base` (for the status and the model picker).
pub(crate) async fn ollama_models(base: &str) -> Result<Vec<String>, String> {
    ollama::models(&http::client(), base).await
}
