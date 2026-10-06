//! Sound models described as data: which field takes the text, the voice, the length, lyrics and
//! language. fal and Replicate host many of the same speech, music and sound-effect models with
//! inputs that differ per model; their tables (`providers/fal/audio.rs`, `providers/replicate.rs`)
//! list each model with these, and one mapping turns a [`GenRequest`] into its input.

use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::provider::{GenError, GenResult};
use crate::types::*;
use crate::voices;

/// How an endpoint takes the length.
#[derive(Clone, Copy)]
pub(crate) enum Len {
    None,
    /// Seconds as a number: (field, min, max, default when none is asked; `None`: the model decides).
    Secs(&'static str, f64, f64, Option<f64>),
    /// Whole seconds.
    Int(&'static str, f64, f64, Option<f64>),
    /// Milliseconds.
    Ms(&'static str, f64, f64, Option<f64>),
}

#[derive(Clone, Copy)]
pub(crate) enum VoiceField {
    None,
    Flat(&'static str),
    /// `{outer: {inner: id}}` (MiniMax's `voice_setting.voice_id`).
    Nested(&'static str, &'static str),
}

#[derive(Clone, Copy)]
pub(crate) enum VoiceList {
    None,
    ElevenLabs,
    MiniMax,
    Gemini,
    Xai,
    /// Kokoro's American English voices (fal hosts one endpoint per language).
    Kokoro,
    KokoroAll,
    Qwen,
    /// `(id, description)`.
    Static(&'static [(&'static str, &'static str)]),
}

impl VoiceList {
    fn voices(self) -> Vec<Voice> {
        match self {
            VoiceList::None => vec![],
            VoiceList::ElevenLabs => voices::elevenlabs(),
            VoiceList::MiniMax => voices::minimax(),
            VoiceList::Gemini => voices::gemini(),
            VoiceList::Xai => voices::xai(),
            VoiceList::Kokoro => voices::kokoro(true),
            VoiceList::KokoroAll => voices::kokoro(false),
            VoiceList::Qwen => voices::qwen(),
            VoiceList::Static(list) => list.iter().map(|(id, d)| Voice::new(*id, id.replace('_', " ")).described(*d)).collect(),
        }
    }
}

/// How the language is sent.
#[derive(Clone, Copy)]
pub(crate) enum Lang {
    None,
    /// ISO code as given (`language_code: "fr"`).
    Code(&'static str),
    /// The language's English name (`language_boost: "French"`).
    Name(&'static str),
    /// The tag as given (`language_code: "fr-FR"`).
    Tag(&'static str),
}

/// What a music endpoint does with lyrics.
#[derive(Clone, Copy)]
pub(crate) enum Lyrics {
    /// Lyrics, if any, go into the prompt (Lyria, ElevenLabs).
    InPrompt,
    Field(&'static str),
    /// The field must be filled unless the piece is instrumental.
    Required(&'static str),
}

pub(crate) struct Sound {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub task: Task,
    /// The field the text or prompt goes in.
    pub text: &'static str,
    pub voice: VoiceField,
    pub voices: VoiceList,
    pub default_voice: &'static str,
    pub len: Len,
    pub lyrics: Lyrics,
    /// Field for "no singing".
    pub instrumental: Option<&'static str>,
    pub lang: Lang,
    pub negative: bool,
    pub seed: bool,
    pub max_chars: Option<u32>,
    pub fixed: &'static [(&'static str, Fixed)],
    pub price: &'static str,
    pub featured: bool,
    /// Typical run time in seconds.
    pub secs: u64,
    /// Replicate: the version to run, for models that aren't official.
    pub version: Option<&'static str>,
}

#[derive(Clone, Copy)]
pub(crate) enum Fixed {
    Str(&'static str),
    Int(i64),
}

pub(crate) const fn speech(id: &'static str, name: &'static str, description: &'static str, text: &'static str, voices: VoiceList, default_voice: &'static str) -> Sound {
    Sound {
        id,
        name,
        description,
        task: Task::TextToSpeech,
        text,
        voice: VoiceField::Flat("voice"),
        voices,
        default_voice,
        len: Len::None,
        lyrics: Lyrics::InPrompt,
        instrumental: None,
        lang: Lang::None,
        negative: false,
        seed: false,
        max_chars: None,
        fixed: &[],
        price: "",
        featured: false,
        secs: 10,
        version: None,
    }
}

pub(crate) const fn music(id: &'static str, name: &'static str, description: &'static str, len: Len) -> Sound {
    Sound { task: Task::TextToMusic, text: "prompt", voice: VoiceField::None, voices: VoiceList::None, default_voice: "", len, secs: 60, ..speech(id, name, description, "prompt", VoiceList::None, "") }
}

pub(crate) const fn effect(id: &'static str, name: &'static str, description: &'static str, len: Len) -> Sound {
    Sound { task: Task::TextToSound, secs: 20, ..music(id, name, description, len) }
}

/// The sound in `table` with this id.
pub(crate) fn find(table: &'static [Sound], id: &str) -> Option<&'static Sound> {
    table.iter().find(|s| s.id == id)
}

impl Len {
    fn range(self) -> Option<(f64, f64)> {
        match self {
            Len::None => None,
            Len::Secs(_, lo, hi, _) | Len::Int(_, lo, hi, _) | Len::Ms(_, lo, hi, _) => Some((lo, hi)),
        }
    }

    fn encode(self, wanted: Option<f64>) -> Option<(&'static str, Value)> {
        let (field, lo, hi, default) = match self {
            Len::None => return None,
            Len::Secs(f, lo, hi, d) | Len::Int(f, lo, hi, d) | Len::Ms(f, lo, hi, d) => (f, lo, hi, d),
        };
        let d = wanted.or(default)?.clamp(lo, hi);
        Some((
            field,
            match self {
                Len::Int(..) => json!(d.round() as i64),
                Len::Ms(..) => json!((d * 1000.0).round() as i64),
                _ => json!((d * 10.0).round() / 10.0),
            },
        ))
    }
}

pub(crate) fn model_info(provider: &str, s: &Sound) -> ModelInfo {
    ModelInfo {
        description: Some(s.description.into()),
        duration_range: s.len.range(),
        voices: !matches!(s.voices, VoiceList::None),
        default_voice: (!s.default_voice.is_empty()).then(|| s.default_voice.into()),
        lyrics: !matches!(s.lyrics, Lyrics::InPrompt),
        instrumental: s.instrumental.is_some(),
        max_chars: s.max_chars,
        negative_prompt: s.negative,
        seed: s.seed,
        price: (!s.price.is_empty()).then(|| s.price.into()),
        featured: s.featured,
        ..ModelInfo::new(provider, s.id, s.name, &[s.task])
    }
}

pub(crate) fn voices_for(table: &'static [Sound], id: &str) -> Vec<Voice> {
    find(table, id).map(|s| s.voices.voices()).unwrap_or_default()
}

pub(crate) fn input_for(s: &Sound, req: &GenRequest) -> GenResult<Map<String, Value>> {
    let mut b = Map::new();
    let mut text = req.prompt.trim().to_string();
    let lyrics = req.lyrics.as_deref().map(str::trim).filter(|l| !l.is_empty());
    let instrumental = req.instrumental.unwrap_or(false);
    match s.lyrics {
        Lyrics::InPrompt => {
            if let (Some(l), false, Task::TextToMusic) = (lyrics, instrumental, s.task) {
                text = format!("{text}\n\nLyrics:\n{l}");
            }
        }
        Lyrics::Field(f) => {
            if let Some(l) = lyrics.filter(|_| !instrumental) {
                b.insert(f.into(), json!(l));
            }
        }
        Lyrics::Required(f) => match lyrics {
            Some(l) if !instrumental => {
                b.insert(f.into(), json!(l));
            }
            _ if instrumental && s.instrumental.is_some() => {}
            _ => {
                let what = if instrumental { "can't make instrumentals" } else { "sings the lyrics you give it" };
                return Err(GenError::Unsupported(format!("{} {what}: write lyrics, or choose another music model.", s.name)));
            }
        },
    }
    if let Some(max) = s.max_chars
        && text.chars().count() > max as usize
    {
        return Err(GenError::Unsupported(format!("{} takes up to {max} characters; this has {}.", s.name, text.chars().count())));
    }
    b.insert(s.text.into(), json!(text));
    let voice = req.voice.as_deref().filter(|v| !v.is_empty()).unwrap_or(s.default_voice);
    if !voice.is_empty() {
        match s.voice {
            VoiceField::None => {}
            VoiceField::Flat(f) => {
                b.insert(f.into(), json!(voice));
            }
            VoiceField::Nested(outer, inner) => {
                b.insert(outer.into(), json!({ inner: voice }));
            }
        }
    }
    // Always said: some models default to instrumental.
    if let Some(field) = s.instrumental {
        b.insert(field.into(), json!(instrumental));
    }
    if let Some(l) = req.language.as_deref().filter(|l| !l.trim().is_empty()) {
        match s.lang {
            Lang::None => {}
            Lang::Code(f) => {
                b.insert(f.into(), json!(l.trim().split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase()));
            }
            Lang::Name(f) => {
                if let Some(name) = voices::language_name(l) {
                    b.insert(f.into(), json!(name));
                }
            }
            Lang::Tag(f) => {
                b.insert(f.into(), json!(l.trim()));
            }
        }
    }
    if let Some((f, v)) = s.len.encode(req.duration) {
        b.insert(f.into(), v);
    }
    if s.negative
        && let Some(n) = req.negative_prompt.as_ref().filter(|n| !n.trim().is_empty())
    {
        b.insert("negative_prompt".into(), json!(n));
    }
    if s.seed
        && let Some(seed) = req.seed
    {
        b.insert("seed".into(), json!(seed));
    }
    for (k, v) in s.fixed {
        b.insert(
            (*k).into(),
            match v {
                Fixed::Str(x) => json!(x),
                Fixed::Int(n) => json!(n),
            },
        );
    }
    for (k, v) in &req.params {
        b.insert(k.clone(), v.clone());
    }
    Ok(b)
}

pub(crate) fn expected(s: &Sound) -> Duration {
    Duration::from_secs(s.secs)
}

/// The generic mapping for a sound endpoint added by id: `prompt`, plus the common fields.
pub(crate) fn generic_input(req: &GenRequest) -> Map<String, Value> {
    let mut b = Map::new();
    b.insert("prompt".into(), json!(req.prompt));
    if req.task == Task::TextToSpeech {
        b.insert("text".into(), json!(req.prompt));
    }
    if let Some(v) = &req.voice {
        b.insert("voice".into(), json!(v));
    }
    if let Some(d) = req.duration {
        b.insert("duration".into(), json!(d));
    }
    if let Some(l) = &req.lyrics {
        b.insert("lyrics".into(), json!(l));
    }
    if let Some(s) = req.seed {
        b.insert("seed".into(), json!(s));
    }
    for (k, v) in &req.params {
        b.insert(k.clone(), v.clone());
    }
    b
}


/// `90 s` → "1 minute 30 seconds", for models that read the length from the prompt.
pub fn spoken_length(secs: f64) -> String {
    let s = secs.round().max(1.0) as u64;
    let (m, r) = (s / 60, s % 60);
    let unit = |n: u64, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    match (m, r) {
        (0, r) => unit(r, "second"),
        (m, 0) => unit(m, "minute"),
        (m, r) => format!("{} {}", unit(m, "minute"), unit(r, "second")),
    }
}
