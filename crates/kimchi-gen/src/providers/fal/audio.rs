//! fal's sound models: speech, music and sound effects, through the same queue as pictures.
//!
//! Their inputs differ more than the picture models' do (the text is `text`, `prompt` or
//! `text_prompt`; lengths are seconds, whole seconds or milliseconds; voices are flat or nested),
//! so each endpoint's fields are listed here as fal's queue OpenAPI gives them
//! (`fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`, read 2026-10-06). Results come back
//! as `{audio: File}`, `{audio: [File]}` or `{audio_file: File}`.

use serde_json::Value;

use crate::sound::*;
use crate::types::*;

pub(crate) const SOUNDS: &[Sound] = &[
    // ---- speech ----
    Sound {
        lang: Lang::Code("language_code"),
        seed: true,
        max_chars: Some(5000),
        price: "$0.08 / 1k characters",
        featured: true,
        ..speech("elevenlabs/tts/eleven-v4", "ElevenLabs v4", "ElevenLabs' newest voices: natural, expressive, many languages.", "text", VoiceList::ElevenLabs, "Rachel")
    },
    Sound {
        lang: Lang::Code("language_code"),
        seed: true,
        max_chars: Some(5000),
        price: "$0.04 / 1k characters",
        secs: 5,
        ..speech("elevenlabs/tts/eleven-v4-turbo", "ElevenLabs v4 Turbo", "Faster, cheaper ElevenLabs v4.", "text", VoiceList::ElevenLabs, "Rachel")
    },
    Sound {
        lang: Lang::Code("language_code"),
        max_chars: Some(5000),
        price: "$0.10 / 1k characters",
        ..speech("fal-ai/elevenlabs/tts/eleven-v3", "ElevenLabs v3", "Expressive speech with audio tags like [whispers] and [laughs].", "text", VoiceList::ElevenLabs, "Rachel")
    },
    Sound {
        voice: VoiceField::Nested("voice_setting", "voice_id"),
        lang: Lang::Name("language_boost"),
        max_chars: Some(10_000),
        fixed: &[("output_format", Fixed::Str("url"))],
        price: "$0.10 / 1k characters",
        ..speech("fal-ai/minimax/speech-2.8-hd", "MiniMax Speech 2.8 HD", "MiniMax's studio-quality voices; pauses with <#0.5#>, sounds like (laughs).", "prompt", VoiceList::MiniMax, "Wise_Woman")
    },
    Sound {
        voice: VoiceField::Nested("voice_setting", "voice_id"),
        lang: Lang::Name("language_boost"),
        max_chars: Some(10_000),
        fixed: &[("output_format", Fixed::Str("url"))],
        price: "$0.06 / 1k characters",
        secs: 5,
        ..speech("fal-ai/minimax/speech-2.8-turbo", "MiniMax Speech 2.8 Turbo", "Faster, cheaper MiniMax speech.", "prompt", VoiceList::MiniMax, "Wise_Woman")
    },
    Sound {
        price: "$0.045 / 1k characters",
        ..speech("google/gemini-3.8-flash-tts", "Gemini 3.8 Flash TTS", "Google's speech: 30 voices, steerable with plain-language style.", "prompt", VoiceList::Gemini, "Kore")
    },
    Sound {
        lang: Lang::Code("language"),
        max_chars: Some(15_000),
        price: "$0.015 / request",
        ..speech("xai/tts/v1", "Grok TTS", "xAI's voices, cheap and quick.", "text", VoiceList::Xai, "eve")
    },
    Sound {
        lang: Lang::Name("language"),
        price: "$0.09 / request",
        ..speech("fal-ai/qwen-3-tts/text-to-speech/1.7b", "Qwen3 TTS", "Alibaba's speech; strong in Chinese, Japanese and Korean.", "text", VoiceList::Qwen, "Vivian")
    },
    Sound {
        price: "$0.02 / 1k characters",
        secs: 5,
        ..speech("fal-ai/kokoro/american-english", "Kokoro", "Small open model: quick, cheap American English voices.", "prompt", VoiceList::Kokoro, "af_heart")
    },
    // ---- music ----
    Sound {
        instrumental: Some("force_instrumental"),
        seed: true,
        price: "$0.60 / minute",
        featured: true,
        ..music("elevenlabs/music/v2.5", "Eleven Music v2.5", "Full songs with vocals or instrumentals, from 3 s to 10 minutes.", Len::Ms("music_length_ms", 3.0, 600.0, Some(30.0)))
    },
    Sound {
        lyrics: Lyrics::Required("lyrics"),
        seed: true,
        price: "$0.002 / second",
        ..music("minimax/music-3", "MiniMax Music 3", "Songs that sing your lyrics; mark sections with [verse], [chorus].", Len::Int("duration", 1.0, 300.0, Some(60.0)))
    },
    Sound {
        lyrics: Lyrics::Required("lyrics"),
        instrumental: Some("is_instrumental"),
        price: "$0.15 / track",
        ..music("fal-ai/minimax-music/v2.6", "MiniMax Music 2.6", "Songs from a style and lyrics, or instrumentals.", Len::None)
    },
    Sound { price: "$0.08 / track", ..music("fal-ai/lyria3/pro", "Lyria 3 Pro", "Google DeepMind's music model; write vocals and lyrics into the prompt.", Len::None) },
    Sound { price: "$0.04 / track", secs: 40, ..music("fal-ai/lyria3", "Lyria 3", "Faster, cheaper Lyria 3.", Len::None) },
    Sound {
        seed: true,
        price: "$0.20 / track",
        ..music("fal-ai/stable-audio-25/text-to-audio", "Stable Audio 2.5", "Stability's music and loops, up to 3 minutes.", Len::Int("seconds_total", 1.0, 190.0, Some(30.0)))
    },
    Sound {
        negative: true,
        seed: true,
        price: "$0.04 / track",
        ..music("fal-ai/stable-audio-3/medium/text-to-audio", "Stable Audio 3", "Stability's newest: music up to six minutes.", Len::Secs("duration", 1.0, 380.0, Some(30.0)))
    },
    Sound {
        instrumental: Some("instrumental"),
        seed: true,
        price: "$0.0002 / second",
        ..music("fal-ai/ace-step/prompt-to-audio", "ACE-Step", "Open model: songs or instrumentals from a description, very cheap.", Len::Int("duration", 5.0, 240.0, Some(60.0)))
    },
    Sound {
        negative: true,
        seed: true,
        price: "$0.10 / track",
        ..music("beatoven/music-generation", "Beatoven", "Royalty-free background music for videos.", Len::Secs("duration", 5.0, 150.0, Some(30.0)))
    },
    // ---- sound effects ----
    Sound {
        text: "text",
        max_chars: Some(450),
        price: "$0.002 / second",
        featured: true,
        secs: 10,
        ..effect("fal-ai/elevenlabs/sound-effects/v2", "ElevenLabs Sound Effects", "Foley, impacts and ambiences, 0.5–22 s.", Len::Secs("duration_seconds", 0.5, 22.0, None))
    },
    Sound {
        negative: true,
        seed: true,
        price: "$0.001 / second",
        ..effect("fal-ai/mmaudio-v2/text-to-audio", "MMAudio", "Open model for sound effects, up to 30 s.", Len::Secs("duration", 1.0, 30.0, Some(8.0)))
    },
    Sound {
        text: "text_prompt",
        seed: true,
        fixed: &[("num_samples", Fixed::Int(1))],
        price: "$0.01 / second",
        ..effect("mirelo-ai/sfx1.6/text-to-audio", "Mirelo SFX 1.6", "Sound design and ambiences up to a minute.", Len::Secs("duration", 0.1, 60.0, Some(10.0)))
    },
    Sound {
        negative: true,
        seed: true,
        price: "$0.02 / track",
        ..effect("fal-ai/stable-audio-3/small/sfx/text-to-audio", "Stable Audio 3 SFX", "Stability's sound-effect model, up to 2 minutes.", Len::Secs("duration", 1.0, 120.0, Some(8.0)))
    },
    Sound {
        negative: true,
        seed: true,
        price: "$0.10 / request",
        ..effect("beatoven/sound-effect-generation", "Beatoven SFX", "Royalty-free sound effects, up to 35 s.", Len::Secs("duration", 1.0, 35.0, Some(5.0)))
    },
];

pub(super) fn sound(id: &str) -> Option<&'static Sound> {
    find(SOUNDS, id)
}

/// Audio files in a result: `audio` (a file or a list), `audio_file`, `audio_url`.
pub(super) fn audio_urls(v: &Value) -> Vec<String> {
    let url = |f: &Value| f.get("url").and_then(Value::as_str).or_else(|| f.as_str()).map(str::to_string);
    for key in ["audio", "audio_file", "audio_url"] {
        match v.get(key) {
            Some(Value::Array(list)) => return list.iter().filter_map(url).collect(),
            Some(f) if !f.is_null() => {
                if let Some(u) = url(f) {
                    return vec![u];
                }
            }
            _ => {}
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::GenError;
    use serde_json::json;

    #[test]
    fn every_sound_is_described_and_unique() {
        for (i, s) in SOUNDS.iter().enumerate() {
            assert!(SOUNDS[..i].iter().all(|o| o.id != s.id), "{} twice", s.id);
            let m = model_info(super::ID, s);
            assert!(m.tasks == [s.task]);
            if s.task == Task::TextToSpeech {
                assert!(m.voices && m.default_voice.is_some(), "{}", s.id);
                assert!(voices_for(SOUNDS, s.id).iter().any(|v| Some(&v.id) == m.default_voice.as_ref()), "{}: default voice isn't listed", s.id);
            }
        }
        for task in [Task::TextToSpeech, Task::TextToMusic, Task::TextToSound] {
            assert_eq!(SOUNDS.iter().filter(|s| s.task == task && s.featured).count(), 1, "one featured {task:?}");
        }
    }

    #[test]
    fn inputs_follow_each_schema() {
        let mut r = GenRequest::new("fal-ai/minimax/speech-2.8-hd", Task::TextToSpeech, "Hello there");
        r.voice = Some("Calm_Woman".into());
        r.language = Some("fr-FR".into());
        let b = input_for(sound(&r.model).unwrap(), &r).unwrap();
        assert_eq!(b["prompt"], "Hello there");
        assert_eq!(b["voice_setting"], json!({"voice_id": "Calm_Woman"}));
        assert_eq!(b["language_boost"], "French");
        assert_eq!(b["output_format"], "url");

        let mut r = GenRequest::new("elevenlabs/music/v2.5", Task::TextToMusic, "lofi beat");
        r.duration = Some(1000.0);
        r.instrumental = Some(true);
        let b = input_for(sound(&r.model).unwrap(), &r).unwrap();
        assert_eq!((b["music_length_ms"].clone(), b["force_instrumental"].clone()), (json!(600_000), json!(true)));

        // Defaults stand in for lengths the model would otherwise make very long.
        let r = GenRequest::new("fal-ai/stable-audio-25/text-to-audio", Task::TextToMusic, "drums");
        assert_eq!(input_for(sound(&r.model).unwrap(), &r).unwrap()["seconds_total"], json!(30));
        // …but a model that decides by itself is left to.
        let r = GenRequest::new("fal-ai/elevenlabs/sound-effects/v2", Task::TextToSound, "door slam");
        assert!(!input_for(sound(&r.model).unwrap(), &r).unwrap().contains_key("duration_seconds"));

        let r = GenRequest::new("minimax/music-3", Task::TextToMusic, "a ballad");
        assert!(matches!(input_for(sound(&r.model).unwrap(), &r), Err(GenError::Unsupported(_))));
        let mut r = GenRequest::new("fal-ai/lyria3/pro", Task::TextToMusic, "a ballad");
        r.lyrics = Some("la la".into());
        assert!(input_for(sound(&r.model).unwrap(), &r).unwrap()["prompt"].as_str().unwrap().ends_with("Lyrics:\nla la"));
    }

    #[test]
    fn audio_outputs() {
        assert_eq!(audio_urls(&json!({"audio": {"url": "u1"}})), ["u1"]);
        assert_eq!(audio_urls(&json!({"audio": [{"url": "a"}, {"url": "b"}]})), ["a", "b"]);
        assert_eq!(audio_urls(&json!({"audio_file": {"url": "c"}})), ["c"]);
        assert!(audio_urls(&json!({"images": []})).is_empty());
    }
}
