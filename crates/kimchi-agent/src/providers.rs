//! What can run the agent, as data: for each provider its group, label, wire protocol and
//! quirks, default model and address, where its key lives and where to get one, and a short
//! list of models for when its own list can't be fetched.
//!
//! The ids are `kimchi_control::settings::AGENT_PROVIDERS`; [`ProviderKind`] has one variant
//! per id. Keys share the keychain with generation where the service is the same (`openai`,
//! `google`, `openrouter`, `xai`, `together`), so one key serves both.

use serde::Serialize;

use crate::ProviderKind;

/// How a provider is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Group {
    /// lsuite AI: sign in to the lsuite account and it works.
    Lsuite,
    /// A coding CLI installed on this computer, with `kimchi-mcp --live` attached.
    Cli,
    /// A model API, with a key.
    Api,
    /// A model server on this computer or the local network.
    Local,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Lsuite, Group::Cli, Group::Api, Group::Local];

    pub fn label(self) -> &'static str {
        match self {
            Group::Lsuite => "No setup",
            Group::Cli => "On this computer",
            Group::Api => "Model APIs",
            Group::Local => "Local servers",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Group::Lsuite => "lsuite",
            Group::Cli => "cli",
            Group::Api => "api",
            Group::Local => "local",
        }
    }
}

/// The request format a provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    Cli,
    Anthropic,
    /// Chat Completions, with the provider's quirks.
    Chat(Quirks),
    Gemini,
    Bedrock,
    Ollama,
}

/// Where Chat Completions servers differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quirks {
    /// The parameter that caps the reply, and the cap, when the default is too short or the
    /// server wants it named one way.
    pub max_tokens: Option<(&'static str, u32)>,
    /// Ask for token counts in the stream (`stream_options.include_usage`).
    pub usage: bool,
    /// At most this many tools in one request.
    pub tool_limit: Option<usize>,
    /// Only a short list of tools (small local models).
    pub compact: bool,
    /// Tool call ids must be 9 letters and digits (Mistral).
    pub short_ids: bool,
    /// The reasoning text streamed with a reply that calls tools goes back with it
    /// (`reasoning_content`: DeepSeek's thinking mode requires it within a tool loop).
    pub reasoning_back: bool,
    /// OpenRouter's `reasoning_details` go back on the assistant message (Gemini and
    /// Anthropic models behind it need their signatures).
    pub reasoning_details: bool,
    /// The `Authorization` header is `api-key: <key>` (Azure).
    pub api_key_header: bool,
}

impl Quirks {
    pub const STANDARD: Quirks =
        Quirks { max_tokens: None, usage: true, tool_limit: None, compact: false, short_ids: false, reasoning_back: false, reasoning_details: false, api_key_header: false };
}

/// Where a provider's API key comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySpec {
    /// Keychain id (shared with generation where it is the same service).
    pub id: &'static str,
    /// Environment variables read when nothing is saved, in order.
    pub env: &'static [&'static str],
    /// A key is needed (else optional: a local server may ask for one).
    pub required: bool,
    /// Where people make one.
    pub url: Option<&'static str>,
    /// What one looks like, for the field's placeholder.
    pub hint: &'static str,
}

/// Everything static about a provider.
#[derive(Clone, Copy, Debug)]
pub struct Info {
    pub kind: ProviderKind,
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    /// One plain line on what it is.
    pub tagline: &'static str,
    pub wire: Wire,
    /// Model used when none is chosen (empty: the CLI's or server's own).
    pub default_model: &'static str,
    /// Empty for the CLIs, and for Azure and custom servers (the person gives it).
    pub default_base_url: &'static str,
    /// The address is the person's to give (Azure resource, a custom server).
    pub needs_base_url: bool,
    /// What goes in the address field.
    pub base_url_hint: &'static str,
    pub key: Option<KeySpec>,
    /// Models shown when the list can't be fetched, the default first.
    pub models: &'static [&'static str],
    pub website: &'static str,
}

const fn key(id: &'static str, env: &'static [&'static str], url: &'static str, hint: &'static str) -> Option<KeySpec> {
    Some(KeySpec { id, env, required: true, url: Some(url), hint })
}

/// Chat Completions with standard behaviour and a tool cap.
const fn chat(tool_limit: Option<usize>) -> Wire {
    Wire::Chat(Quirks { tool_limit, ..Quirks::STANDARD })
}

pub const ALL: &[Info] = &[
    // ---- lsuite AI ----
    Info {
        kind: ProviderKind::Lsuite,
        id: "lsuite",
        label: "lsuite AI",
        group: Group::Lsuite,
        tagline: "No setup. Sign in and your agent works.",
        // Anthropic's Messages API, served by the lsuite server at `<server>/api/ai` with the
        // account's token as the key (`Api::prepare`).
        wire: Wire::Anthropic,
        default_model: "claude-sonnet-5-5",
        default_base_url: "",
        needs_base_url: false,
        base_url_hint: "",
        key: None,
        models: &["claude-sonnet-5-5", "claude-haiku-4-5", "claude-opus-5-5"],
        website: "https://lsuite.xyz",
    },
    // ---- on this computer ----
    Info {
        kind: ProviderKind::Zenith,
        id: "zenith",
        label: "zenith · lsuite",
        group: Group::Cli,
        tagline: "Your lsuite agents and their permissions, through zenith.",
        wire: Wire::Cli,
        default_model: "",
        default_base_url: "",
        needs_base_url: false,
        base_url_hint: "",
        key: None,
        models: &[],
        website: "https://lsuite.xyz/zenith",
    },
    Info {
        kind: ProviderKind::ClaudeCode,
        id: "claude-code",
        label: "Claude Code",
        group: Group::Cli,
        tagline: "Your Claude subscription, through the Claude Code app.",
        wire: Wire::Cli,
        default_model: "",
        default_base_url: "",
        needs_base_url: false,
        base_url_hint: "",
        key: None,
        models: &["opus", "sonnet", "haiku"],
        website: "https://claude.com/claude-code",
    },
    Info {
        kind: ProviderKind::Codex,
        id: "codex",
        label: "Codex",
        group: Group::Cli,
        tagline: "Your ChatGPT plan, through OpenAI's Codex CLI.",
        wire: Wire::Cli,
        default_model: "",
        default_base_url: "",
        needs_base_url: false,
        base_url_hint: "",
        key: None,
        models: &[],
        website: "https://developers.openai.com/codex/cli",
    },
    Info {
        kind: ProviderKind::GeminiCli,
        id: "gemini-cli",
        label: "Gemini CLI",
        group: Group::Cli,
        tagline: "Your Google account, through Google's Gemini CLI.",
        wire: Wire::Cli,
        default_model: "",
        default_base_url: "",
        needs_base_url: false,
        base_url_hint: "",
        key: None,
        models: &["pro", "flash", "flash-lite"],
        website: "https://geminicli.com",
    },
    // ---- model APIs ----
    Info {
        kind: ProviderKind::Anthropic,
        id: "anthropic",
        label: "Anthropic API",
        group: Group::Api,
        tagline: "Claude models, billed per use by Anthropic.",
        wire: Wire::Anthropic,
        default_model: "claude-sonnet-5-5",
        default_base_url: "https://api.anthropic.com",
        needs_base_url: false,
        base_url_hint: "",
        key: key("anthropic", &["ANTHROPIC_API_KEY"], "https://platform.claude.com/settings/keys", "sk-ant-…"),
        models: &["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-4-5", "claude-fable-5-1"],
        website: "https://www.anthropic.com/api",
    },
    Info {
        kind: ProviderKind::OpenAi,
        id: "openai",
        label: "OpenAI API",
        group: Group::Api,
        tagline: "GPT models, billed per use by OpenAI.",
        // OpenAI takes at most 128 functions in one request.
        wire: chat(Some(128)),
        default_model: "gpt-5",
        default_base_url: "https://api.openai.com/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("openai", &["OPENAI_API_KEY"], "https://platform.openai.com/api-keys", "sk-…"),
        models: &["gpt-5"],
        website: "https://platform.openai.com",
    },
    Info {
        kind: ProviderKind::Gemini,
        id: "gemini",
        label: "Google Gemini API",
        group: Group::Api,
        tagline: "Gemini models with a Google AI Studio key; there is a free tier.",
        wire: Wire::Gemini,
        default_model: "gemini-3.1-pro-preview",
        default_base_url: "https://generativelanguage.googleapis.com/v1beta",
        needs_base_url: false,
        base_url_hint: "",
        key: key("google", &["GEMINI_API_KEY", "GOOGLE_API_KEY"], "https://aistudio.google.com/apikey", "AIza…"),
        models: &["gemini-3.1-pro-preview"],
        website: "https://ai.google.dev",
    },
    Info {
        kind: ProviderKind::OpenRouter,
        id: "openrouter",
        label: "OpenRouter",
        group: Group::Api,
        tagline: "Hundreds of models from every lab with one key.",
        wire: Wire::Chat(Quirks { reasoning_details: true, ..Quirks::STANDARD }),
        default_model: "anthropic/claude-sonnet-5.5",
        default_base_url: "https://openrouter.ai/api/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("openrouter", &["OPENROUTER_API_KEY"], "https://openrouter.ai/settings/keys", "sk-or-…"),
        models: &["anthropic/claude-sonnet-5.5"],
        website: "https://openrouter.ai",
    },
    Info {
        kind: ProviderKind::Groq,
        id: "groq",
        label: "Groq",
        group: Group::Api,
        tagline: "Open models answering very fast.",
        wire: chat(Some(128)),
        default_model: "openai/gpt-oss-120b",
        default_base_url: "https://api.groq.com/openai/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("groq", &["GROQ_API_KEY"], "https://console.groq.com/keys", "gsk_…"),
        models: &["openai/gpt-oss-120b"],
        website: "https://groq.com",
    },
    Info {
        kind: ProviderKind::Mistral,
        id: "mistral",
        label: "Mistral",
        group: Group::Api,
        tagline: "Mistral's own models, from Europe.",
        wire: Wire::Chat(Quirks { short_ids: true, ..Quirks::STANDARD }),
        default_model: "mistral-large-latest",
        default_base_url: "https://api.mistral.ai/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("mistral", &["MISTRAL_API_KEY"], "https://console.mistral.ai/api-keys", ""),
        models: &["mistral-large-latest"],
        website: "https://mistral.ai",
    },
    Info {
        kind: ProviderKind::DeepSeek,
        id: "deepseek",
        label: "DeepSeek",
        group: Group::Api,
        tagline: "DeepSeek's models at a low price.",
        wire: Wire::Chat(Quirks { reasoning_back: true, max_tokens: Some(("max_tokens", 8192)), ..Quirks::STANDARD }),
        default_model: "deepseek-chat",
        default_base_url: "https://api.deepseek.com",
        needs_base_url: false,
        base_url_hint: "",
        key: key("deepseek", &["DEEPSEEK_API_KEY"], "https://platform.deepseek.com/api_keys", "sk-…"),
        models: &["deepseek-chat", "deepseek-reasoner"],
        website: "https://platform.deepseek.com",
    },
    Info {
        kind: ProviderKind::Xai,
        id: "xai",
        label: "xAI",
        group: Group::Api,
        tagline: "Grok models.",
        wire: chat(Some(128)),
        default_model: "grok-4",
        default_base_url: "https://api.x.ai/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("xai", &["XAI_API_KEY"], "https://console.x.ai", "xai-…"),
        models: &["grok-4"],
        website: "https://x.ai/api",
    },
    Info {
        kind: ProviderKind::Together,
        id: "together",
        label: "Together AI",
        group: Group::Api,
        tagline: "Open models (Qwen, DeepSeek, Kimi, Llama) in the cloud.",
        wire: chat(Some(128)),
        default_model: "Qwen/Qwen3-235B-A22B-Instruct-2507-tput",
        default_base_url: "https://api.together.xyz/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("together", &["TOGETHER_API_KEY"], "https://api.together.ai/settings/api-keys", ""),
        models: &["Qwen/Qwen3-235B-A22B-Instruct-2507-tput"],
        website: "https://www.together.ai",
    },
    Info {
        kind: ProviderKind::Fireworks,
        id: "fireworks",
        label: "Fireworks AI",
        group: Group::Api,
        tagline: "Open models served fast.",
        wire: chat(Some(128)),
        default_model: "accounts/fireworks/models/kimi-k2-instruct-0905",
        default_base_url: "https://api.fireworks.ai/inference/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("fireworks", &["FIREWORKS_API_KEY"], "https://app.fireworks.ai/settings/users/api-keys", "fw_…"),
        models: &["accounts/fireworks/models/kimi-k2-instruct-0905"],
        website: "https://fireworks.ai",
    },
    Info {
        kind: ProviderKind::Cerebras,
        id: "cerebras",
        label: "Cerebras",
        group: Group::Api,
        tagline: "Open models at thousands of words a second.",
        wire: Wire::Chat(Quirks { tool_limit: Some(128), max_tokens: None, ..Quirks::STANDARD }),
        default_model: "gpt-oss-120b",
        default_base_url: "https://api.cerebras.ai/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: key("cerebras", &["CEREBRAS_API_KEY"], "https://cloud.cerebras.ai", "csk-…"),
        models: &["gpt-oss-120b"],
        website: "https://www.cerebras.ai",
    },
    Info {
        kind: ProviderKind::AzureOpenAi,
        id: "azure-openai",
        label: "Azure OpenAI",
        group: Group::Api,
        tagline: "OpenAI models deployed in your own Azure resource.",
        wire: Wire::Chat(Quirks { api_key_header: true, tool_limit: Some(128), ..Quirks::STANDARD }),
        default_model: "",
        default_base_url: "",
        needs_base_url: true,
        base_url_hint: "Your resource: my-resource, or https://my-resource.openai.azure.com",
        key: key("azure-openai", &["AZURE_OPENAI_API_KEY"], "https://portal.azure.com/#view/Microsoft_Azure_ProjectOxford/CognitiveServicesHub/~/OpenAI", ""),
        models: &[],
        website: "https://azure.microsoft.com/products/ai-services/openai-service",
    },
    Info {
        kind: ProviderKind::Bedrock,
        id: "bedrock",
        label: "Amazon Bedrock",
        group: Group::Api,
        tagline: "Claude, Nova, Llama and more in your AWS account.",
        wire: Wire::Bedrock,
        default_model: "us.anthropic.claude-sonnet-5-5",
        default_base_url: "",
        needs_base_url: false,
        base_url_hint: "Region, e.g. us-east-1 (default: your AWS configuration's)",
        key: Some(KeySpec {
            id: "bedrock",
            env: &["AWS_BEARER_TOKEN_BEDROCK", "AWS_ACCESS_KEY_ID"],
            required: true,
            url: Some("https://console.aws.amazon.com/bedrock/home#/api-keys"),
            hint: "Bedrock API key (ABSK…)",
        }),
        models: &["us.anthropic.claude-sonnet-5-5"],
        website: "https://aws.amazon.com/bedrock",
    },
    // ---- local servers ----
    Info {
        kind: ProviderKind::Ollama,
        id: "ollama",
        label: "Ollama",
        group: Group::Local,
        tagline: "Models on this computer. Nothing leaves it.",
        wire: Wire::Ollama,
        default_model: "",
        default_base_url: "http://127.0.0.1:11434",
        needs_base_url: false,
        base_url_hint: "",
        key: None,
        models: &[],
        website: "https://ollama.com",
    },
    Info {
        kind: ProviderKind::LmStudio,
        id: "lmstudio",
        label: "LM Studio",
        group: Group::Local,
        tagline: "Models on this computer, from the LM Studio app.",
        wire: Wire::Chat(Quirks { usage: false, compact: true, ..Quirks::STANDARD }),
        default_model: "",
        default_base_url: "http://127.0.0.1:1234/v1",
        needs_base_url: false,
        base_url_hint: "",
        key: Some(KeySpec { id: "lmstudio", env: &["LM_API_TOKEN"], required: false, url: None, hint: "Only if the server asks for one" }),
        models: &[],
        website: "https://lmstudio.ai",
    },
    Info {
        kind: ProviderKind::OpenAiCompatible,
        id: "openai-compatible",
        label: "OpenAI-compatible server",
        group: Group::Local,
        tagline: "Any server that speaks OpenAI's Chat Completions: vLLM, llama.cpp, LiteLLM, a proxy…",
        wire: Wire::Chat(Quirks { usage: false, ..Quirks::STANDARD }),
        default_model: "",
        default_base_url: "",
        needs_base_url: true,
        base_url_hint: "The server's address, e.g. http://127.0.0.1:8000/v1",
        key: Some(KeySpec { id: "openai-compatible", env: &[], required: false, url: None, hint: "Only if the server asks for one" }),
        models: &[],
        website: "",
    },
];

/// The static facts about `kind`.
pub fn info(kind: ProviderKind) -> &'static Info {
    ALL.iter().find(|i| i.kind == kind).expect("every provider is in providers::ALL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_matches_the_settings_ids() {
        let ids: Vec<&str> = ALL.iter().map(|i| i.id).collect();
        assert_eq!(ids, kimchi_control::settings::AGENT_PROVIDERS);
        for i in ALL {
            assert_eq!(i.kind.id(), i.id);
            assert_eq!(ProviderKind::parse(i.id), Some(i.kind));
            assert_eq!(i.group == Group::Cli, i.wire == Wire::Cli, "{}", i.id);
            if let Some(k) = i.key.filter(|k| k.required) {
                assert!(k.url.is_some(), "{}: a key is needed, so say where to get one", i.id);
            }
            if !i.default_model.is_empty() && !i.models.is_empty() {
                assert_eq!(i.models[0], i.default_model, "{}: the default model comes first", i.id);
            }
        }
    }
}
