//! The tool loop for the API and local providers: ask the model, run the tools
//! it calls through the registry, hand back the results, until it answers.

mod anthropic;
mod ollama;
mod openai;

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::tools::{Ran, spec_for_tool, tool_defs};
use crate::{Conversation, Message, Part, ProviderKind, Role, Run, http};

/// Pictures sent again in later requests to OpenAI and Ollama (older ones become a line of text;
/// Anthropic keeps the thread as it was, which its thinking blocks need).
pub(crate) const RECENT_PICTURES: usize = 4;

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
    /// The model takes pictures. Off for an Ollama model without vision, and for a compatible
    /// server that refused one.
    pub vision: AtomicBool,
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
        let vision = match c.provider {
            ProviderKind::Ollama => ollama::sees(&http, &base, &model).await,
            _ => true,
        };
        Ok(Self { kind: c.provider, http, key, model, base, vision: AtomicBool::new(vision) })
    }

    pub fn sees(&self) -> bool {
        self.vision.load(Ordering::Acquire)
    }

    /// One model request. A compatible server that turns pictures away gets the request again
    /// without them, and none for the rest of the run.
    async fn step(&self, run: &Run, tools: &[crate::ToolDef], messages: &[Message], round: usize) -> Result<Step, String> {
        match self.request(run, tools, messages, round).await {
            Err(e) if self.kind != ProviderKind::Anthropic && self.sees() && has_pictures(messages) && refuses_pictures(&e) => {
                tracing::info!("{}: {} doesn't take pictures ({e}); going on without them", self.base, self.model);
                self.vision.store(false, Ordering::Release);
                self.request(run, tools, messages, round).await
            }
            r => r,
        }
    }

    async fn request(&self, run: &Run, tools: &[crate::ToolDef], messages: &[Message], round: usize) -> Result<Step, String> {
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
    conv.messages.push(Message::user(crate::context::glance(&run.session).frame(&prompt)));
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
        let mut pictures = vec![];
        for call in calls {
            let label = spec_for_tool(&call.name).map_or(call.name.as_str(), |s| s.name);
            run.status(format!("Running {label}…"));
            let Ran { mut output, is_error, pictures: seen } = run.run_tool(&call.name, call.input).await;
            if !seen.is_empty() {
                if api.sees() {
                    output.push_str(if seen.len() == 1 { "\nThe picture is attached." } else { "\nThe pictures are attached." });
                    pictures.extend(seen.into_iter().map(|p| Part::Image { call: Some(call.id.clone()), media_type: p.media_type.into(), data: p.data }));
                } else {
                    output.push_str("\n(This model can't see pictures: judge the result from the project's data, or ask the person to look.)");
                }
            }
            results.parts.push(Part::ToolResult { id: call.id, name: call.name, output, is_error });
            // Keep what ran if the person stops the run between two calls.
            let mut partial = conv.clone();
            partial.messages.push(Message { role: Role::User, parts: results.parts.iter().chain(&pictures).cloned().collect() });
            run.set_conversation(&partial);
        }
        results.parts.extend(pictures);
        conv.messages.push(results);
    }
    Err(format!("Stopped after {steps} model steps without finishing. Finished edits stay; ask it to continue, or split the request."))
}

fn has_pictures(messages: &[Message]) -> bool {
    messages.iter().any(|m| m.parts.iter().any(|p| matches!(p, Part::Image { .. })))
}

/// An error that says the model or server doesn't take images.
fn refuses_pictures(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    ["image", "vision", "multimodal", "image_url"].iter().any(|w| e.contains(w))
}

/// The pictures to send in full: the latest [`RECENT_PICTURES`] (none when `vision` is off).
/// The others are replaced by a line of text in the request.
pub(crate) fn recent_pictures(messages: &[Message], vision: bool) -> std::collections::HashSet<(usize, usize)> {
    if !vision {
        return Default::default();
    }
    let all: Vec<(usize, usize)> =
        messages.iter().enumerate().flat_map(|(i, m)| m.parts.iter().enumerate().filter(|(_, p)| matches!(p, Part::Image { .. })).map(move |(j, _)| (i, j))).collect();
    all.into_iter().rev().take(RECENT_PICTURES).collect()
}

/// Message `i`'s pictures to send (media type, base64), and how many of its others are left out.
pub(crate) fn pictures_of<'a>(m: &'a Message, i: usize, recent: &std::collections::HashSet<(usize, usize)>) -> (Vec<(&'a str, &'a str)>, usize) {
    let (mut shown, mut older) = (vec![], 0);
    for (j, p) in m.parts.iter().enumerate() {
        if let Part::Image { media_type, data, .. } = p {
            if recent.contains(&(i, j)) {
                shown.push((media_type.as_str(), data.as_str()));
            } else {
                older += 1;
            }
        }
    }
    (shown, older)
}

/// The text that goes with the pictures of one round of commands.
pub(crate) fn pictures_note(shown: usize, older: usize) -> String {
    let mut note = match shown {
        0 => String::new(),
        1 => "The picture from the command above.".to_string(),
        n => format!("The {n} pictures from the commands above, in order."),
    };
    if older > 0 {
        if !note.is_empty() {
            note.push(' ');
        }
        note.push_str(&format!("({older} earlier picture{} not shown again.)", if older == 1 { " is" } else { "s are" }));
    }
    note
}

/// Parses streamed tool arguments; empty means no arguments.
pub(crate) fn parse_args(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() { Ok(Value::Object(Default::default())) } else { serde_json::from_str(raw).map_err(|e| e.to_string()) }
}

/// Models installed in Ollama at `base` (for the status and the model picker).
pub(crate) async fn ollama_models(base: &str) -> Result<Vec<String>, String> {
    ollama::models(&http::client(), base).await
}
