//! The project document: settings, assets, tracks and clips.
//!
//! Times are seconds (`f64`) on the timeline. Positions are in project pixels,
//! relative to the canvas centre.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::anim::{KeyValue, Keyframes, number_at, value_at};
use crate::effects::{EFFECT_PROPS, Effects};
use crate::motion::{Scene, TemplateRef};
use crate::transition::Transition;

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
    /// A video track whose titles are the captions (exported as subtitles too).
    #[serde(default, skip_serializing_if = "is_false")]
    pub captions: bool,
    /// Sorted by `start`, never overlapping.
    pub clips: Vec<Clip>,
}

impl Track {
    pub fn new(kind: TrackKind, name: impl Into<String>) -> Self {
        Self { id: new_id(), kind, name: name.into(), muted: false, hidden: false, locked: false, captions: false, clips: vec![] }
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
    /// Plays the source range backwards (the first frame on the timeline is the range's last).
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
    pub content: ClipContent,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "one")]
    pub volume: f64,
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    /// Animated properties ([`CLIP_PROPS`]), times in seconds from the clip's start.
    #[serde(default, skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
    /// Colour corrections, vignette, sharpen, chroma key, LUT.
    #[serde(default, skip_serializing_if = "Effects::is_default")]
    pub effects: Effects,
    /// How the clip comes in at its start (see [`crate::transition`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
    /// Motion clips rendered ahead of time: the frames to play instead of drawing the scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<Rendered>,
}

/// A motion clip rendered ahead ("Render" on the timeline): its frames in a file, played instead
/// of drawing the scene while they still match it. When the scene (or the project's size or
/// frame rate, or a picture it uses) changes, the key no longer matches and the clip is drawn
/// live again until it is rendered again. Clips that aren't rendered are drawn live in the
/// preview (fast settings) and at full quality in the export.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rendered {
    /// The video file (lossless, with transparency).
    pub file: String,
    /// What it was made from (see `kimchi_media::render::cache::key`).
    pub key: String,
    /// Scene time of the first frame.
    pub from: f64,
    pub fps: f64,
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    /// The engine that made it (`standard` or `path`).
    pub engine: String,
}

impl Rendered {
    /// Does it have the frame for scene time `t`?
    pub fn covers(&self, t: f64) -> bool {
        let i = ((t - self.from) * self.fps).round();
        i >= 0.0 && i < self.frames as f64
    }
}

fn one() -> f64 {
    1.0
}

fn is_false(b: &bool) -> bool {
    !*b
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
            reverse: false,
            content,
            transform: Transform::default(),
            volume: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            keyframes: Keyframes::new(),
            effects: Effects::default(),
            transition: None,
            rendered: None,
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

    /// Source time (seconds into the media) shown at timeline time `t`. Outside the clip (a
    /// transition plays it on past its edges) the source continues the same way.
    pub fn source_time(&self, t: f64) -> f64 {
        let local = t - self.start;
        if self.reverse { self.in_point + (self.duration - local) * self.speed } else { self.in_point + local * self.speed }
    }

    /// Source seconds the clip uses: `(in_point, in_point + duration × speed)`.
    pub fn source_range(&self) -> (f64, f64) {
        (self.in_point, self.in_point + self.duration * self.speed)
    }

    /// The part of the clip on the timeline between `a` and `b` (inside it), as a clip with the
    /// same id: its in-point follows the source (backwards for reversed clips), keyframes stay
    /// where they were on the timeline. Fades and transitions are left to the caller.
    pub fn cut(&self, a: f64, b: f64) -> Clip {
        let mut c = self.clone();
        let (a, b) = (a.max(self.start), b.min(self.end()));
        c.start = a;
        c.duration = (b - a).max(0.0);
        c.in_point = if self.reverse { self.source_time(b) } else { self.source_time(a) };
        crate::anim::shift(&mut c.keyframes, -(a - self.start));
        c
    }

    /// Source seconds available before the clip's first frame and after its last (how far
    /// its start and end can be dragged out), for a source `len` seconds long.
    pub fn room(&self, len: f64) -> (f64, f64) {
        let (lo, hi) = self.source_range();
        let (before, after) = ((lo / self.speed).max(0.0), ((len - hi) / self.speed).max(0.0));
        if self.reverse { (after, before) } else { (before, after) }
    }

    /// Scene time a motion clip shows at timeline time `t` (like [`Self::source_time`]).
    pub fn scene_time(&self, t: f64) -> f64 {
        self.source_time(t)
    }

    /// The clip's effects at timeline time `t`, with their keyframes applied.
    pub fn effects_at(&self, t: f64) -> Effects {
        let mut e = self.effects.clone();
        if self.keyframes.is_empty() {
            return e;
        }
        let local = t - self.start;
        for name in EFFECT_PROPS {
            if let Some(v) = number_at(&self.keyframes, name, local) {
                e.set(name, v);
            }
        }
        e
    }

    /// Does anything about the clip change while it plays (keyframes, or a moving scene)?
    pub fn is_animated(&self) -> bool {
        !self.keyframes.is_empty() || matches!(self.content, ClipContent::Motion { .. })
    }

    /// Where the clip's picture is at timeline time `t`: the transform with keyframes applied.
    pub fn placement_at(&self, t: f64) -> Placement {
        let local = t - self.start;
        let k = &self.keyframes;
        let tf = &self.transform;
        let get = |name: &str, base: f64| number_at(k, name, local).unwrap_or(base);
        let (mut x, mut y) = (get("x", tf.x), get("y", tf.y));
        if let Some(p) = k.get("position").and_then(|keys| value_at(keys, local)).and_then(|v| v.as_vec(2)) {
            (x, y) = (p[0], p[1]);
        }
        Placement {
            x,
            y,
            scale_x: get("scale", tf.scale) * get("scaleX", 1.0),
            scale_y: get("scale", tf.scale) * get("scaleY", 1.0),
            rotation: get("rotation", tf.rotation),
            opacity: get("opacity", tf.opacity).clamp(0.0, 1.0),
            blur: get("blur", 0.0).max(0.0),
            fit: tf.fit,
        }
    }

    /// Volume at timeline time `t` (keyframes on `volume`, else the clip's).
    pub fn volume_at(&self, t: f64) -> f64 {
        number_at(&self.keyframes, "volume", t - self.start).unwrap_or(self.volume).clamp(0.0, 4.0)
    }

    /// A text clip's style at timeline time `t`, with `fontSize`, `color` and `letterSpacing`
    /// keyframes applied.
    pub fn text_at(&self, t: f64) -> Option<TextStyle> {
        let ClipContent::Text { style } = &self.content else { return None };
        let mut style = style.clone();
        let local = t - self.start;
        if let Some(v) = number_at(&self.keyframes, "fontSize", local) {
            style.font_size = v.max(0.0);
        }
        if let Some(v) = number_at(&self.keyframes, "letterSpacing", local) {
            style.letter_spacing = v;
        }
        if let Some(KeyValue::Text(c)) = self.keyframes.get("color").and_then(|keys| value_at(keys, local)) {
            style.color = c;
        }
        Some(style)
    }

    /// The scale reached anywhere in the clip (to decode pictures sharply enough).
    pub fn max_scale(&self) -> f64 {
        let mut m = self.transform.scale.max(0.0);
        for name in ["scale", "scaleX", "scaleY"] {
            for key in self.keyframes.get(name).into_iter().flatten() {
                let v = key.value.as_vec(2).map_or(0.0, |v| v.into_iter().fold(0.0, f64::max));
                let v = if name == "scale" { v } else { v * self.transform.scale };
                m = m.max(v);
            }
        }
        m
    }
}

/// Clip properties that take keyframes. Picture clips: x, y, position ([x, y]), scale, scaleX,
/// scaleY, rotation, opacity, blur, and the effects brightness, contrast, saturation,
/// temperature, tint, vignette, sharpen. Sound: volume. Text clips also: fontSize, color,
/// letterSpacing.
pub const CLIP_PROPS: &[&str] = &[
    "x",
    "y",
    "position",
    "scale",
    "scaleX",
    "scaleY",
    "rotation",
    "opacity",
    "blur",
    "volume",
    "fontSize",
    "color",
    "letterSpacing",
    "brightness",
    "contrast",
    "saturation",
    "temperature",
    "tint",
    "vignette",
    "sharpen",
];

/// Checks that `name` can be keyframed on `content` and that `value` fits it.
pub fn check_clip_key(content: &ClipContent, name: &str, value: &KeyValue) -> Result<(), String> {
    let text_only = ["fontSize", "color", "letterSpacing"];
    if !CLIP_PROPS.contains(&name) {
        let hint = crate::closest(name, CLIP_PROPS).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        return Err(format!("Clips can't animate `{name}`.{hint} Clip properties: {}.", CLIP_PROPS.join(", ")));
    }
    if text_only.contains(&name) && !matches!(content, ClipContent::Text { .. }) {
        return Err(format!("`{name}` is for text clips; for motion clips animate the layer inside the scene (motion.setKeyframes)."));
    }
    match name {
        "color" => {
            let c = value.as_str().ok_or("color takes a colour like \"#ffffff\"")?;
            crate::motion::check_color(c, "color")
        }
        "position" => value.as_vec(2).map(|_| ()).ok_or_else(|| "position takes [x, y]".to_string()),
        _ => value.as_f64().map(|_| ()).ok_or_else(|| format!("{name} takes a number")),
    }
}

/// A picture clip's transform at one instant (keyframes applied).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    /// `transform.scale` × `scaleX`/`scaleY`.
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation: f64,
    pub opacity: f64,
    /// Gaussian blur radius in project pixels.
    pub blur: f64,
    pub fit: Fit,
}

impl Placement {
    /// As a plain [`Transform`] (uniform scale: the larger of the two).
    pub fn transform(&self) -> Transform {
        Transform { x: self.x, y: self.y, scale: self.scale_x.max(self.scale_y), rotation: self.rotation, opacity: self.opacity, fit: self.fit }
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
    /// Motion graphics or a 3D scene, drawn by kimchi (see [`crate::motion`]).
    Motion {
        scene: Scene,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<TemplateRef>,
    },
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

    /// The captions track (the top-most one if there are several).
    pub fn caption_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.captions)
    }

    /// Every caption, by time: (track, clip, words).
    pub fn captions(&self) -> Vec<(&Track, &Clip, &str)> {
        let mut out: Vec<(&Track, &Clip, &str)> = self
            .tracks
            .iter()
            .filter(|t| t.captions)
            .flat_map(|t| {
                t.clips.iter().filter_map(move |c| match &c.content {
                    ClipContent::Text { style } => Some((t, c, style.content.as_str())),
                    _ => None,
                })
            })
            .collect();
        out.sort_by(|a, b| a.1.start.total_cmp(&b.1.start));
        out
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
