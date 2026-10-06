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

use std::path::Path;

use kimchi_core::Project;
use serde::Serialize;

use crate::{Report, Result};

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
/// (`.xml`, `.json`), from its first bytes.
pub fn detect(path: &Path) -> Option<&'static Format> {
    let _ = path;
    None
}

/// An imported timeline: a kimchi project (assets not probed yet, see the module doc) and what
/// changed on the way.
#[derive(Debug, Clone)]
pub struct Imported {
    pub project: Project,
    pub report: Report,
}

/// Reads a project or timeline file. `format` overrides [`detect`].
pub fn import(path: &Path, format: Option<&str>) -> Result<Imported> {
    let _ = (path, format);
    Err("Opening other editors' projects isn't built yet.".into())
}

/// Writes `project` as `format` to `path`; the report says what didn't fit the format.
pub fn export(project: &Project, format: &str, path: &Path) -> Result<Report> {
    let _ = (project, format, path);
    Err("Writing projects for other editors isn't built yet.".into())
}
