use kimchi_gen::{providers::{elevenlabs::ElevenLabs, stability::Stability}, *};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, header}};

fn ctx(server: &MockServer, provider: &str) -> Ctx {
    Ctx::new(provider, reqwest::Client::new(), server.uri()).with_key(Some("test-key".into()))
}

#[tokio::test]
async fn speech_discovers_account_models_and_voices_and_uses_them() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1/models")).respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"model_id":"test_speech","name":"Test speech","can_do_text_to_speech":true},
        {"model_id":"transcription","name":"Transcription","can_do_text_to_speech":false}
    ]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1/voices")).respond_with(ResponseTemplate::new(200)
        .set_body_json(json!({"voices":[{"voice_id":"testVoice","name":"Test voice"}]}))).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/text-to-speech/testVoice")).and(header("xi-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ID3audio".to_vec())).expect(1).mount(&server).await;
    let cx = ctx(&server, "elevenlabs");
    let models = ElevenLabs.models(&cx).await.unwrap();
    let speech = models.iter().find(|m| m.id == "test_speech").unwrap();
    assert_eq!(speech.tasks, [Task::TextToSpeech]);
    assert!(!models.iter().any(|m| m.id == "transcription"));
    let mut request = GenRequest::new(&speech.id, Task::TextToSpeech, "A new scene begins.");
    request.params.insert("voice_id".into(), speech.params[0].default.clone());
    let result = ElevenLabs.generate(&cx, &request).await.unwrap();
    assert_eq!(result.items[0].kind, OutputKind::Audio);
    assert!(matches!(&result.items[0].source, OutputSource::Bytes { mime, .. } if mime == "audio/mpeg"));
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    assert_eq!(body, json!({"text":"A new scene begins.","model_id":"test_speech"}));
}

#[tokio::test]
async fn sound_effects_and_music_send_distinct_duration_units() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"ID3audio".to_vec())).mount(&server).await;
    let cx = ctx(&server, "elevenlabs");
    let mut effect = GenRequest::new("eleven_text_to_sound_v2", Task::TextToAudio, "A gentle wave");
    effect.duration = Some(7.5);
    effect.params.insert("loop".into(), json!(true));
    ElevenLabs.generate(&cx, &effect).await.unwrap();
    let mut music = GenRequest::new("music_v1", Task::TextToAudio, "A quiet piano score");
    music.duration = Some(32.);
    ElevenLabs.generate(&cx, &music).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), "/v1/sound-generation");
    let effect_body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(effect_body["duration_seconds"], 7.5);
    assert_eq!(effect_body["loop"], true);
    assert_eq!(requests[1].url.path(), "/v1/music");
    let music_body: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(music_body["music_length_ms"], 32000);
    assert_eq!(music_body["force_instrumental"], true);
    effect.duration = Some(31.);
    assert!(ElevenLabs.generate(&cx, &effect).await.is_err());
    let speech = GenRequest::new("test", Task::TextToSpeech, "Hello");
    assert!(ElevenLabs.generate(&cx, &speech).await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 2, "invalid requests never spend credits");
}

#[tokio::test]
async fn stable_audio_sends_multipart_audio_and_reports_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v2beta/audio/stable-audio-2/text-to-audio"))
        .and(header("authorization", "Bearer test-key")).and(header("accept", "audio/*"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RIFF....WAVEdata".to_vec())).expect(1).mount(&server).await;
    let cx = ctx(&server, "stability");
    let mut request = GenRequest::new("stable-audio-2.5", Task::TextToAudio, "Forest ambience");
    request.duration = Some(60.); request.seed = Some(42);
    let output = Stability.generate(&cx, &request).await.unwrap();
    assert_eq!(output.items[0].kind, OutputKind::Audio);
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body);
    for value in ["Forest ambience", "stable-audio-2.5", "name=\"duration\"\r\n\r\n60", "name=\"seed\"\r\n\r\n42"] { assert!(body.contains(value), "{body}"); }
    request.duration = Some(191.);
    assert!(Stability.generate(&cx, &request).await.is_err());
}
