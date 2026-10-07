//! The logos of the services and apps kimchi works with (AI providers, MCP
//! clients, the other lsuite apps), shown next to their names. The files are
//! the companies' own (`assets/logos/SOURCES.md` says where each came from);
//! they are full colour, so they are drawn with `img()`, not tinted like icons.

use gpui::{App, IntoElement, ObjectFit, Pixels, RenderOnce, Styled, Window, div, img, prelude::*};

use crate::theme::ActiveTheme;
use crate::ui::icon;

/// A logo file in `assets/logos/`, and whether it has a `-dark` twin for the
/// dark theme (black marks such as OpenAI's turn white there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogoFile {
    pub file: &'static str,
    pub dark: bool,
}

const fn one(file: &'static str) -> Option<LogoFile> {
    Some(LogoFile { file, dark: false })
}

const fn themed(file: &'static str) -> Option<LogoFile> {
    Some(LogoFile { file, dark: true })
}

/// Every id the window can name, with its logo. `None` marks a service with no
/// logo of its own (a generic server); those show the generic icon.
///
/// Ids: agent providers (`kimchi_agent::ProviderKind::id`), generation providers
/// (`kimchi_gen::providers`), MCP clients and the lsuite apps.
pub const LOGOS: &[(&str, Option<LogoFile>)] = &[
    // lsuite AI (the agent's lsuite provider): the lsuite mark.
    ("lsuite", one("lsuite")),
    // Claude: the agent's Claude Code and Anthropic API, and Claude Desktop over MCP.
    ("claude", one("claude")),
    ("claude-code", one("claude")),
    ("claude-desktop", one("claude")),
    ("anthropic", one("claude")),
    // OpenAI: the ChatGPT mark for the API, Codex and ChatGPT.
    ("openai", themed("openai")),
    ("codex", themed("openai")),
    ("chatgpt", themed("openai")),
    ("ollama", themed("ollama")),
    // Google's Gemini CLI: the Gemini mark.
    ("gemini-cli", one("google")),
    // Agent providers without a logo bundled yet: the generic icon.
    ("groq", None),
    ("mistral", None),
    ("deepseek", None),
    ("fireworks", None),
    ("cerebras", None),
    ("azure-openai", None),
    ("bedrock", None),
    ("lmstudio", None),
    ("openai-compatible", None),
    // Generation providers.
    ("openrouter", one("openrouter")),
    ("fal", one("fal")),
    ("replicate", one("replicate")),
    ("google", one("google")),
    ("gemini", one("google")),
    ("xai", one("xai")),
    ("runway", one("runway")),
    ("luma", themed("luma")),
    ("bfl", one("bfl")),
    ("stability", one("stability")),
    ("elevenlabs", one("elevenlabs")),
    ("together", one("together")),
    ("comfyui", one("comfyui")),
    // Stable Diffusion WebUI (AUTOMATIC1111) has no logo; nor does a generic OpenAI-compatible server.
    ("a1111", None),
    ("openai_compat", None),
    // MCP clients.
    ("cursor", one("cursor")),
    ("vscode", one("vscode")),
    // Plugin formats kimchi loads (Plugins). LUTs and Audio Units have no logo of their own.
    ("frei0r", themed("frei0r")),
    ("clap", themed("clap")),
    ("vst3", one("vst3")),
    ("lut", None),
    ("au", None),
    // Editors kimchi opens projects, looks or keys from (`kimchi_interop::apps` ids), and
    // OpenTimelineIO. Apps it has no real support for show their initials instead.
    ("premiere", one("premiere")),
    ("aftereffects", one("aftereffects")),
    ("lightroom", one("lightroom")),
    ("finalcut", one("finalcut")),
    ("imovie", one("imovie")),
    ("resolve", one("resolve")),
    ("capcut", one("capcut")),
    ("avid", one("avid")),
    ("blender", one("blender")),
    ("otio", one("otio")),
    ("kdenlive", one("kdenlive")),
    ("vegas", themed("vegas")),
    ("nuke", themed("nuke")),
    ("shotcut", one("shotcut")),
    // lsuite.
    ("ryolune", one("ryolune")),
    ("zenith", one("zenith")),
];

/// The logo for an id (any case), if it has one.
pub fn logo_file(id: &str) -> Option<LogoFile> {
    let id = id.trim().to_ascii_lowercase();
    LOGOS.iter().find(|(k, _)| *k == id).and_then(|(_, f)| *f)
}

/// The asset path for a theme: `logos/<file>.png`, or `logos/<file>-dark.png`.
pub fn logo_path(f: LogoFile, dark: bool) -> String {
    if dark && f.dark { format!("logos/{}-dark.png", f.file) } else { format!("logos/{}.png", f.file) }
}

/// A service's logo, `size` square, picked for the theme; ids without a logo
/// show the generic `box` icon in the text colour.
pub fn logo(id: &str, size: Pixels) -> Logo {
    Logo { file: logo_file(id), size }
}

#[derive(IntoElement)]
pub struct Logo {
    file: Option<LogoFile>,
    size: Pixels,
}

impl RenderOnce for Logo {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = self.size;
        match self.file {
            // A small rounding so square tiles sit like app icons.
            Some(f) => div().flex_none().size(size).child(
                img(logo_path(f, cx.theme().is_dark())).size(size).object_fit(ObjectFit::Contain).rounded(size * 0.22),
            ),
            None => div().flex_none().size(size).flex().items_center().justify_center().child(icon("box").size(size * 0.9)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Assets;

    /// Every id the app names: agent providers, generation providers, MCP clients, lsuite apps.
    fn known_ids() -> Vec<String> {
        let mut ids: Vec<String> = kimchi_agent::ProviderKind::ALL.iter().map(|k| k.id().to_string()).collect();
        ids.extend(kimchi_gen::providers::all().iter().map(|p| p.info().id));
        ids.extend(["claude-desktop", "cursor", "vscode", "ryolune", "zenith"].map(String::from));
        ids
    }

    #[test]
    fn every_known_id_has_a_logo_entry_and_its_files_are_bundled() {
        for id in known_ids() {
            let entry = LOGOS.iter().find(|(k, _)| *k == id).unwrap_or_else(|| panic!("no logo entry for `{id}` in ui::logos::LOGOS"));
            if let Some(f) = entry.1 {
                for dark in [false, true] {
                    let path = logo_path(f, dark);
                    let data = Assets::get(&path).unwrap_or_else(|| panic!("`{id}`: {path} isn't bundled"));
                    let img = image::load_from_memory(&data.data).unwrap_or_else(|e| panic!("`{id}`: {path} doesn't decode: {e}"));
                    assert!(img.width() >= 32 && img.height() >= 32, "{path} is too small");
                }
            }
        }
    }

    #[test]
    fn only_generic_services_go_without_a_logo() {
        let without: Vec<&str> = LOGOS.iter().filter(|(_, f)| f.is_none()).map(|(k, _)| *k).collect();
        assert_eq!(without, ["groq", "mistral", "deepseek", "fireworks", "cerebras", "azure-openai", "bedrock", "lmstudio", "openai-compatible", "a1111", "openai_compat", "lut", "au"]);
        // The owner's mapping: Claude everywhere for Claude, the ChatGPT mark for OpenAI and Codex.
        assert_eq!(logo_file("Claude-Code").map(|f| f.file), Some("claude"));
        assert_eq!(logo_file("anthropic").map(|f| f.file), Some("claude"));
        assert_eq!(logo_file("codex").map(|f| f.file), Some("openai"));
        assert_eq!(logo_file("nope"), None);
        assert_eq!(logo_path(logo_file("openai").unwrap(), true), "logos/openai-dark.png");
        assert_eq!(logo_path(logo_file("claude").unwrap(), true), "logos/claude.png");
    }

    /// An editor shows its own logo exactly when kimchi really works with it: opens its projects,
    /// takes its looks or models, or uses its keys. The others keep their initials.
    #[test]
    fn editors_have_logos_only_where_kimchi_really_supports_them() {
        for app in kimchi_interop::apps::APPS {
            let supported = !app.opens.is_empty() || app.keymap.is_some() || matches!(app.id, "aftereffects" | "lightroom" | "blender");
            assert_eq!(logo_file(app.id).is_some(), supported, "{}", app.id);
        }
        assert!(logo_file("otio").is_some());
    }

    #[test]
    fn every_bundled_logo_is_listed_and_sourced() {
        let sources = include_str!("../../assets/logos/SOURCES.md");
        for path in Assets::iter().filter(|p| p.starts_with("logos/") && p.ends_with(".png")) {
            let file = path.trim_start_matches("logos/");
            let base = file.trim_end_matches(".png").trim_end_matches("-dark");
            assert!(LOGOS.iter().any(|(_, f)| f.is_some_and(|f| f.file == base)), "{path} is bundled but no id uses it");
            assert!(sources.contains(&format!("`{file}`")), "{file} isn't in assets/logos/SOURCES.md");
        }
    }
}
