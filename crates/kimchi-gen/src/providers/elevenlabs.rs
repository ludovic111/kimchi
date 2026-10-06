//! ElevenLabs: voices, music and sound effects.
//!
//! Every call is synchronous and answers with the audio bytes (MP3 by default), following the
//! API reference at `elevenlabs.io/docs` (read 2026-10-06):
//! * Speech: `POST /v1/text-to-speech/{voice_id}?output_format=…` `{text, model_id,
//!   language_code?, seed?}`.
//! * Sound effects: `POST /v1/sound-generation` `{text, duration_seconds?, model_id}`.
//! * Music: `POST /v1/music` `{prompt, music_length_ms, model_id, force_instrumental}`.
//! * Voices: `GET /v2/voices` (paged, with `preview_url` samples); models: `GET /v1/models`.
//!
//! Errors come as `{"detail": {"type", "code", "message"}}`.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "elevenlabs";

pub struct ElevenLabs;

/// Rachel, the voice ElevenLabs' own examples use.
pub const DEFAULT_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";
const OUTPUT: &str = "mp3_44100_128";

fn speech(id: &str, name: &str, description: &str, max_chars: u32, price: &str, featured: bool) -> ModelInfo {
    ModelInfo {
        description: Some(description.into()),
        max_chars: Some(max_chars),
        seed: true,
        price: Some(price.into()),
        featured,
        ..ModelInfo::speech(ID, id, name, DEFAULT_VOICE)
    }
}

fn catalog() -> Vec<ModelInfo> {
    vec![
        speech("eleven_v3", "Eleven v3", "The most expressive: audio tags like [whispers] and [laughs], 70+ languages.", 5_000, "≈ $0.10 / 1k characters", true),
        speech("eleven_multilingual_v2", "Multilingual v2", "Steady, lifelike narration in 29 languages.", 10_000, "≈ $0.10 / 1k characters", false),
        speech("eleven_flash_v2_5", "Flash v2.5", "Fast and half the price, 32 languages.", 40_000, "≈ $0.05 / 1k characters", false),
        ModelInfo {
            description: Some("Songs with vocals or instrumentals, 3 s to 5 minutes. Paid plans.".into()),
            instrumental: true,
            price: Some("≈ $0.60 / minute".into()),
            featured: true,
            ..ModelInfo::sound(ID, "music_v2_5", "Eleven Music v2.5", Task::TextToMusic, 3.0, 300.0)
        },
        ModelInfo {
            description: Some("The previous music model.".into()),
            instrumental: true,
            ..ModelInfo::sound(ID, "music_v2", "Eleven Music v2", Task::TextToMusic, 3.0, 300.0)
        },
        ModelInfo {
            description: Some("Foley, impacts, ambiences and loops, 0.5–30 s.".into()),
            max_chars: Some(450),
            price: Some("40 credits / second".into()),
            featured: true,
            params: vec![ParamSpec {
                key: "loop".into(),
                label: "Loop".into(),
                kind: ParamKind::Bool,
                default: json!(false),
                help: Some("Ends where it starts, to repeat seamlessly.".into()),
            }],
            ..ModelInfo::sound(ID, "eleven_text_to_sound_v2", "Sound Effects v2", Task::TextToSound, 0.5, 30.0)
        },
    ]
}

/// Speech models the account can use, from `GET /v1/models` (newer ones show up there first).
async fn listed(cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
    #[derive(Deserialize)]
    struct M {
        model_id: String,
        name: Option<String>,
        description: Option<String>,
        #[serde(default)]
        can_do_text_to_speech: bool,
        maximum_text_length_per_request: Option<u32>,
        #[serde(default)]
        languages: Vec<Lang>,
    }
    #[derive(Deserialize)]
    struct Lang {
        language_id: String,
    }
    let list: Vec<M> = util::send_json(cx, cx.http.get(cx.url("/v1/models")).header("xi-api-key", cx.key()?)).await?;
    Ok(list
        .into_iter()
        .filter(|m| m.can_do_text_to_speech && !m.model_id.contains("turbo") && !m.model_id.contains("conversational"))
        .map(|m| ModelInfo {
            description: m.description.map(|d| util::truncate(&d, 160)),
            max_chars: m.maximum_text_length_per_request,
            seed: true,
            languages: m.languages.into_iter().map(|l| l.language_id).collect(),
            ..ModelInfo::speech(ID, m.model_id.clone(), m.name.unwrap_or(m.model_id), DEFAULT_VOICE)
        })
        .collect())
}

#[derive(Deserialize)]
struct VoiceEntry {
    voice_id: String,
    name: String,
    category: Option<String>,
    description: Option<String>,
    preview_url: Option<String>,
    #[serde(default)]
    labels: Map<String, Value>,
}

impl VoiceEntry {
    fn into_voice(self) -> Voice {
        let label = |k: &str| self.labels.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        let words: Vec<String> = [label("descriptive").or(label("description")), label("accent"), label("use_case")].into_iter().flatten().collect();
        let description = self.description.clone().filter(|d| !d.trim().is_empty()).map(|d| util::truncate(d.trim(), 120)).or_else(|| (!words.is_empty()).then(|| words.join(", ")));
        Voice {
            language: label("language").or_else(|| label("accent")),
            gender: label("gender"),
            description,
            preview_url: self.preview_url.clone().filter(|u| !u.is_empty()),
            custom: !matches!(self.category.as_deref(), Some("premade") | None),
            id: self.voice_id,
            name: self.name,
        }
    }
}

fn post(cx: &Ctx, path: &str) -> GenResult<reqwest::RequestBuilder> {
    Ok(cx.http.post(cx.url(path)).header("xi-api-key", cx.key()?).header("accept", "audio/mpeg").query(&[("output_format", OUTPUT)]))
}

async fn audio(cx: &Ctx, req: reqwest::RequestBuilder, expected: Duration) -> GenResult<GenOutput> {
    // The answer only comes when the sound is made: estimate meanwhile.
    let resp = util::with_estimate(cx, expected, "Generating", util::send(cx, req.timeout(Duration::from_secs(600)))).await?;
    let mime = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("audio/mpeg").to_string();
    let data = resp.bytes().await.map_err(util::net_err(cx))?;
    if data.is_empty() {
        return Err(util::decode_err(cx, "empty audio"));
    }
    let mime = if mime.starts_with("audio/") { mime } else { util::sniff_mime(&data).unwrap_or("audio/mpeg").to_string() };
    Ok(GenOutput { items: vec![OutputItem::bytes(OutputKind::Audio, data, mime)], seed: None, cost_usd: None })
}

#[async_trait]
impl Provider for ElevenLabs {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "ElevenLabs".into(),
            kind: ProviderKind::Cloud,
            tagline: "Lifelike voices in 70+ languages, music and sound effects".into(),
            website: "https://elevenlabs.io".into(),
            needs_key: true,
            key_env: vec!["ELEVENLABS_API_KEY".into(), "XI_API_KEY".into()],
            key_url: Some("https://elevenlabs.io/app/settings/api-keys".into()),
            key_hint: Some("sk_…".into()),
            default_base_url: "https://api.elevenlabs.io".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToSpeech, Task::TextToMusic, Task::TextToSound],
            group: ProviderGroup::Sound,
            quick_start: false,
            base_url_presets: vec![
                ("United States".into(), "https://api.us.elevenlabs.io".into()),
                ("Europe (data residency)".into(), "https://api.eu.residency.elevenlabs.io".into()),
                ("India (data residency)".into(), "https://api.in.residency.elevenlabs.io".into()),
            ],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let mut list = catalog();
        if cx.api_key.is_some()
            && let Ok(more) = listed(cx).await
        {
            for m in more {
                match list.iter_mut().find(|x| x.id == m.id) {
                    // The account's limits and languages are the real ones.
                    Some(known) => {
                        known.max_chars = m.max_chars.or(known.max_chars);
                        known.languages = m.languages;
                    }
                    None => list.push(m),
                }
            }
        }
        Ok(list)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(cx, cx.http.get(cx.url("/v1/user/subscription")).header("xi-api-key", cx.key()?)).await?;
        let tier = v.get("tier").and_then(Value::as_str).unwrap_or("");
        let left = match (v.get("character_count").and_then(Value::as_u64), v.get("character_limit").and_then(Value::as_u64)) {
            (Some(used), Some(limit)) if limit > 0 => format!(", {} of {limit} credits left", limit.saturating_sub(used)),
            _ => String::new(),
        };
        Ok(if tier.is_empty() { "Key works".into() } else { format!("Key works ({tier} plan{left})") })
    }

    async fn voices(&self, cx: &Ctx, _model: &str) -> GenResult<Vec<Voice>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            voices: Vec<VoiceEntry>,
            #[serde(default)]
            has_more: bool,
            next_page_token: Option<String>,
        }
        let mut out = vec![];
        let mut token: Option<String> = None;
        // The account's voices first (clones, designed, saved), then the defaults; a few pages at most.
        for _ in 0..5 {
            let mut q: Vec<(&str, String)> = vec![("page_size", "100".into())];
            if let Some(t) = &token {
                q.push(("next_page_token", t.clone()));
            }
            let page: Page = util::send_json(cx, cx.http.get(cx.url("/v2/voices")).header("xi-api-key", cx.key()?).query(&q)).await?;
            out.extend(page.voices.into_iter().map(VoiceEntry::into_voice));
            match (page.has_more, page.next_page_token) {
                (true, Some(t)) => token = Some(t),
                _ => break,
            }
        }
        out.sort_by_key(|v| !v.custom);
        Ok(out)
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let text = req.prompt.trim();
        match req.task {
            Task::TextToSpeech => {
                let voice = req.voice.as_deref().filter(|v| !v.is_empty()).unwrap_or(DEFAULT_VOICE);
                let mut b = json!({ "text": text, "model_id": req.model });
                // Multilingual v2 detects the language itself and refuses the field.
                if let Some(l) = req.language.as_deref().filter(|l| !l.is_empty() && req.model != "eleven_multilingual_v2") {
                    b["language_code"] = json!(l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase());
                }
                if let Some(s) = req.seed {
                    b["seed"] = json!(s.rem_euclid(4_294_967_295));
                }
                extend(&mut b, &req.params);
                let path = format!("/v1/text-to-speech/{}", url::form_urlencoded::byte_serialize(voice.as_bytes()).collect::<String>());
                let secs = (text.chars().count() as u64 / 200).clamp(3, 60);
                audio(cx, post(cx, &path)?.json(&b), Duration::from_secs(secs)).await
            }
            Task::TextToSound => {
                let mut b = json!({ "text": text, "model_id": req.model });
                if let Some(d) = req.duration {
                    b["duration_seconds"] = json!(d.clamp(0.5, 30.0));
                }
                extend(&mut b, &req.params);
                audio(cx, post(cx, "/v1/sound-generation")?.json(&b), Duration::from_secs(10)).await
            }
            Task::TextToMusic => {
                let mut prompt = text.to_string();
                if let Some(l) = req.lyrics.as_deref().map(str::trim).filter(|l| !l.is_empty() && req.instrumental != Some(true)) {
                    prompt = format!("{prompt}\n\nLyrics:\n{l}");
                }
                let secs = req.duration.unwrap_or(30.0).clamp(3.0, 300.0);
                let mut b = json!({
                    "prompt": prompt,
                    "model_id": req.model,
                    "music_length_ms": (secs * 1000.0).round() as i64,
                    "force_instrumental": req.instrumental.unwrap_or(false),
                });
                extend(&mut b, &req.params);
                audio(cx, post(cx, "/v1/music")?.json(&b), Duration::from_secs(30 + secs as u64 / 2)).await
            }
            other => Err(GenError::Unsupported(format!("ElevenLabs makes sound, not {}", other.label().to_lowercase()))),
        }
    }
}

fn extend(b: &mut Value, params: &Map<String, Value>) {
    if let Some(o) = b.as_object_mut() {
        for (k, v) in params {
            o.insert(k.clone(), v.clone());
        }
    }
}
