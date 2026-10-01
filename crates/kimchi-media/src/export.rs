//! Renders a project to a file by compiling the timeline into one ffmpeg filter graph.

use std::collections::HashMap;
use std::path::PathBuf;

use kimchi_core::{Id, Project};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::{MediaError, MediaResult, Tools};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExportFormat {
    /// H.264 + AAC in MP4. Plays everywhere.
    Mp4,
    /// HEVC + AAC in MP4. Smaller files.
    Hevc,
    /// ProRes 422 HQ + PCM in MOV, for further editing.
    Prores,
    /// VP9 + Opus in WebM.
    Webm,
    /// Animated GIF (no audio).
    Gif,
    /// Audio only (AAC in M4A).
    Audio,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Quality {
    Draft,
    Standard,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ExportSettings {
    pub path: String,
    pub format: ExportFormat,
    pub quality: Quality,
    /// Output size; defaults to the project size.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// Only render this range (seconds).
    pub range: Option<(f64, f64)>,
}

/// Pre-rendered transparent PNGs, one per text clip, the size of the canvas.
/// The UI rasterises text so exports match the preview exactly.
pub type Overlays = HashMap<Id, PathBuf>;

/// Renders `project`. `progress` receives 0.0–1.0.
pub async fn export(
    _tools: &Tools,
    _project: &Project,
    _overlays: &Overlays,
    _settings: &ExportSettings,
    _progress: impl Fn(f64) + Send + Sync,
    _cancel: CancellationToken,
) -> MediaResult<()> {
    Err(MediaError::ToolsMissing)
}
