//! Shared vocabulary between the editor and every model provider.

use std::path::PathBuf;

use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What a model is asked to do.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    TextToImage,
    /// Edit or restyle one or more reference images.
    ImageToImage,
    TextToVideo,
    /// Animate a start frame (optionally towards an end frame).
    ImageToVideo,
    /// A voice reading the prompt.
    TextToSpeech,
    /// A piece of music described by the prompt (with lyrics, or instrumental).
    TextToMusic,
    /// A sound effect or ambience described by the prompt.
    TextToSound,
}

impl Task {
    pub const ALL: [Task; 7] =
        [Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo, Task::TextToSpeech, Task::TextToMusic, Task::TextToSound];

    pub fn output(self) -> OutputKind {
        match self {
            Task::TextToImage | Task::ImageToImage => OutputKind::Image,
            Task::TextToVideo | Task::ImageToVideo => OutputKind::Video,
            Task::TextToSpeech | Task::TextToMusic | Task::TextToSound => OutputKind::Audio,
        }
    }

    pub fn is_audio(self) -> bool {
        self.output() == OutputKind::Audio
    }

    /// Reads `text_to_speech` (or `textToSpeech`).
    pub fn parse(s: &str) -> Option<Task> {
        let flat: String = s.chars().filter(|c| *c != '_' && *c != '-').collect::<String>().to_ascii_lowercase();
        Task::ALL.into_iter().find(|t| t.as_str().replace('_', "") == flat)
    }

    /// How people call it: `Text to speech`.
    pub fn label(self) -> &'static str {
        match self {
            Task::TextToImage => "Text to image",
            Task::ImageToImage => "Image to image",
            Task::TextToVideo => "Text to video",
            Task::ImageToVideo => "Image to video",
            Task::TextToSpeech => "Speech",
            Task::TextToMusic => "Music",
            Task::TextToSound => "Sound effects",
        }
    }

    pub fn needs_image(self) -> bool {
        matches!(self, Task::ImageToImage | Task::ImageToVideo)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Task::TextToImage => "text_to_image",
            Task::ImageToImage => "image_to_image",
            Task::TextToVideo => "text_to_video",
            Task::ImageToVideo => "image_to_video",
            Task::TextToSpeech => "text_to_speech",
            Task::TextToMusic => "text_to_music",
            Task::TextToSound => "text_to_sound",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    Image,
    Video,
    Audio,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Runs on someone else's GPUs; needs an API key.
    Cloud,
    /// Runs on this machine or the local network.
    Local,
}

/// How Settings and the first-run setup group providers.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProviderGroup {
    /// One key, many companies' models (OpenRouter, fal, Replicate…).
    Gateway,
    /// A company's own image and video models.
    #[default]
    Media,
    /// Voices, music and sound effects.
    Sound,
    /// Servers on this computer or the local network.
    Local,
}

impl ProviderGroup {
    pub const ALL: [ProviderGroup; 4] = [ProviderGroup::Gateway, ProviderGroup::Media, ProviderGroup::Sound, ProviderGroup::Local];

    pub fn label(self) -> &'static str {
        match self {
            ProviderGroup::Gateway => "Gateways",
            ProviderGroup::Media => "Images & video",
            ProviderGroup::Sound => "Sound",
            ProviderGroup::Local => "On this computer",
        }
    }
}

/// Static description of a provider, shown in settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderInfo {
    /// Stable id, e.g. `"openrouter"`.
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    /// One line: what this provider is good for.
    pub tagline: String,
    pub website: String,
    /// Whether an API key is required.
    pub needs_key: bool,
    /// Environment variables checked for a key when none is saved, in order.
    pub key_env: Vec<String>,
    /// Where users create a key.
    pub key_url: Option<String>,
    /// Placeholder hint for the key field (e.g. `"sk-or-…"`).
    pub key_hint: Option<String>,
    /// Base URL used when the user hasn't set one.
    pub default_base_url: String,
    /// Local servers and self-hosted gateways let users point elsewhere.
    pub base_url_editable: bool,
    pub tasks: Vec<Task>,
    /// Where Settings lists it.
    #[serde(default)]
    pub group: ProviderGroup,
    /// Quickest to start with: one key (or nothing to install) opens many models. The first-run
    /// setup offers these first.
    #[serde(default)]
    pub quick_start: bool,
    /// Other addresses the same kind of server usually answers on (local forks, regions), as
    /// `(label, url)`; Settings offers them next to the address field.
    #[serde(default)]
    pub base_url_presets: Vec<(String, String)>,
}

/// Type of a model-specific knob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    Int { min: i64, max: i64, step: i64 },
    Float { min: f64, max: f64, step: f64 },
    Bool,
    Select { options: Vec<SelectOption> },
    Text { multiline: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

impl SelectOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into() }
    }
}

/// A model-specific knob rendered in the "Advanced" section of the generate panel.
/// Values are passed to the provider in [`GenRequest::params`] under `key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    pub kind: ParamKind,
    pub default: Value,
    pub help: Option<String>,
}

/// One model a provider can run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    /// Provider-specific id sent back in [`GenRequest::model`].
    pub id: String,
    pub name: String,
    pub provider: String,
    pub tasks: Vec<Task>,
    pub description: Option<String>,
    /// Supported aspect ratios like `"16:9"`. Empty means any size via width/height.
    pub aspect_ratios: Vec<String>,
    /// Supported clip lengths in seconds (video). Empty means the model decides.
    pub durations: Vec<f64>,
    /// Supported output resolutions like `"720p"`, `"1080p"`, `"1K"`.
    pub resolutions: Vec<String>,
    /// How many outputs one request can return.
    pub max_outputs: u32,
    pub negative_prompt: bool,
    pub seed: bool,
    /// Can be given an end frame in addition to the start frame (image-to-video).
    pub end_frame: bool,
    /// Max reference images for image-to-image / image-to-video.
    pub max_images: u32,
    /// Produces sound along with the video.
    pub audio: bool,
    pub params: Vec<ParamSpec>,
    /// Human hint like `"≈ $0.04 / image"`.
    pub price: Option<String>,
    /// Shown first in pickers.
    pub featured: bool,
    /// Speech: the model reads with one of [`Provider::voices`](crate::Provider::voices).
    #[serde(default)]
    pub voices: bool,
    /// Speech: the voice used when none is chosen.
    #[serde(default)]
    pub default_voice: Option<String>,
    /// Sound: shortest and longest result in seconds, when the length can be asked for
    /// (music, sound effects). `durations` lists fixed choices instead.
    #[serde(default)]
    pub duration_range: Option<(f64, f64)>,
    /// Music: takes lyrics ([`GenRequest::lyrics`]).
    #[serde(default)]
    pub lyrics: bool,
    /// Music: can be asked for no singing ([`GenRequest::instrumental`]).
    #[serde(default)]
    pub instrumental: bool,
    /// Speech: the longest text one request takes, in characters.
    #[serde(default)]
    pub max_chars: Option<u32>,
    /// Languages it speaks or sings (`"en"`, `"fr"`…); empty: not said.
    #[serde(default)]
    pub languages: Vec<String>,
}

impl ModelInfo {
    /// A model with conservative defaults; adjust fields with struct update syntax.
    pub fn new(provider: &str, id: impl Into<String>, name: impl Into<String>, tasks: &[Task]) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            provider: provider.to_string(),
            tasks: tasks.to_vec(),
            description: None,
            aspect_ratios: vec![],
            durations: vec![],
            resolutions: vec![],
            max_outputs: 1,
            negative_prompt: false,
            seed: false,
            end_frame: false,
            max_images: if tasks.iter().any(|t| t.needs_image()) { 1 } else { 0 },
            audio: false,
            params: vec![],
            price: None,
            featured: false,
            voices: false,
            default_voice: None,
            duration_range: None,
            lyrics: false,
            instrumental: false,
            max_chars: None,
            languages: vec![],
        }
    }

    /// A speech model that reads with the provider's voices.
    pub fn speech(provider: &str, id: impl Into<String>, name: impl Into<String>, default_voice: &str) -> Self {
        Self { voices: true, default_voice: Some(default_voice.into()), ..Self::new(provider, id, name, &[Task::TextToSpeech]) }
    }

    /// A music or sound-effect model whose length can be asked for, `min`–`max` seconds.
    pub fn sound(provider: &str, id: impl Into<String>, name: impl Into<String>, task: Task, min: f64, max: f64) -> Self {
        Self { duration_range: Some((min, max)), ..Self::new(provider, id, name, &[task]) }
    }

    /// The length to ask for: `wanted` within the model's range or closest fixed choice, else
    /// `default` (also kept within the range).
    pub fn fit_duration(&self, wanted: Option<f64>, default: f64) -> f64 {
        let d = wanted.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(default);
        if let Some((lo, hi)) = self.duration_range {
            return d.clamp(lo, hi);
        }
        if !self.durations.is_empty() {
            return crate::util::closest_duration(Some(d), &self.durations, d);
        }
        d
    }

    pub fn supports(&self, task: Task) -> bool {
        self.tasks.contains(&task)
    }
}

/// A voice a speech model reads with.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Voice {
    /// Sent back in [`GenRequest::voice`].
    pub id: String,
    pub name: String,
    /// Language or accent, as the provider says it (`"en-US"`, `"British"`, `"multilingual"`).
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub gender: Option<String>,
    /// A few words: `"Warm, calm narrator"`.
    #[serde(default)]
    pub description: Option<String>,
    /// A short sample to listen to, when the provider has one.
    #[serde(default)]
    pub preview_url: Option<String>,
    /// The person's own voices (clones, designed) rather than the provider's.
    #[serde(default)]
    pub custom: bool,
}

impl Voice {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), ..Default::default() }
    }

    pub fn described(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn gender(mut self, gender: impl Into<String>) -> Self {
        self.gender = Some(gender.into());
        self
    }

    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageRole {
    /// Image to edit / style or subject reference.
    Reference,
    /// First frame of a video.
    StartFrame,
    /// Last frame of a video.
    EndFrame,
}

/// An image handed to a model. The editor fills `path`; the harness loads `data`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputImage {
    pub role: ImageRole,
    pub path: String,
    #[serde(default)]
    pub mime: String,
    #[serde(skip)]
    pub data: Bytes,
}

impl InputImage {
    pub fn from_bytes(role: ImageRole, data: impl Into<Bytes>, mime: impl Into<String>) -> Self {
        Self { role, path: String::new(), mime: mime.into(), data: data.into() }
    }

    pub fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }

    /// `data:image/png;base64,…`
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.base64())
    }

    pub fn file_name(&self) -> String {
        format!("input.{}", crate::util::extension_for(&self.mime))
    }
}

/// A generation request, independent of any provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenRequest {
    pub model: String,
    pub task: Task,
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub images: Vec<InputImage>,
    /// `"16:9"`, `"1:1"`, … Providers map it to the closest supported value.
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Exact pixel size, for providers that take one. Usually the project size.
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Seconds (video, music, sound effects).
    #[serde(default)]
    pub duration: Option<f64>,
    /// Speech: the voice id ([`Voice::id`]).
    #[serde(default)]
    pub voice: Option<String>,
    /// Speech and songs: the language (`"en"`, `"fr"`…) when the model takes one.
    #[serde(default)]
    pub language: Option<String>,
    /// Music: the words to sing. Without them, models that need lyrics write their own.
    #[serde(default)]
    pub lyrics: Option<String>,
    /// Music: no singing.
    #[serde(default)]
    pub instrumental: Option<bool>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub seed: Option<i64>,
    /// Number of outputs wanted.
    #[serde(default = "one")]
    pub count: u32,
    /// Generate sound with the video when the model can.
    #[serde(default)]
    pub audio: Option<bool>,
    /// Model-specific values keyed by [`ParamSpec::key`].
    #[serde(default)]
    pub params: Map<String, Value>,
}

fn one() -> u32 {
    1
}

impl GenRequest {
    pub fn new(model: impl Into<String>, task: Task, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            task,
            prompt: prompt.into(),
            negative_prompt: None,
            images: vec![],
            aspect_ratio: None,
            width: None,
            height: None,
            duration: None,
            voice: None,
            language: None,
            lyrics: None,
            instrumental: None,
            resolution: None,
            seed: None,
            count: 1,
            audio: None,
            params: Map::new(),
        }
    }

    pub fn image(&self, role: ImageRole) -> Option<&InputImage> {
        self.images.iter().find(|i| i.role == role)
    }

    /// Start frame, falling back to the first image of any role.
    pub fn start_frame(&self) -> Option<&InputImage> {
        self.image(ImageRole::StartFrame).or_else(|| self.images.first())
    }

    pub fn references(&self) -> impl Iterator<Item = &InputImage> {
        self.images.iter().filter(|i| i.role != ImageRole::EndFrame)
    }

    /// The aspect ratio, falling back to width/height, then to 16:9.
    pub fn aspect(&self) -> String {
        if let Some(a) = &self.aspect_ratio {
            return a.clone();
        }
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => crate::util::ratio_string(w, h),
            _ => "16:9".into(),
        }
    }

    pub fn param(&self, key: &str) -> Option<&Value> {
        self.params.get(key)
    }
}

/// Where a finished output lives before the harness saves it.
#[derive(Debug, Clone)]
pub enum OutputSource {
    Bytes { data: Bytes, mime: String },
    /// Downloaded by the harness, with optional extra headers (e.g. auth).
    Url { url: String, headers: Vec<(String, String)> },
}

#[derive(Debug, Clone)]
pub struct OutputItem {
    pub kind: OutputKind,
    pub source: OutputSource,
}

impl OutputItem {
    pub fn url(kind: OutputKind, url: impl Into<String>) -> Self {
        Self { kind, source: OutputSource::Url { url: url.into(), headers: vec![] } }
    }

    pub fn bytes(kind: OutputKind, data: impl Into<Bytes>, mime: impl Into<String>) -> Self {
        Self { kind, source: OutputSource::Bytes { data: data.into(), mime: mime.into() } }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GenOutput {
    pub items: Vec<OutputItem>,
    /// Seed actually used, when the provider reports it.
    pub seed: Option<i64>,
    pub cost_usd: Option<f64>,
}

/// Progress reported while a job runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Progress {
    /// 0.0–1.0 when known.
    pub fraction: Option<f64>,
    /// Short status such as `"In queue (3 ahead)"` or `"Sampling 12/30"`.
    pub message: Option<String>,
}

impl Progress {
    pub fn message(msg: impl Into<String>) -> Self {
        Self { fraction: None, message: Some(msg.into()) }
    }

    pub fn fraction(f: f64, msg: impl Into<String>) -> Self {
        Self { fraction: Some(f.clamp(0.0, 1.0)), message: Some(msg.into()) }
    }
}

/// A saved output on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedOutput {
    pub path: String,
    pub kind: OutputKind,
    pub mime: String,
}

impl SavedOutput {
    pub fn path_buf(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }
}
