//! The tool loop for the API and local providers: ask the model, run the tools
//! it calls through the registry, hand back the results, until it answers.

mod anthropic;
pub(crate) mod bedrock;
mod gemini;
mod ollama;
mod openai;

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::providers::{Quirks, Wire};
use crate::tools::{Ran, ToolSet, spec_for_tool};
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
    pub wire: Wire,
    pub http: reqwest::Client,
    pub key: Option<String>,
    pub model: String,
    /// Without a trailing slash. For Azure, the whole Chat Completions URL.
    pub base: String,
    pub bedrock: Option<bedrock::Target>,
    /// The model takes pictures. Off for an Ollama model without vision, and for a compatible
    /// server that refused one.
    pub vision: AtomicBool,
}

/// The tools a wire takes at most, and whether only the short list goes (small local models).
fn tool_budget(wire: Wire) -> (Option<usize>, bool) {
    match wire {
        Wire::Chat(q) => (q.tool_limit, q.compact),
        Wire::Ollama => (None, true),
        Wire::Gemini | Wire::Anthropic | Wire::Bedrock | Wire::Cli => (None, false),
    }
}

/// The Chat Completions URL of an Azure OpenAI resource. `base` is the resource's name, its
/// address, its v1 address, or a whole deployment URL with an `api-version` (the classic API).
pub(crate) fn azure_url(base: &str, deployment: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.contains("api-version=") {
        // https://res.openai.azure.com/openai/deployments/{d}/chat/completions?api-version=2024-10-21,
        // or the resource address with only the version.
        let (addr, query) = b.split_once('?').unwrap_or((b, ""));
        let resource = addr.split("/openai").next().unwrap_or(addr);
        return format!("{resource}/openai/deployments/{deployment}/chat/completions?{query}");
    }
    format!("{}/chat/completions", azure_v1(b))
}

/// The v1 API root of an Azure resource (`https://{name}.openai.azure.com/openai/v1`).
pub(crate) fn azure_v1(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    let b = b.split('?').next().unwrap_or(b);
    if !b.contains("://") && !b.contains('.') && !b.is_empty() {
        return format!("https://{b}.openai.azure.com/openai/v1");
    }
    let addr = if b.contains("://") { b.to_string() } else { format!("https://{b}") };
    let root = addr.split("/openai").next().unwrap_or(&addr).to_string();
    format!("{root}/openai/v1")
}

impl Api {
    pub(crate) async fn prepare(run: &Run) -> Result<Self, String> {
        let c = &run.config;
        let info = c.provider.info();
        let http = http::client();
        let mut base = c.base_url();
        let key = c.api_key(&run.session);
        let mut model = c.model();
        let mut wire = info.wire;
        let mut bedrock = None;
        let missing_key = || {
            let spec = info.key.expect("a provider with a required key has a key spec");
            let env = spec.env.first().map(|e| format!(", or set {e}")).unwrap_or_default();
            let url = spec.url.map(|u| format!(" (get one at {u})")).unwrap_or_default();
            format!("No {} key. Paste one in Settings › Agent{url}{env}.", info.label)
        };
        match c.provider {
            ProviderKind::ClaudeCode | ProviderKind::Codex | ProviderKind::GeminiCli => return Err("This provider runs as a CLI.".into()),
            // Another address is a compatible server, which may not need a key.
            ProviderKind::OpenAi if base != info.default_base_url => wire = Wire::Chat(Quirks { tool_limit: Some(128), usage: false, ..Quirks::STANDARD }),
            ProviderKind::Bedrock => {
                let target = bedrock::resolve(run.session.secret("bedrock").as_deref(), &c.base_url)?;
                if c.model.trim().is_empty() {
                    model = bedrock::default_model(&target.region);
                }
                bedrock = Some(target);
            }
            ProviderKind::AzureOpenAi => {
                if c.base_url.trim().is_empty() {
                    return Err("Give your Azure OpenAI resource (its name or address) in Settings › Agent.".into());
                }
                if model.is_empty() {
                    return Err("Type the name of your Azure OpenAI deployment as the model in Settings › Agent.".into());
                }
                if key.is_none() {
                    return Err(missing_key());
                }
                base = azure_url(&c.base_url, &model);
            }
            ProviderKind::OpenAiCompatible if c.base_url.trim().is_empty() => {
                return Err("Give the server's address in Settings › Agent (for example http://127.0.0.1:8000/v1).".into());
            }
            ProviderKind::Ollama if model.is_empty() => {
                let models = crate::models::ollama(&http, &base).await?;
                model = models.iter().find(|m| m.tools == Some(true)).or(models.first()).map(|m| m.id.clone()).ok_or_else(|| {
                    "Ollama has no models yet. Pull one that can use tools (for example `ollama pull qwen3`), then choose it in Settings › Agent.".to_string()
                })?;
            }
            ProviderKind::LmStudio | ProviderKind::OpenAiCompatible if model.is_empty() => {
                let models = crate::models::fetch(&http, c.provider, &base, key.as_deref()).await?;
                model = models.iter().find(|m| m.loaded == Some(true) && m.tools != Some(false)).or(models.first()).map(|m| m.id.clone()).ok_or_else(|| {
                    format!("{} has no model loaded. Load one (one that can use tools), or choose it in Settings › Agent.", info.label)
                })?;
            }
            _ => {}
        }
        let other_server = c.provider == ProviderKind::OpenAi && base != info.default_base_url;
        if info.key.is_some_and(|k| k.required) && key.is_none() && bedrock.is_none() && !other_server {
            return Err(missing_key());
        }
        if model.is_empty() {
            return Err(format!("Choose a model for {} in Settings › Agent.", info.label));
        }
        let vision = match wire {
            Wire::Ollama => ollama::sees(&http, &base, &model).await,
            // Pictures aren't sent to Bedrock yet.
            Wire::Bedrock => false,
            _ => true,
        };
        Ok(Self { kind: c.provider, wire, http, key, model, base, bedrock, vision: AtomicBool::new(vision) })
    }

    /// How errors name the service: "OpenAI API", or the address of a custom server.
    pub(crate) fn label(&self) -> String {
        let info = self.kind.info();
        if info.default_base_url.is_empty() || self.base == info.default_base_url { info.label.to_string() } else { format!("{} at {}", info.label, self.base.split('?').next().unwrap_or("")) }
    }

    /// The request with the key, as the provider wants it.
    pub(crate) fn authorized(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match (&self.key, self.wire) {
            (None, _) => r,
            (Some(k), Wire::Chat(q)) if q.api_key_header => r.header("api-key", k),
            (Some(k), _) => r.bearer_auth(k),
        }
    }

    pub fn sees(&self) -> bool {
        self.vision.load(Ordering::Acquire)
    }

    /// One model request. A compatible server that turns pictures away gets the request again
    /// without them, and none for the rest of the run.
    async fn step(&self, run: &Run, tools: &ToolSet, messages: &[Message], round: usize) -> Result<Step, String> {
        match self.request(run, tools, messages, round).await {
            Err(e) if self.kind != ProviderKind::Anthropic && self.sees() && has_pictures(messages) && refuses_pictures(&e) => {
                tracing::info!("{}: {} doesn't take pictures ({e}); going on without them", self.base, self.model);
                self.vision.store(false, Ordering::Release);
                self.request(run, tools, messages, round).await
            }
            r => r,
        }
    }

    async fn request(&self, run: &Run, tools: &ToolSet, messages: &[Message], round: usize) -> Result<Step, String> {
        match self.wire {
            Wire::Anthropic => anthropic::step(self, run, tools, messages).await,
            Wire::Chat(q) => openai::step(self, q, run, tools, messages).await,
            Wire::Gemini => gemini::step(self, run, tools, messages, round).await,
            Wire::Bedrock => bedrock::step(self, run, tools, messages).await,
            Wire::Ollama => ollama::step(self, run, tools, messages, round).await,
            Wire::Cli => Err("This provider runs as a CLI.".into()),
        }
    }
}

pub(crate) async fn run(run: &mut Run, prompt: String, mut conv: Conversation) -> Result<String, String> {
    let api = Api::prepare(run).await?;
    let (limit, compact) = tool_budget(api.wire);
    let tools = ToolSet::new(limit, compact);
    conv.prepare_turn();
    // The live context: in front of the request, then before each later step when it changed.
    let mut seen = crate::context::glance_since(&run.session, Some(u64::MAX), &[]);
    conv.messages.push(Message::user(seen.frame(&prompt)));
    run.set_conversation(&conv);
    let steps = run.config.max_steps.max(1);
    for round in 0..steps {
        if round > 0 {
            refresh_context(run, &mut conv, &mut seen);
        }
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
            let label = match (call.name.as_str(), &call.input) {
                (crate::tools::RUN_TOOL, Ok(v)) => v["command"].as_str().unwrap_or("a command").to_string(),
                (name, _) => spec_for_tool(name).map_or(name.to_string(), |s| s.name.to_string()),
            };
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

/// Before a model step: when the project or the window changed since the context the model last
/// saw (its own edits, or the person's meanwhile), an updated `<context>` block goes after the
/// tool results, with what others changed.
pub(crate) fn refresh_context(run: &Run, conv: &mut Conversation, seen: &mut crate::context::Glance) {
    let now = crate::context::glance_since(&run.session, Some(seen.seq), &[kimchi_control::Source::Agent]);
    let changed = now.lines != seen.lines || now.changes.is_some();
    if changed && let Some(last) = conv.messages.last_mut().filter(|m| m.role == Role::User) {
        last.parts.push(Part::Text { text: format!("<context>\nUpdated before this step.\n{}\n</context>", now.text()) });
        run.set_conversation(conv);
    }
    *seen = now;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_addresses_take_every_form() {
        assert_eq!(azure_url("my-res", "gpt5"), "https://my-res.openai.azure.com/openai/v1/chat/completions");
        assert_eq!(azure_url("https://my-res.openai.azure.com/", "gpt5"), "https://my-res.openai.azure.com/openai/v1/chat/completions");
        assert_eq!(azure_url("https://my-res.services.ai.azure.com/openai/v1", "d"), "https://my-res.services.ai.azure.com/openai/v1/chat/completions");
        assert_eq!(
            azure_url("https://my-res.openai.azure.com/openai/deployments/old/chat/completions?api-version=2024-10-21", "gpt5"),
            "https://my-res.openai.azure.com/openai/deployments/gpt5/chat/completions?api-version=2024-10-21"
        );
        assert_eq!(azure_url("https://my-res.openai.azure.com?api-version=2024-10-21", "d"), "https://my-res.openai.azure.com/openai/deployments/d/chat/completions?api-version=2024-10-21");
    }
}
