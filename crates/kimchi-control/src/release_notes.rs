//! Release notes: `CHANGELOG.md` at the repository root, built into the app.
//!
//! Each release is a `## <version> — <date>` section of Markdown. The window shows the
//! current version's section once after an update ([`take_unseen`]) and on request
//! (`app.whatsNew`); the release workflow puts the same section in `latest.json`'s notes
//! (`kimchi-release notes`), so an available update says what it brings.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The changelog as built into this copy.
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    /// `2026-10-03`, when the heading has one.
    pub date: Option<String>,
    /// The section's Markdown, without its heading.
    pub notes: String,
}

/// Every release in `changelog`, newest first (the file's order).
pub fn parse(changelog: &str) -> Vec<Release> {
    let mut out: Vec<Release> = vec![];
    for line in changelog.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            let head = head.trim();
            let (version, date) = match head.split_once(['—', '–']).or_else(|| head.split_once(" - ")) {
                Some((v, d)) => (v.trim(), Some(d.trim().to_string()).filter(|d| !d.is_empty())),
                None => (head, None),
            };
            out.push(Release { version: version.trim_start_matches('v').to_string(), date, notes: String::new() });
        } else if let Some(r) = out.last_mut() {
            r.notes.push_str(line);
            r.notes.push('\n');
        }
    }
    for r in &mut out {
        r.notes = r.notes.trim().to_string();
    }
    out
}

/// The built-in releases.
pub fn all() -> Vec<Release> {
    parse(CHANGELOG)
}

/// One release's notes (a leading `v` is fine).
pub fn find(version: &str) -> Option<Release> {
    let v = version.trim().trim_start_matches('v');
    all().into_iter().find(|r| r.version == v)
}

/// Releases newer than `since` up to and including this build, newest first.
pub fn since(since: &str) -> Vec<Release> {
    all().into_iter().filter(|r| crate::update::is_newer(&r.version, since) && !crate::update::is_newer(&r.version, crate::update::CURRENT)).collect()
}

/// After an update: the version that ran before, when there are release notes this person
/// hasn't seen ([`since`] it), and remembers this version. `None` on a first install (nothing
/// to compare with) and when nothing changed. `<config>/last-version` holds the version.
pub fn take_unseen(config_dir: &Path) -> Option<String> {
    let file = config_dir.join("last-version");
    let last = std::fs::read_to_string(&file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if last.as_deref() != Some(crate::update::CURRENT) {
        let _ = std::fs::write(&file, crate::update::CURRENT);
    }
    last.filter(|l| !since(l).is_empty())
}

/// The notes for `kimchi-release notes`: one release's section, as Markdown.
pub fn section(changelog: &str, version: &str) -> Option<String> {
    let v = version.trim().trim_start_matches('v');
    parse(changelog).into_iter().find(|r| r.version == v).map(|r| r.notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_changelog_has_this_version() {
        let releases = all();
        assert!(!releases.is_empty());
        assert_eq!(releases[0].version, crate::update::CURRENT, "CHANGELOG.md's first section must be the workspace version");
        assert!(find(crate::update::CURRENT).is_some_and(|r| !r.notes.is_empty()));
        assert!(find(&format!("v{}", crate::update::CURRENT)).is_some());
    }

    #[test]
    fn parses_headings_and_sections() {
        let md = "# Changelog\nintro\n\n## 0.2.0 — 2026-01-02\n### New\n- b\n\n## 0.1.0\n- a\n";
        let r = parse(md);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], Release { version: "0.2.0".into(), date: Some("2026-01-02".into()), notes: "### New\n- b".into() });
        assert_eq!(r[1].date, None);
        assert_eq!(section(md, "v0.1.0").as_deref(), Some("- a"));
    }

    #[test]
    fn shows_new_notes_once_after_an_update() {
        let dir = tempfile::tempdir().unwrap();
        // First install: nothing to show, but the version is remembered.
        assert_eq!(take_unseen(dir.path()), None);
        assert_eq!(take_unseen(dir.path()), None);
        // Coming from 0.1.0: everything since.
        std::fs::write(dir.path().join("last-version"), "0.1.0").unwrap();
        assert_eq!(take_unseen(dir.path()).as_deref(), Some("0.1.0"));
        let shown = since("0.1.0");
        assert!(shown.iter().any(|r| r.version == crate::update::CURRENT));
        assert!(shown.iter().all(|r| r.version != "0.1.0"));
        assert_eq!(take_unseen(dir.path()), None);
    }
}
