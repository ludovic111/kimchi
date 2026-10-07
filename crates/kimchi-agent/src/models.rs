//! The models each provider offers, for the model pickers: fetched from the provider's own
//! list where it has one (cached for a few hours, refreshed on request), else a short built-in
//! list. Where the list says so, a model that can't call tools is marked; the agent needs tools.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use kimchi_control::Session;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;

use crate::api::bedrock;
use crate::providers::{Group, Wire};
use crate::{AgentConfig, ProviderKind};

/// How long a fetched list is used before it is fetched again.
const TTL: Duration = Duration::from_secs(6 * 3600);

/// Lists longer than this are cut (OpenRouter has hundreds; the field still takes any id).
const MAX_MODELS: usize = 400;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    /// The provider's name for it, when it has one besides the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether it can call tools, when the provider says (`false`: it can't run the agent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    /// Context window in tokens, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// Loaded in the local server now (LM Studio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded: Option<bool>,
}

impl ModelInfo {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), name: None, tools: None, context: None, loaded: None }
    }
}

/// What `agent.models` answers.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelList {
    pub provider: ProviderKind,
    /// `api`: from the provider; `builtin`: kimchi's short list (no list to fetch, or it failed).
    pub source: &'static str,
    pub models: Vec<ModelInfo>,
    /// The model used when none is chosen (empty: the provider decides).
    pub default_model: String,
    pub fetched_at: DateTime<Utc>,
    /// Why the provider's list couldn't be fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

type Cache = Mutex<HashMap<String, (Instant, ModelList)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// A short fingerprint of a key, so a new key fetches a new list without the key being kept.
fn fingerprint(key: Option<&str>) -> String {
    key.map(|k| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in k.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:x}")
    })
    .unwrap_or_default()
}

fn builtin(kind: ProviderKind) -> Vec<ModelInfo> {
    kind.info().models.iter().map(|m| ModelInfo::new(*m)).collect()
}

/// What is known of `kind`'s models without asking it: the last list fetched, else the built-in one.
pub fn known(kind: ProviderKind) -> (Vec<ModelInfo>, &'static str) {
    let prefix = format!("{kind}|");
    let cached = cache().lock().iter().filter(|(k, _)| k.starts_with(&prefix)).max_by_key(|(_, (at, _))| *at).map(|(_, (_, l))| (l.models.clone(), l.source));
    cached.unwrap_or_else(|| (builtin(kind), "builtin"))
}

/// The models of `kind` (the chosen provider's address and key, or its defaults for another).
pub async fn list(session: &Arc<Session>, kind: ProviderKind, refresh: bool) -> ModelList {
    let settings = session.settings().agent;
    let mut config = AgentConfig::from_settings(&settings);
    if config.provider != kind {
        config = AgentConfig::new(kind);
    }
    let key = config.api_key(session);
    let base = config.base_url();
    let saved_bedrock = session.secret("bedrock");
    let cache_key = format!("{kind}|{base}|{}|{}", config.base_url, fingerprint(key.as_deref().or(saved_bedrock.as_deref())));
    if !refresh && let Some((at, list)) = cache().lock().get(&cache_key).cloned() && at.elapsed() < TTL {
        return list;
    }
    let mut default_model = kind.default_model().to_string();
    let http = crate::http::client();
    let fetched: Result<Vec<ModelInfo>, String> = match kind {
        ProviderKind::ClaudeCode | ProviderKind::Codex | ProviderKind::GeminiCli => Err(String::new()),
        ProviderKind::AzureOpenAi => Err("Azure doesn't list deployments for a key: type your deployment's name.".into()),
        ProviderKind::Bedrock => match bedrock::resolve(saved_bedrock.as_deref(), &config.base_url) {
            Ok(target) => {
                default_model = bedrock::default_model(&target.region);
                bedrock_models(&http, &target).await
            }
            Err(e) => Err(e),
        },
        ProviderKind::Ollama => ollama(&http, &base).await,
        ProviderKind::Lsuite => match kimchi_control::account::load() {
            Some(a) => fetch(&http, kind, &kimchi_control::account::ai_base(&a.server), Some(&a.token)).await,
            None => Err("Sign in to lsuite AI to see your plan's models.".into()),
        },
        _ if kind.info().key.is_some_and(|k| k.required) && key.is_none() => Err("Add a key to see this provider's models.".into()),
        _ => fetch(&http, kind, &base, key.as_deref()).await,
    };
    let (source, mut models, error) = match fetched {
        Ok(m) if !m.is_empty() => ("api", m, None),
        Ok(_) => ("builtin", builtin(kind), Some("The provider listed no models.".to_string())),
        Err(e) => ("builtin", builtin(kind), (!e.is_empty()).then_some(e)),
    };
    models.truncate(MAX_MODELS);
    let list = ModelList { provider: kind, source, models, default_model, fetched_at: Utc::now(), error };
    // A failed fetch is tried again next time.
    if source == "api" || kind.group() == Group::Cli {
        cache().lock().insert(cache_key, (Instant::now(), list.clone()));
    }
    list
}

/// Not a model that chats (embeddings, speech, pictures, moderation…).
fn not_chat(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    ["embed", "whisper", "tts", "dall-e", "moderation", "rerank", "transcribe", "realtime", "image", "audio", "-search", "davinci", "babbage", "guard", "veo", "imagen", "lyria", "sora", "aqa"]
        .iter()
        .any(|w| id.contains(w))
}

fn get(http: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    http.get(url).timeout(Duration::from_secs(8))
}

async fn json(r: reqwest::RequestBuilder, what: &str) -> Result<Value, String> {
    let r = r.send().await.map_err(|e| if e.is_connect() || e.is_timeout() { format!("{what} isn't answering.") } else { format!("Couldn't reach {what}: {e}") })?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{what} answered {}: {}", status.as_u16(), crate::http::api_error(&text)));
    }
    serde_json::from_str(&text).map_err(|e| format!("{what} answered oddly: {e}"))
}

/// The provider's own model list (every wire but the CLIs, Ollama and Bedrock).
pub(crate) async fn fetch(http: &reqwest::Client, kind: ProviderKind, base: &str, key: Option<&str>) -> Result<Vec<ModelInfo>, String> {
    let label = kind.label();
    let bearer = |r: reqwest::RequestBuilder| match key {
        Some(k) => r.bearer_auth(k),
        None => r,
    };
    let mut out: Vec<ModelInfo> = match kind.info().wire {
        Wire::Anthropic => {
            let v = json(get(http, &format!("{base}/v1/models?limit=1000")).header("x-api-key", key.unwrap_or("")).header("anthropic-version", "2023-06-01"), label).await?;
            // lsuite's server may list its models plainly (`{models: ["claude-…"]}`).
            let items = v["data"].as_array().or_else(|| v["models"].as_array()).or_else(|| v.as_array()).cloned().unwrap_or_default();
            items.iter().filter_map(|m| Some(ModelInfo { name: m["display_name"].as_str().or_else(|| m["name"].as_str()).map(str::to_string), tools: Some(true), context: m["max_input_tokens"].as_u64(), ..ModelInfo::new(m["id"].as_str().or_else(|| m.as_str())?) })).collect()
        }
        Wire::Gemini => {
            let v = json(get(http, &format!("{base}/models?pageSize=1000")).header("x-goog-api-key", key.unwrap_or("")), label).await?;
            v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["supportedGenerationMethods"].as_array().is_some_and(|a| a.iter().any(|x| x == "generateContent")))
                .filter_map(|m| {
                    let id = m["name"].as_str()?.trim_start_matches("models/").to_string();
                    Some(ModelInfo { name: m["displayName"].as_str().map(str::to_string), context: m["inputTokenLimit"].as_u64(), ..ModelInfo::new(id) })
                })
                .filter(|m| !not_chat(&m.id) && !m.id.starts_with("gemma"))
                .collect()
        }
        _ if kind == ProviderKind::LmStudio => lmstudio(http, base, key).await?,
        _ => {
            let v = json(bearer(get(http, &format!("{base}/models"))), label).await?;
            let items = v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default();
            items
                .iter()
                .filter(|m| kind != ProviderKind::Together || m["type"].as_str().is_none_or(|t| t == "chat"))
                .filter_map(|m| {
                    let id = m["id"].as_str()?.to_string();
                    let tools = match kind {
                        ProviderKind::OpenRouter => m["supported_parameters"].as_array().map(|p| p.iter().any(|x| x == "tools")),
                        ProviderKind::Mistral => m["capabilities"]["function_calling"].as_bool(),
                        ProviderKind::Fireworks => m["supports_tools"].as_bool().or_else(|| m["supportsTools"].as_bool()),
                        _ => None,
                    };
                    let context = m["context_length"].as_u64().or_else(|| m["context_window"].as_u64()).or_else(|| m["max_context_length"].as_u64());
                    let name = m["name"].as_str().or_else(|| m["display_name"].as_str()).filter(|n| *n != id).map(str::to_string);
                    Some(ModelInfo { name, tools, context, ..ModelInfo::new(id) })
                })
                .filter(|m| !not_chat(&m.id))
                .collect()
        }
    };
    // Newest-looking first is the provider's order; keep it, but tool-capable ones before others.
    out.sort_by_key(|m| m.tools == Some(false));
    let default = kind.default_model();
    if let Some(i) = out.iter().position(|m| m.id == default) {
        let m = out.remove(i);
        out.insert(0, m);
    }
    Ok(out)
}

/// LM Studio's own list (0.4+: which models are loaded and trained for tools), else its
/// OpenAI-compatible one.
async fn lmstudio(http: &reqwest::Client, base: &str, key: Option<&str>) -> Result<Vec<ModelInfo>, String> {
    let root = base.trim_end_matches('/').trim_end_matches("/v1");
    let with = |r: reqwest::RequestBuilder| match key {
        Some(k) => r.bearer_auth(k),
        None => r,
    };
    if let Ok(v) = json(with(get(http, &format!("{root}/api/v1/models"))), "LM Studio").await
        && let Some(models) = v["models"].as_array()
    {
        let mut out: Vec<ModelInfo> = models
            .iter()
            .filter(|m| m["type"].as_str().is_none_or(|t| t == "llm" || t == "vlm"))
            .filter_map(|m| {
                let id = m["key"].as_str().or_else(|| m["id"].as_str())?.to_string();
                Some(ModelInfo {
                    name: m["display_name"].as_str().map(str::to_string),
                    tools: m["capabilities"]["trained_for_tool_use"].as_bool(),
                    context: m["max_context_length"].as_u64(),
                    loaded: Some(m["loaded_instances"].as_array().is_some_and(|l| !l.is_empty())),
                    id,
                })
            })
            .collect();
        out.sort_by_key(|m| (m.loaded != Some(true), m.tools == Some(false)));
        return Ok(out);
    }
    let v = json(with(get(http, &format!("{root}/v1/models"))), "LM Studio").await.map_err(|_| format!("LM Studio isn't running on {}. Open LM Studio and start its server (Developer › Start server).", root.trim_start_matches("http://")))?;
    Ok(v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).filter(|id| !not_chat(id)).map(ModelInfo::new).collect())
}

/// Installed Ollama models, with whether each can call tools (`/api/show` capabilities).
pub(crate) async fn ollama(http: &reqwest::Client, base: &str) -> Result<Vec<ModelInfo>, String> {
    let v = json(get(http, &format!("{base}/api/tags")).timeout(Duration::from_secs(3)), "Ollama")
        .await
        .map_err(|_| format!("Ollama isn't running at {base}. Start it, or choose another provider in Settings › Agent."))?;
    let names: Vec<String> = v["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(str::to_string)).take(40).collect();
    let shows = names.iter().map(|name| async move {
        let r = http.post(format!("{base}/api/show")).json(&serde_json::json!({ "model": name })).timeout(Duration::from_secs(4)).send().await.ok()?;
        r.json::<Value>().await.ok()
    });
    let shows = futures::future::join_all(shows).await;
    Ok(names
        .into_iter()
        .zip(shows)
        .map(|(id, show)| {
            let tools = show.as_ref().and_then(|s| s["capabilities"].as_array()).map(|c| c.iter().any(|x| x == "tools"));
            let context = show.as_ref().and_then(|s| s["model_info"].as_object()).and_then(|o| o.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, v)| v.as_u64()));
            ModelInfo { tools, context, ..ModelInfo::new(id) }
        })
        .collect())
}

/// Bedrock's inference profiles (what newer models are called through), then the on-demand
/// text models.
async fn bedrock_models(http: &reqwest::Client, t: &bedrock::Target) -> Result<Vec<ModelInfo>, String> {
    let host = format!("bedrock.{}.amazonaws.com", t.region);
    let call = |path: &'static str, query: &'static str| {
        let url = format!("https://{host}{path}?{query}");
        let mut r = get(http, &url);
        let control = bedrock::Target { endpoint: format!("https://{host}"), ..t.clone() };
        for (k, v) in control.headers("GET", path, query, b"") {
            r = r.header(k, v);
        }
        json(r, "Amazon Bedrock")
    };
    let profiles = call("/inference-profiles", "maxResults=1000").await?;
    let mut out: Vec<ModelInfo> = profiles["inferenceProfileSummaries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["status"].as_str().is_none_or(|s| s == "ACTIVE"))
        .filter_map(|p| Some(ModelInfo { name: p["inferenceProfileName"].as_str().map(str::to_string), ..ModelInfo::new(p["inferenceProfileId"].as_str()?) }))
        .filter(|m| !not_chat(&m.id))
        .collect();
    if let Ok(models) = call("/foundation-models", "byInferenceType=ON_DEMAND&byOutputModality=TEXT").await {
        out.extend(
            models["modelSummaries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["responseStreamingSupported"].as_bool() != Some(false))
                .filter_map(|m| Some(ModelInfo { name: m["modelName"].as_str().map(str::to_string), ..ModelInfo::new(m["modelId"].as_str()?) }))
                .filter(|m| !not_chat(&m.id)),
        );
    }
    Ok(out)
}
