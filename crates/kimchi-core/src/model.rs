//! The project document: settings, assets, tracks and clips.
//!
//! Times are seconds (`f64`) on the timeline. Positions are in project pixels,
//! relative to the canvas centre.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type Id = Uuid;

pub fn new_id() -> Id {
    Uuid::new_v4()
}

/// Shortest clip the editor allows, in seconds.
pub const MIN_CLIP: f64 = 1.0 / 60.0;
/// Default length of an image, text or solid clip.
pub const DEFAULT_STILL_DURATION: f64 = 5.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub settings: ProjectSettings,
    pub assets: Vec<Asset>,
    /// Index 0 is the top-most track (drawn last, on top).
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub markers: Vec<Marker>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Canvas colour as `#rrggbb`.
    pub background: String,
    pub sample_rate: u32,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self { width: 1920, height: 1080, fps: 30.0, background: "#000000".into(), sample_rate: 48_000 }
    }
}

impl ProjectSettings {
    pub fn aspect_ratio(&self) -> String {
        fn gcd(a: u32, b: u32) -> u32 {
            if b == 0 { a } else { gcd(b, a % b) }
        }
        let g = gcd(self.width, self.height).max(1);
        format!("{}:{}", self.width / g, self.height / g)
    }

    pub fn frame(&self) -> f64 {
        1.0 / self.fps.max(1.0)
    }

    pub fn snap_to_frame(&self, t: f64) -> f64 {
        (t * self.fps).round() / self.fps
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Video,
    Image,
    Audio,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Asset {
    pub id: Id,
    pub name: String,
    pub kind: MediaKind,
    /// Absolute path to the source file.
    pub path: String,
    pub meta: MediaMeta,
    pub origin: AssetOrigin,
    pub created_at: DateTime<Utc>,
    /// Small poster image for the library.
    #[serde(default)]
    pub thumbnail: Option<String>,
    /// Horizontal strip of frames used to draw clips on the timeline.
    #[serde(default)]
    pub filmstrip: Option<Filmstrip>,
    /// Audio peaks file (little-endian f32, `peaks_per_second` values per second).
    #[serde(default)]
    pub waveform: Option<Waveform>,
    /// Browser-friendly transcode used by the preview when the source codec isn't playable.
    #[serde(default)]
    pub proxy: Option<String>,
}

impl Asset {
    /// Length in seconds, or `None` for stills.
    pub fn duration(&self) -> Option<f64> {
        match self.kind {
            MediaKind::Image => None,
            _ => self.meta.duration,
        }
    }

    pub fn is_generated(&self) -> bool {
        matches!(self.origin, AssetOrigin::Generated(_))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MediaMeta {
    pub duration: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub has_video: bool,
    pub has_audio: bool,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Filmstrip {
    pub path: String,
    /// Number of frames laid out left to right in the strip.
    pub frames: u32,
    pub frame_width: u32,
    pub frame_height: u32,
    /// Seconds between two frames.
    pub interval: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Waveform {
    pub path: String,
    pub peaks_per_second: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Imported is the common case; boxing would only add noise.
pub enum AssetOrigin {
    Imported,
    Generated(Generation),
}

/// Everything needed to understand (and redo) how an asset was generated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Generation {
    pub job_id: String,
    pub provider: String,
    pub model: String,
    pub model_name: String,
    pub task: String,
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub seed: Option<i64>,
    /// The full request parameters, so a generation can be re-run or varied.
    pub params: serde_json::Value,
    /// Assets that were fed into the model (reference images, start/end frames).
    #[serde(default)]
    pub inputs: Vec<Id>,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Pictures: video, images, text, solids.
    Video,
    Audio,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: Id,
    pub kind: TrackKind,
    pub name: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub locked: bool,
    /// Sorted by `start`, never overlapping.
    pub clips: Vec<Clip>,
}

impl Track {
    pub fn new(kind: TrackKind, name: impl Into<String>) -> Self {
        Self { id: new_id(), kind, name: name.into(), muted: false, hidden: false, locked: false, clips: vec![] }
    }

    pub fn end(&self) -> f64 {
        self.clips.iter().map(Clip::end).fold(0.0, f64::max)
    }

    pub fn accepts(&self, content: &ClipContent, assets: &[Asset]) -> bool {
        let wants = content.track_kind(assets);
        wants.is_none_or(|k| k == self.kind)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub id: Id,
    pub name: String,
    /// Timeline position of the first frame.
    pub start: f64,
    /// Length on the timeline (already accounts for speed).
    pub duration: f64,
    /// Offset into the source media, in source seconds.
    #[serde(default)]
    pub in_point: f64,
    #[serde(default = "one")]
    pub speed: f64,
    pub content: ClipContent,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "one")]
    pub volume: f64,
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
}

fn one() -> f64 {
    1.0
}

impl Clip {
    pub fn new(name: impl Into<String>, start: f64, duration: f64, content: ClipContent) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            start,
            duration,
            in_point: 0.0,
            speed: 1.0,
            content,
            transform: Transform::default(),
            volume: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
        }
    }

    pub fn end(&self) -> f64 {
        self.start + self.duration
    }

    pub fn contains(&self, t: f64) -> bool {
        t > self.start + 1e-9 && t < self.end() - 1e-9
    }

    pub fn asset_id(&self) -> Option<Id> {
        match self.content {
            ClipContent::Media { asset_id } => Some(asset_id),
            _ => None,
        }
    }

    /// Source time (seconds into the media) shown at timeline time `t`.
    pub fn source_time(&self, t: f64) -> f64 {
        self.in_point + (t - self.start) * self.speed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipContent {
    Media { asset_id: Id },
    Text { style: TextStyle },
    Solid { color: String },
    /// A placeholder for media that is still being generated.
    Pending { job_id: String, kind: MediaKind, prompt: String, model_name: String },
}

impl ClipContent {
    /// Which kind of track this content lives on, if it can be determined.
    pub fn track_kind(&self, assets: &[Asset]) -> Option<TrackKind> {
        match self {
            ClipContent::Media { asset_id } => assets.iter().find(|a| a.id == *asset_id).map(|a| match a.kind {
                MediaKind::Audio => TrackKind::Audio,
                _ => TrackKind::Video,
            }),
            ClipContent::Pending { kind: MediaKind::Audio, .. } => Some(TrackKind::Audio),
            _ => Some(TrackKind::Video),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transform {
    /// Offset of the clip centre from the canvas centre, in project pixels.
    pub x: f64,
    pub y: f64,
    /// 1.0 = fitted to the canvas (contain).
    pub scale: f64,
    /// Degrees, clockwise.
    pub rotation: f64,
    pub opacity: f64,
    #[serde(default)]
    pub fit: Fit,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 1.0, rotation: 0.0, opacity: 1.0, fit: Fit::Contain }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    #[default]
    Contain,
    Cover,
    Stretch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextStyle {
    pub content: String,
    pub font_family: String,
    /// Pixels in project space.
    pub font_size: f64,
    pub font_weight: u16,
    #[serde(default)]
    pub italic: bool,
    pub color: String,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub align: TextAlign,
    #[serde(default = "line_height")]
    pub line_height: f64,
    #[serde(default)]
    pub letter_spacing: f64,
    #[serde(default)]
    pub shadow: bool,
}

fn line_height() -> f64 {
    1.15
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            content: "Title".into(),
            font_family: "Instrument Sans".into(),
            font_size: 96.0,
            font_weight: 600,
            italic: false,
            color: "#ffffff".into(),
            background: None,
            align: TextAlign::Center,
            line_height: line_height(),
            letter_spacing: 0.0,
            shadow: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Marker {
    pub id: Id,
    pub time: f64,
    pub label: String,
    pub color: String,
}

impl Project {
    pub fn new(name: impl Into<String>, settings: ProjectSettings) -> Self {
        let now = Utc::now();
        Self {
            id: new_id(),
            name: name.into(),
            created_at: now,
            updated_at: now,
            settings,
            assets: vec![],
            tracks: vec![Track::new(TrackKind::Video, "Video 1"), Track::new(TrackKind::Audio, "Audio 1")],
            markers: vec![],
        }
    }

    pub fn duration(&self) -> f64 {
        self.tracks.iter().map(Track::end).fold(0.0, f64::max)
    }

    pub fn asset(&self, id: Id) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    pub fn asset_mut(&mut self, id: Id) -> Option<&mut Asset> {
        self.assets.iter_mut().find(|a| a.id == id)
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: Id) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Returns `(track index, clip index)`.
    pub fn locate_clip(&self, clip_id: Id) -> Option<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(ti, t)| t.clips.iter().position(|c| c.id == clip_id).map(|ci| (ti, ci)))
    }

    pub fn clip(&self, clip_id: Id) -> Option<&Clip> {
        self.locate_clip(clip_id).map(|(t, c)| &self.tracks[t].clips[c])
    }

    pub fn clip_mut(&mut self, clip_id: Id) -> Option<&mut Clip> {
        self.locate_clip(clip_id).map(|(t, c)| &mut self.tracks[t].clips[c])
    }

    pub fn clips(&self) -> impl Iterator<Item = (&Track, &Clip)> {
        self.tracks.iter().flat_map(|t| t.clips.iter().map(move |c| (t, c)))
    }

    /// Next free name like "Video 3" for a new track of `kind`.
    pub fn next_track_name(&self, kind: TrackKind) -> String {
        let label = match kind {
            TrackKind::Video => "Video",
            TrackKind::Audio => "Audio",
        };
        let n = self.tracks.iter().filter(|t| t.kind == kind).count() + 1;
        format!("{label} {n}")
    }
}
