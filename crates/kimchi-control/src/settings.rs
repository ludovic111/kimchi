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
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentSettings {
    pub permissions: Permissions,
    /// Which model runs the built-in agent: `claude-code`, `codex`, `anthropic`, `openai` or `ollama`.
    pub provider: String,
    /// Model id for the API and local providers (empty: the provider's default).
    pub model: String,
    /// Base URL for `ollama` (and OpenAI-compatible servers).
    pub base_url: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self { permissions: Permissions::default(), provider: "claude-code".into(), model: String::new(), base_url: String::new() }
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
    /// Check GitHub Releases when the app starts. `KIMCHI_NO_UPDATE=1` also turns it off.
    pub check_on_start: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check_on_start: true }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct GenerateDefaults {
    /// `provider::model` used for images when a command names no model.
    pub image_model: String,
    /// `provider::model` used for video when a command names no model.
    pub video_model: String,
}

impl Settings {
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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
        );
        if !same_type {
            return Err(format!("`{key}` expects {}, got {value}", type_name(slot)));
        }
        *slot = value;
        *self = serde_json::from_value(root).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn unknown(key: &str) -> String {
    format!("Unknown setting `{key}`. app.settings lists them.")
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "true or false",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        _ => "an object",
    }
}
