//! Project and timeline files from other editors, read into a kimchi [`Project`] and written
//! from one.
//!
//! Reading gives a project whose assets point at the media files the other app used. Their
//! `meta` is what the file itself says (duration, size, rate when it says them) and their kind is
//! guessed from the file name: `project.importFrom` probes them afterwards and keeps the clips'
//! timing. Files that aren't where the project says are listed in [`Report::missing_media`]
//! (`media.relink` finds them again).
//!
//! Times: other apps count in frames or rational seconds; kimchi in seconds on the timeline.
//! Every format converts through exact rationals where it has them, so a cut lands on the same
//! frame after a round trip.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kimchi_core::{Id, Project};
use serde::Serialize;

use crate::{Report, Result};

pub mod capcut;
pub mod common;
pub mod edl;
pub mod fcpxml;
pub mod mlt;
pub mod openshot;
pub mod otio;
pub mod prproj;
pub mod time;
pub mod xml;
pub mod xmeml;

#[cfg(test)]
mod tests;

/// How well kimchi handles a format in one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Support {
    /// Not at all.
    No,
    /// The cut: clips, their timing and tracks; little else.
    Basic,
    /// The cut plus most of what the format carries that kimchi has: transforms, opacity,
    /// volume, fades, transitions, titles, markers, speed.
    Good,
}

/// A project or timeline format.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Format {
    pub id: &'static str,
    pub label: &'static str,
    /// File extensions, without the dot (a folder for the bundle formats).
    pub extensions: &'static [&'static str],
    /// `apps::App` ids of the apps that read or write it.
    pub apps: &'static [&'static str],
    pub import: Support,
    pub export: Support,
    /// One line on what it is and its limits.
    pub notes: &'static str,
}

pub const FORMATS: &[Format] = &[
    Format {
        id: "otio",
        label: "OpenTimelineIO",
        extensions: &["otio", "otioz", "otiod"],
        apps: &["resolve", "premiere", "avid", "kdenlive", "nuke"],
        import: Support::Good,
        export: Support::Good,
        notes: "The open interchange format of the Academy Software Foundation: every track, clip, gap, transition and marker.",
    },
    Format {
        id: "fcpxml",
        label: "Final Cut Pro XML (FCPXML)",
        extensions: &["fcpxml", "fcpxmld"],
        apps: &["finalcut", "resolve", "imovie", "premiere"],
        import: Support::Good,
        export: Support::Good,
        notes: "Final Cut Pro's own interchange (versions 1.6 to 1.13), also read and written by DaVinci Resolve.",
    },
    Format {
        id: "xmeml",
        label: "Final Cut Pro 7 XML (Premiere XML)",
        extensions: &["xml"],
        apps: &["premiere", "resolve", "finalcut7", "vegas", "avid"],
        import: Support::Good,
        export: Support::Good,
        notes: "What Premiere Pro exports with File › Export › Final Cut Pro XML; read by Resolve and most editors.",
    },
    Format {
        id: "edl",
        label: "CMX 3600 EDL",
        extensions: &["edl"],
        apps: &["premiere", "resolve", "avid", "finalcut", "vegas"],
        import: Support::Basic,
        export: Support::Basic,
        notes: "The oldest common list of cuts: one video track and its sound, reel names and timecodes, dissolves and wipes.",
    },
    Format {
        id: "kdenlive",
        label: "Kdenlive project",
        extensions: &["kdenlive"],
        apps: &["kdenlive"],
        import: Support::Good,
        export: Support::Good,
        notes: "Kdenlive's MLT project, with its frei0r filters kept as frei0r plugins.",
    },
    Format {
        id: "mlt",
        label: "Shotcut / MLT XML",
        extensions: &["mlt"],
        apps: &["shotcut", "kdenlive"],
        import: Support::Good,
        export: Support::Good,
        notes: "Shotcut's project file (MLT XML), with its frei0r filters kept as frei0r plugins.",
    },
    Format {
        id: "openshot",
        label: "OpenShot project",
        extensions: &["osp"],
        apps: &["openshot"],
        import: Support::Good,
        export: Support::Good,
        notes: "OpenShot's JSON project: clips, keyframed position, scale, rotation, alpha and volume, transitions.",
    },
    Format {
        id: "prproj",
        label: "Premiere Pro project",
        extensions: &["prproj"],
        apps: &["premiere"],
        import: Support::Basic,
        export: Support::No,
        notes: "Premiere's own project file, read without Premiere: its sequences' cuts. For more, export Final Cut Pro XML from Premiere.",
    },
    Format {
        id: "capcut",
        label: "CapCut / JianYing draft",
        extensions: &["json"],
        apps: &["capcut"],
        import: Support::Basic,
        export: Support::No,
        notes: "A CapCut draft folder (draft_content.json or draft_info.json). Recent CapCut versions encrypt drafts on Windows and Mac; those can't be read.",
    },
];

/// The format with this id (case-insensitive), with a "did you mean" error.
pub fn format(id: &str) -> Result<&'static Format> {
    if let Some(f) = FORMATS.iter().find(|f| f.id.eq_ignore_ascii_case(id.trim())) {
        return Ok(f);
    }
    let ids: Vec<&str> = FORMATS.iter().map(|f| f.id).collect();
    let hint = kimchi_core::closest(id, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("Unknown project format `{id}`.{hint} Formats: {}.", ids.join(", ")))
}

/// The format a file (or bundle folder) is in, from its name and, when that is ambiguous
/// (`.xml`, `.json`, folders), from what is inside.
pub fn detect(path: &Path) -> Option<&'static Format> {
    let by_id = |id: &str| FORMATS.iter().find(|f| f.id == id);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if path.is_dir() {
        if ext == "fcpxmld" || path.join("Info.fcpxml").is_file() {
            return by_id("fcpxml");
        }
        if ext == "otiod" || path.join("content.otio").is_file() {
            return by_id("otio");
        }
        if capcut::draft_file(path).is_some() {
            return by_id("capcut");
        }
        return None;
    }
    match ext.as_str() {
        "otio" | "otioz" => return by_id("otio"),
        "fcpxml" => return by_id("fcpxml"),
        "edl" => return by_id("edl"),
        "kdenlive" => return by_id("kdenlive"),
        "mlt" => return by_id("mlt"),
        "osp" => return by_id("openshot"),
        "prproj" => return by_id("prproj"),
        _ => {}
    }
    sniff(path).and_then(by_id)
}

/// The format id from a file's first bytes.
fn sniff(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = vec![0u8; 64 * 1024];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    head.truncate(n);
    if head.starts_with(&[0x1f, 0x8b]) {
        return Some("prproj");
    }
    if head.starts_with(b"PK\x03\x04") {
        return Some("otio");
    }
    let text = String::from_utf8_lossy(&head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with('<') {
        if t.contains("<xmeml") {
            return Some("xmeml");
        }
        if t.contains("<fcpxml") {
            return Some("fcpxml");
        }
        if t.contains("<mlt") {
            return Some(if t.contains("kdenlive") { "kdenlive" } else { "mlt" });
        }
        if t.contains("<PremiereData") {
            return Some("prproj");
        }
        return None;
    }
    if t.starts_with('{') {
        if t.contains("\"OTIO_SCHEMA\"") {
            return Some("otio");
        }
        if t.contains("\"materials\"") && (t.contains("\"tracks\"") || t.contains("\"canvas_config\"")) {
            return Some("capcut");
        }
        if t.contains("\"layers\"") || t.contains("\"clips\"") && t.contains("\"files\"") {
            return Some("openshot");
        }
        return None;
    }
    if t.lines().take(20).any(|l| l.trim_start().starts_with("TITLE:") || l.trim_start().starts_with("FCM:")) || t.lines().take(20).any(|l| l.split_whitespace().next().is_some_and(|w| w.len() == 3 && w.chars().all(|c| c.is_ascii_digit())) && l.contains(':')) {
        return Some("edl");
    }
    None
}

/// An imported timeline: a kimchi project (assets not probed yet, see the module doc) and what
/// changed on the way.
#[derive(Debug, Clone)]
pub struct Imported {
    pub project: Project,
    pub report: Report,
}

/// Choices for reading.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Premiere projects: the sequence to read, by name (default: the newest one).
    pub sequence: Option<String>,
}

/// Choices for writing.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Video files standing in for clips the other app can't draw (motion clips, titles,
    /// solids), by clip id: each starts at the clip's first frame and lasts as long. Written as
    /// media clips (formats that carry kimchi's own data keep the clip too).
    pub rendered: HashMap<Id, PathBuf>,
}

/// Reads a project or timeline file. `format` overrides [`detect`].
pub fn import(path: &Path, format: Option<&str>) -> Result<Imported> {
    import_with(path, format, &ImportOptions::default())
}

/// [`import`] with options.
pub fn import_with(path: &Path, format: Option<&str>, opts: &ImportOptions) -> Result<Imported> {
    if !path.exists() {
        return Err(format!("{} doesn't exist.", path.display()));
    }
    let f = match format {
        Some(id) => self::format(id)?,
        None => detect(path).ok_or_else(|| {
            let exts: Vec<String> = FORMATS.iter().flat_map(|f| f.extensions.iter().map(|e| format!(".{e}"))).collect();
            format!("kimchi doesn't know what kind of project {} is. It opens {} (give `format` if the name is unusual).", path.display(), exts.join(", "))
        })?,
    };
    if f.import == Support::No {
        return Err(format!("kimchi can't open {} files.", f.label));
    }
    let imported = match f.id {
        "otio" => otio::read(path),
        "fcpxml" => fcpxml::read(path),
        "xmeml" => xmeml::read(path),
        "edl" => edl::read(path),
        "kdenlive" | "mlt" => mlt::read(path, f.id),
        "openshot" => openshot::read(path),
        "prproj" => prproj::read(path, opts.sequence.as_deref()),
        "capcut" => capcut::read(path),
        other => Err(format!("Opening {other} isn't built yet.")),
    }?;
    tracing::info!(format = f.id, path = %path.display(), clips = imported.project.clips().count(), "imported a project from another editor");
    Ok(imported)
}

/// Writes `project` as `format` to `path`; the report says what didn't fit the format.
pub fn export(project: &Project, format: &str, path: &Path) -> Result<Report> {
    export_with(project, format, path, &ExportOptions::default())
}

/// [`export`] with options.
pub fn export_with(project: &Project, format: &str, path: &Path, opts: &ExportOptions) -> Result<Report> {
    let f = self::format(format)?;
    if f.export == Support::No {
        let writable: Vec<&str> = FORMATS.iter().filter(|f| f.export != Support::No).map(|f| f.id).collect();
        return Err(format!("kimchi can't write {} files. It writes {}.", f.label, writable.join(", ")));
    }
    let mut report = match f.id {
        "otio" => otio::write(project, path, opts),
        "fcpxml" => fcpxml::write(project, path, opts),
        "xmeml" => xmeml::write(project, path, opts),
        "edl" => edl::write(project, path, opts),
        "kdenlive" | "mlt" => mlt::write(project, path, f.id, opts),
        "openshot" => openshot::write(project, path, opts),
        other => Err(format!("Writing {other} isn't built yet.")),
    }?;
    report.format = f.id.to_string();
    tracing::info!(format = f.id, path = %path.display(), "wrote a project for another editor");
    Ok(report)
}

/// The format a destination file name asks for (`.xml` is Final Cut 7 XML).
pub fn format_for_path(path: &Path) -> Option<&'static Format> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    if ext == "xml" {
        return FORMATS.iter().find(|f| f.id == "xmeml");
    }
    FORMATS.iter().find(|f| f.extensions.contains(&ext.as_str()) && f.id != "capcut")
}

/// Writes a text file atomically (a temporary file renamed over it), making its folder.
pub(crate) fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "timeline".into());
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't write {}: {e}", path.display())
    })
}
