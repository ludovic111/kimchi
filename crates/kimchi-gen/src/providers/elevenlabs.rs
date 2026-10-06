//! ElevenLabs speech, sound effects and music. Speech models and voices are
//! discovered from the account; audio uses the same queue and provenance as video.
use async_trait::async_trait;
use serde_json::{Value, json};
use crate::{provider::{Ctx, GenError, GenResult, Provider}, types::*, util};

pub struct ElevenLabs;
pub const ID: &str = "elevenlabs";

fn speech(id: &str, name: &str, voices: &[SelectOption]) -> ModelInfo {
    let voice = ParamSpec {
        key: "voice_id".into(), label: "Voice".into(),
        kind: if voices.is_empty() { ParamKind::Text { multiline: false } } else { ParamKind::Select { options: voices.to_vec() } },
        default: json!(voices.first().map(|v| v.value.as_str()).unwrap_or("")),
        help: Some("Choose a voice from your ElevenLabs account, or enter its voice id.".into()),
    };
    ModelInfo { params: vec![voice], featured: true, ..ModelInfo::new(ID, id, name, &[Task::TextToSpeech]) }
}

#[async_trait]
impl Provider for ElevenLabs {
    fn info(&self) -> ProviderInfo {
        ProviderInfo { id: ID.into(), name: "ElevenLabs".into(), kind: ProviderKind::Cloud,
            tagline: "Speech, sound effects and music".into(), website: "https://elevenlabs.io".into(),
            needs_key: true, key_env: vec!["ELEVENLABS_API_KEY".into(), "XI_API_KEY".into()],
            key_url: Some("https://elevenlabs.io/app/settings/api-keys".into()), key_hint: Some("sk_…".into()),
            default_base_url: "https://api.elevenlabs.io".into(), base_url_editable: false,
            tasks: vec![Task::TextToSpeech, Task::TextToAudio] }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let mut speech_models = vec![speech("eleven_multilingual_v2", "Multilingual speech", &[])];
        if let Some(key) = &cx.api_key {
            let voices: Vec<SelectOption> = util::send_json::<Value>(cx, cx.http.get(cx.url("/v1/voices")).header("xi-api-key", key)).await.ok()
                .and_then(|v| v["voices"].as_array().cloned()).unwrap_or_default().iter()
                .filter_map(|v| Some(SelectOption::new(v["voice_id"].as_str()?, v["name"].as_str()?))).collect();
            speech_models = vec![speech("eleven_multilingual_v2", "Multilingual speech", &voices)];
            if let Ok(models) = util::send_json::<Value>(cx, cx.http.get(cx.url("/v1/models")).header("xi-api-key", key)).await {
                let found: Vec<_> = models.as_array().into_iter().flatten().filter(|m| m["can_do_text_to_speech"] == true)
                    .filter_map(|m| Some(speech(m["model_id"].as_str()?, m["name"].as_str()?, &voices))).collect();
                if !found.is_empty() { speech_models = found; }
            }
        }
        speech_models.push(ModelInfo { durations: vec![5., 10., 20., 30.], featured: true,
            params: vec![ParamSpec { key: "loop".into(), label: "Seamless loop".into(), kind: ParamKind::Bool,
                default: json!(false), help: None }],
            ..ModelInfo::new(ID, "eleven_text_to_sound_v2", "Sound effects", &[Task::TextToAudio]) });
        for (id, name) in [("music_v2_5", "Eleven Music 2.5"), ("music_v2", "Eleven Music 2"), ("music_v1", "Eleven Music 1")] {
            speech_models.push(ModelInfo { durations: vec![30., 60., 120., 180., 300., 600.],
                params: vec![ParamSpec { key: "force_instrumental".into(), label: "Instrumental only".into(), kind: ParamKind::Bool,
                    default: json!(true), help: None }],
                ..ModelInfo::new(ID, id, name, &[Task::TextToAudio]) });
        }
        Ok(speech_models)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        util::send(cx, cx.http.get(cx.url("/v1/models")).header("xi-api-key", cx.key()?)).await?;
        Ok("Connected to ElevenLabs".into())
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let (path, body) = match req.task {
            Task::TextToSpeech => {
                let voice = req.param("voice_id").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| GenError::Provider("Choose an ElevenLabs voice before generating speech.".into()))?;
                if !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { return Err(GenError::Provider("Invalid voice id.".into())); }
                (format!("/v1/text-to-speech/{voice}"), json!({"text": req.prompt, "model_id": req.model}))
            }
            Task::TextToAudio if matches!(req.model.as_str(), "music_v1" | "music_v2" | "music_v2_5") => {
                let duration = req.duration.unwrap_or(30.);
                if !duration.is_finite() || !(3.0..=600.0).contains(&duration) { return Err(GenError::Provider("Music duration must be between 3 and 600 seconds.".into())); }
                ("/v1/music".into(), json!({"prompt": req.prompt, "music_length_ms": (duration * 1000.).round() as u64,
                    "model_id": req.model, "force_instrumental": req.param("force_instrumental").and_then(Value::as_bool).unwrap_or(true)}))
            }
            Task::TextToAudio if req.model == "eleven_text_to_sound_v2" => {
                let duration = req.duration.unwrap_or(5.);
                if !duration.is_finite() || !(0.5..=30.0).contains(&duration) { return Err(GenError::Provider("Sound effects must be between 0.5 and 30 seconds.".into())); }
                ("/v1/sound-generation".into(), json!({"text": req.prompt, "duration_seconds": duration,
                    "model_id": req.model, "loop": req.param("loop").and_then(Value::as_bool).unwrap_or(false)}))
            }
            _ => return Err(GenError::Unsupported("Choose a speech, sound effect or music model.".into())),
        };
        cx.report(Progress::message("Generating audio…"));
        let response = util::send(cx, cx.http.post(cx.url(&path)).header("xi-api-key", cx.key()?)
            .query(&[("output_format", "mp3_44100_128")]).json(&body)).await?;
        let bytes = response.bytes().await.map_err(util::net_err(cx))?;
        if bytes.is_empty() { return Err(util::decode_err(cx, "Empty audio response")); }
        Ok(GenOutput { items: vec![OutputItem::bytes(OutputKind::Audio, bytes, "audio/mpeg")], ..Default::default() })
    }
}
