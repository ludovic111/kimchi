//! App settings, saved as `settings.json` in the config folder.
//!
//! `agent.permissions` is the one place agent and MCP requests are checked
//! against (see [`crate::registry::Perm`]); off means off for both.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub agent: AgentSettings,
    pub updates: UpdateSettings,
    pub appearance: Appearance,
    pub generate: GenerateDefaults,
    pub diagnostics: DiagnosticsSettings,
    pub audio: AudioSettings,
    /// The first-run setup. A settings file written before 0.9 counts as set up already.
    #[serde(default = "OnboardingSettings::set_up_before")]
    pub onboarding: OnboardingSettings,
    pub shortcuts: ShortcutSettings,
    pub plugins: PluginSettings,
}

/// The first-run setup (`app.onboarding`, `app.finishOnboarding`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct OnboardingSettings {
    /// The kimchi version the setup was finished or skipped in; empty: never (it shows at start).
    pub completed: String,
    /// The editor the person said they came from (`kimchi_interop::apps` id), or empty.
    pub coming_from: String,
}

impl OnboardingSettings {
    /// For a settings file from before the setup existed: the person has used kimchi already.
    fn set_up_before() -> Self {
        Self { completed: "0.8".into(), coming_from: String::new() }
    }

    pub fn is_done(&self) -> bool {
        !self.completed.trim().is_empty()
    }
}

/// Keyboard shortcuts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ShortcutSettings {
    /// Which editor's keys kimchi uses ([`KEYMAPS`]): kimchi's own, or another editor's layout for
    /// the commands they share.
    pub keymap: String,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self { keymap: "kimchi".into() }
    }
}

/// Keyboard layouts kimchi can take from other editors (`app.keymaps` describes them).
pub const KEYMAPS: &[&str] = &["kimchi", "premiere", "finalcut", "resolve", "avid", "capcut", "kdenlive", "shotcut", "vegas", "imovie"];

/// Video plugins (kimchi's own, frei0r, OpenFX). Audio plugin folders are in [`AudioSettings`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginSettings {
    /// More folders to look for video plugins in, besides the standard ones.
    pub video_folders: Vec<String>,
}

/// What can run the built-in agent (`settings.agent.provider`): the person's coding CLIs, model
/// APIs, and local servers. `kimchi_agent::ProviderKind` has one variant per id.
pub const AGENT_PROVIDERS: &[&str] = &[
    "zenith",
    "claude-code",
    "codex",
    "gemini-cli",
    "anthropic",
    "openai",
    "gemini",
    "openrouter",
    "groq",
    "mistral",
    "deepseek",
    "xai",
    "together",
    "fireworks",
    "cerebras",
    "azure-openai",
    "bedrock",
    "ollama",
    "lmstudio",
    "openai-compatible",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentSettings {
    /// The Agent panel is offered (off hides it; MCP and the CLI still work).
    pub enabled: bool,
    pub permissions: Permissions,
    /// Which model runs the built-in agent, one of [`AGENT_PROVIDERS`].
    pub provider: String,
    /// Model id for the API and local providers (empty: the provider's default).
    pub model: String,
    /// Base URL for `ollama` (and OpenAI-compatible servers).
    pub base_url: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self { enabled: true, permissions: Permissions::default(), provider: "claude-code".into(), model: String::new(), base_url: String::new() }
    }
}

/// What an agent (the built-in one, MCP clients, `kimchi-cli --agent`) may do
/// besides editing the project, which is always allowed and always undoable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Permissions {
    /// Master switch: off refuses every agent and MCP request.
    pub enabled: bool,
    /// Import media, export, write files, save a copy of the project.
    pub files: bool,
    /// Create, open, close, duplicate or delete projects.
    pub projects: bool,
    /// Generate images and video (spends the provider's credits).
    pub generate: bool,
    /// Change settings other than these permissions and API keys.
    pub settings: bool,
    /// Quit the app, install an update.
    pub app_control: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self { enabled: true, files: true, projects: true, generate: true, settings: false, app_control: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateSettings {
    /// Check GitHub Releases when the app starts, and every few hours while it runs.
    /// `KIMCHI_NO_UPDATE=1` also turns it off.
    pub check_on_start: bool,
    /// Download and install a found update without asking; it is used from the next start.
    pub auto_install: bool,
    /// Show what's new once after kimchi updates.
    pub show_whats_new: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check_on_start: true, auto_install: false, show_whats_new: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DiagnosticsSettings {
    /// How much kimchi writes to its log: `info`, `debug` or `trace`. `RUST_LOG` wins when set.
    pub log_level: String,
}

impl Default for DiagnosticsSettings {
    fn default() -> Self {
        Self { log_level: "debug".into() }
    }
}

/// Settings › Audio: devices, loudness, plugins and how the timeline treats sound.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioSettings {
    /// The speakers the preview plays through, by name (empty: the system's default).
    pub output_device: String,
    /// The microphone voice-overs are recorded from, by name (empty: the system's default).
    pub input_device: String,
    /// Loudness clips are normalised to (`audio.normalize`), in LUFS.
    pub default_loudness: f64,
    /// More folders to look for plugins in, besides the standard ones.
    pub plugin_folders: Vec<String>,
    /// Hear the sound while the playhead is dragged.
    pub scrub: bool,
    /// Dragged clips and the playhead also stick to the beats of music on the timeline.
    pub snap_to_beats: bool,
    /// Seconds counted in before a voice-over take starts recording.
    pub count_in: f64,
    /// Render ryolune songs again when the window comes back to the front and they were saved.
    pub refresh_songs: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            output_device: String::new(),
            input_device: String::new(),
            default_loudness: -16.0,
            plugin_folders: vec![],
            scrub: true,
            snap_to_beats: false,
            count_in: 3.0,
            refresh_songs: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Appearance {
    /// `system`, `dark` or `light`.
    pub mode: String,
    /// Glass chrome; off gives opaque surfaces (also follows the OS "reduce transparency").
    pub transparency: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { mode: "system".into(), transparency: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct GenerateDefaults {
    /// Generative AI is offered in the window: the Generate tab and the AI actions on clips and
    /// frames. Off hides them (the person chose not to use it); `generate.*` still works.
    pub enabled: bool,
    /// `provider::model` used for images when a command names no model.
    pub image_model: String,
    /// `provider::model` used for video when a command names no model.
    pub video_model: String,
    pub audio_model: String,
    pub speech_model: String,
}

impl Default for GenerateDefaults {
    fn default() -> Self {
        Self { enabled: true, image_model: String::new(), video_model: String::new(), audio_model: String::new(), speech_model: String::new() }
    }
}

impl Settings {
    /// Reads `settings.json`. A file that can't be read as settings is kept as
    /// `settings.json.bad` (so the next save doesn't lose it) and the defaults are used.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        let Ok(bytes) = std::fs::read(&path) else { return Self::default() };
        match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("{} isn't valid ({e}); kept as settings.json.bad, using the defaults", path.display());
                let _ = std::fs::rename(&path, dir.join("settings.json.bad"));
                Self::default()
            }
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, dir.join("settings.json"))
    }

    /// Whether the update check should run (setting and `KIMCHI_NO_UPDATE`).
    pub fn update_check_enabled(&self) -> bool {
        self.updates.check_on_start && !std::env::var("KIMCHI_NO_UPDATE").is_ok_and(|v| !v.is_empty() && v != "0")
    }

    /// Reads a setting by dotted key (`updates.checkOnStart`).
    pub fn get(&self, key: &str) -> Option<Value> {
        let mut v = serde_json::to_value(self).ok()?;
        for part in key.split('.').filter(|p| !p.is_empty()) {
            v = v.get(part)?.clone();
        }
        Some(v)
    }

    /// Sets a setting by dotted key, keeping the value's type.
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        let mut root = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
        let (last, path) = parts.split_last().ok_or("empty setting key")?;
        let mut node = &mut root;
        for p in path {
            node = node.get_mut(*p).ok_or_else(|| unknown(key))?;
        }
        let slot = node.get_mut(*last).ok_or_else(|| unknown(key))?;
        let same_type = matches!(
            (&*slot, &value),
            (Value::Bool(_), Value::Bool(_)) | (Value::String(_), Value::String(_)) | (Value::Number(_), Value::Number(_))
        ) || matches!((&*slot, &value), (Value::Array(_), Value::Array(items)) if items.iter().all(Value::is_string));
        if !same_type {
            return Err(format!("`{key}` expects {}, got {value}", type_name(slot)));
        }
        if let Some(allowed) = choices(key)
            && !value.as_str().is_some_and(|v| allowed.contains(&v))
        {
            return Err(format!("`{key}` is one of {}, not {value}.", allowed.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(", ")));
        }
        if let (Some((lo, hi)), Some(v)) = (range(key), value.as_f64())
            && !(lo..=hi).contains(&v)
        {
            return Err(format!("`{key}` goes from {lo} to {hi}, not {v}."));
        }
        *slot = value;
        *self = serde_json::from_value(root).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// The values a text setting takes, for those with a fixed set.
pub fn choices(key: &str) -> Option<&'static [&'static str]> {
    Some(match key {
        "appearance.mode" => &["system", "dark", "light"],
        "agent.provider" => AGENT_PROVIDERS,
        "shortcuts.keymap" => KEYMAPS,
        "diagnostics.logLevel" => crate::diagnostics::LEVELS,
        _ => return None,
    })
}

/// The range of a number setting, for those with one.
pub fn range(key: &str) -> Option<(f64, f64)> {
    Some(match key {
        "audio.defaultLoudness" => (-40.0, -5.0),
        "audio.countIn" => (0.0, 10.0),
        _ => return None,
    })
}

fn unknown(key: &str) -> String {
    format!("Unknown setting `{key}`. app.settings lists them.")
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "true or false",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list of strings",
        _ => "an object",
    }
}
