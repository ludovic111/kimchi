//! kimchi-media: everything that touches media files, via ffmpeg/ffprobe.
//!
//! TODO: implementations are stubs.

pub mod export;

use std::path::{Path, PathBuf};

use kimchi_core::{Filmstrip, MediaKind, MediaMeta, Waveform};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("ffmpeg wasn't found. Install it (e.g. `brew install ffmpeg`) or set KIMCHI_FFMPEG.")]
    ToolsMissing,
    #[error("unsupported file: {0}")]
    Unsupported(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type MediaResult<T> = Result<T, MediaError>;

/// Paths to the ffmpeg binaries.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    /// Finds ffmpeg/ffprobe: `KIMCHI_FFMPEG`/`KIMCHI_FFPROBE`, next to the
    /// executable (bundled sidecars), then `PATH` and common install locations.
    pub fn locate() -> MediaResult<Self> {
        Err(MediaError::ToolsMissing)
    }
}

/// What ffprobe tells us about a file.
#[derive(Debug, Clone)]
pub struct Probe {
    pub kind: MediaKind,
    pub meta: MediaMeta,
}

pub async fn probe(_tools: &Tools, _path: &Path) -> MediaResult<Probe> {
    Err(MediaError::ToolsMissing)
}

/// Poster frame (JPEG) at most `max_width` wide. For audio, writes nothing and returns false.
pub async fn thumbnail(_tools: &Tools, _path: &Path, _kind: MediaKind, _out: &Path, _max_width: u32) -> MediaResult<bool> {
    Err(MediaError::ToolsMissing)
}

/// Horizontal strip of frames (JPEG) `height` px tall, for drawing clips on the timeline.
pub async fn filmstrip(_tools: &Tools, _path: &Path, _duration: f64, _out: &Path, _height: u32) -> MediaResult<Filmstrip> {
    Err(MediaError::ToolsMissing)
}

/// Audio peaks (little-endian f32 in 0..1) at `peaks_per_second`.
pub async fn waveform(_tools: &Tools, _path: &Path, _out: &Path, _peaks_per_second: u32) -> MediaResult<Waveform> {
    Err(MediaError::ToolsMissing)
}

/// Whether the webview can't play this file directly and needs a proxy.
pub fn needs_proxy(_meta: &MediaMeta, _path: &Path) -> bool {
    false
}

/// H.264/AAC MP4 proxy for preview playback.
pub async fn proxy(_tools: &Tools, _path: &Path, _out: &Path) -> MediaResult<()> {
    Err(MediaError::ToolsMissing)
}

/// Full-resolution PNG of the frame at `time` seconds.
pub async fn grab_frame(_tools: &Tools, _path: &Path, _time: f64, _out: &Path) -> MediaResult<()> {
    Err(MediaError::ToolsMissing)
}
