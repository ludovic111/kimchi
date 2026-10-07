//! The agent harness (lsuite `HARNESS.md`): what an agent working in kimchi knows and sees.
//!
//! * [`brief`]: the expert brief, one source for the built-in agent's system prompt and
//!   `kimchi-mcp`'s instructions (`brief.md`, with the skills' index appended).
//! * [`skills`] / [`skill`]: playbooks for the trade's jobs (`skills/*.md`), loaded with
//!   `harness.skill`; over MCP each is also a prompt and a resource (`kimchi://skills/<name>`).
//! * [`context`]: the live context, a compact summary of what the person sees, sent before every
//!   model step.
//! * `harness.look` (`commands/harness.rs`): the best picture of the current work, with its numbers.

pub mod context;

pub use context::{Glance, glance};

/// The brief's body; [`brief`] adds the skills' index.
const BRIEF: &str = include_str!("brief.md");

/// Every skill: its name (the file's stem) and markdown. The first line is `# Title`, the second
/// `When: …`.
const SKILLS: &[(&str, &str)] = &[
    ("rough-cut", include_str!("skills/rough-cut.md")),
    ("trailer", include_str!("skills/trailer.md")),
    ("social-vertical", include_str!("skills/social-vertical.md")),
    ("titles-and-captions", include_str!("skills/titles-and-captions.md")),
    ("color-grade", include_str!("skills/color-grade.md")),
    ("audio-mix", include_str!("skills/audio-mix.md")),
    ("motion-graphics", include_str!("skills/motion-graphics.md")),
    ("3d-product-shot", include_str!("skills/3d-product-shot.md")),
    ("b-roll", include_str!("skills/b-roll.md")),
    ("scoring", include_str!("skills/scoring.md")),
    ("export", include_str!("skills/export.md")),
    ("write-a-plugin", include_str!("skills/write-a-plugin.md")),
    ("review-the-cut", include_str!("skills/review-the-cut.md")),
];

/// One skill as `harness.skills` lists it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SkillInfo {
    pub name: &'static str,
    pub title: &'static str,
    /// When to use it.
    pub when: &'static str,
}

/// Every skill, in a stable order.
pub fn skills() -> Vec<SkillInfo> {
    SKILLS
        .iter()
        .map(|(name, text)| {
            let mut lines = text.lines();
            let title = lines.next().unwrap_or("").trim_start_matches('#').trim();
            let when = text.lines().find_map(|l| l.strip_prefix("When:")).unwrap_or("").trim();
            SkillInfo { name, title, when }
        })
        .collect()
}

/// One skill's markdown, by name (`rough-cut`), case and spacing forgiven. An unknown name
/// answers with the closest one and the list.
pub fn skill(name: &str) -> Result<&'static str, String> {
    let key = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    let key = key.trim_end_matches(".md");
    if let Some((_, text)) = SKILLS.iter().find(|(n, _)| *n == key) {
        return Ok(text);
    }
    let names: Vec<&str> = SKILLS.iter().map(|(n, _)| *n).collect();
    let hint = kimchi_core::closest(key, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("There is no skill `{name}`.{hint} Skills: {}.", names.join(", ")))
}

/// The expert brief, with the index of skills at the end. Commands are written `family.verb`;
/// each client says how its tools are named.
pub fn brief() -> String {
    let index: Vec<String> = skills().iter().map(|s| format!("- `{}`: {}", s.name, s.title)).collect();
    format!(
        "{}\n## Skills\n\nBefore one of these jobs, load its playbook with `harness.skill {{name}}` (`harness.skills` says when each applies):\n\n{}\n",
        BRIEF.trim_end(),
        index.join("\n")
    )
}

/// `text` with every command in backquotes written as its tool name (`clip.addText` becomes
/// `clip_addText`), for agents whose tools are named that way (the built-in agent, MCP clients).
pub fn as_tools(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, chunk) in text.split('`').enumerate() {
        if i > 0 {
            out.push('`');
        }
        let name = chunk.split([' ', '{']).next().unwrap_or("");
        if i % 2 == 1 && name.contains('.') && crate::spec(name).is_some() {
            out.push_str(&chunk.replacen('.', "_", 1));
        } else {
            out.push_str(chunk);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brief_is_between_800_and_1500_words() {
        let words = brief().split_whitespace().count();
        assert!((800..=1500).contains(&words), "the brief has {words} words");
    }

    #[test]
    fn every_skill_has_a_title_a_when_steps_and_checks() {
        let all = skills();
        assert!((8..=15).contains(&all.len()));
        for s in &all {
            assert!(!s.title.is_empty() && !s.when.is_empty(), "{s:?}");
            let text = skill(s.name).unwrap();
            assert!(text.contains("## Steps") && text.contains("## Checks"), "{} lacks steps or checks", s.name);
            assert!(brief().contains(&format!("`{}`", s.name)), "{} isn't in the brief's index", s.name);
        }
    }

    #[test]
    fn skills_and_the_brief_name_only_real_commands() {
        let mut texts: Vec<(&str, String)> = SKILLS.iter().map(|(n, t)| (*n, t.to_string())).collect();
        texts.push(("brief", brief()));
        for (name, text) in texts {
            // `family.verb` in backquotes, possibly followed by its parameters.
            for chunk in text.split('`').skip(1).step_by(2) {
                let word = chunk.split([' ', '{']).next().unwrap_or("");
                let Some((family, verb)) = word.split_once('.') else { continue };
                let is_command = !family.is_empty()
                    && family.chars().all(|c| c.is_ascii_lowercase())
                    && verb.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && verb.chars().all(|c| c.is_ascii_alphanumeric());
                if is_command && !["src.lib", "plugin.toml"].contains(&word) {
                    assert!(crate::spec(word).is_some(), "{name} names `{word}`, which isn't a command");
                }
            }
        }
    }

    #[test]
    fn commands_become_tool_names() {
        assert_eq!(as_tools("Run `clip.addText {text}` then `src/lib.rs` and `motion.guide`."), "Run `clip_addText {text}` then `src/lib.rs` and `motion_guide`.");
        assert!(as_tools(&brief()).contains("`harness_look`"));
    }

    #[test]
    fn skills_are_found_by_near_names() {
        assert!(skill("Rough cut").unwrap().starts_with("# Rough cut"));
        assert!(skill("3d_product_shot.md").is_ok());
        let e = skill("rough-cuts").unwrap_err();
        assert!(e.contains("Did you mean `rough-cut`?"), "{e}");
    }
}
