//! Google Gemini provider against a mock server.

use kimchi_gen::providers::google::Google;
use kimchi_gen::*;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const MP4: &[u8] = b"\0\0\0\x18ftypmp42\0\0\0\0";

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("google", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn key_header(item: &OutputItem) -> (&str, Option<&str>) {
    match &item.source {
        OutputSource::Url { url, headers } => {
            (url.as_str(), headers.iter().find(|(k, _)| k == "x-goog-api-key").map(|(_, v)| v.as_str()))
        }
        other => panic!("expected a URL, got {other:?}"),
    }
}

#[tokio::test]
async fn image_calls_generate_content_once_per_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/gemini-3.1-flash-image:generateContent"))
        .and(header("x-goog-api-key", "test"))
        .and(body_partial_json(json!({
            "generationConfig": {
                "responseModalities": ["TEXT", "IMAGE"],
                "imageConfig": { "aspectRatio": "4:1", "imageSize": "2K" },
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [{
                "finishReason": "STOP",
                "content": { "parts": [
                    { "text": "thinking", "thought": true },
                    { "inlineData": { "mimeType": "image/png", "data": util::b64(b"draft") }, "thought": true },
                    { "text": "Here you go" },
                    { "inlineData": { "mimeType": "image/png", "data": util::b64(PNG) } },
                ]}
            }]
        })))
        .expect(2)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("gemini-3.1-flash-image", Task::ImageToImage, "a panorama of this");
    req.aspect_ratio = Some("4:1".into());
    req.resolution = Some("2k".into());
    req.count = 2;
    req.images = vec![InputImage::from_bytes(ImageRole::Reference, PNG, "image/png")];
    let out = Google.generate(&ctx(&server), &req).await.unwrap();

    // Thought images are skipped: one real image per call.
    assert_eq!(out.items.len(), 2);
    assert!(matches!(&out.items[0].source, OutputSource::Bytes { data, .. } if &data[..] == PNG));
    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let parts = sent["contents"][0]["parts"].as_array().unwrap();
    assert_eq!(parts[0]["inlineData"]["data"], util::b64(PNG));
    assert_eq!(parts[1]["text"], "a panorama of this");
}

#[tokio::test]
async fn blocked_prompt_is_moderated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/gemini-3-pro-image:generateContent"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "promptFeedback": { "blockReason": "SAFETY" } })),
        )
        .mount(&server)
        .await;
    let req = GenRequest::new("gemini-3-pro-image", Task::TextToImage, "x");
    let err = Google.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(_)), "{err:?}");

    // Same for an image-safety finish reason with no image.
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/models/gemini-3-pro-image:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [{ "finishReason": "IMAGE_SAFETY", "content": { "parts": [] } }]
        })))
        .mount(&server)
        .await;
    let err = Google.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(_)), "{err:?}");
}

#[tokio::test(start_paused = true)]
async fn veo_polls_the_operation_and_downloads_with_the_key() {
    let server = MockServer::start().await;
    let model = "veo-3.1-fast-generate-preview";
    Mock::given(method("POST"))
        .and(path(format!("/models/{model}:predictLongRunning")))
        .and(header("x-goog-api-key", "test"))
        .and(body_partial_json(json!({
            "parameters": { "aspectRatio": "9:16", "durationSeconds": 8, "resolution": "1080p", "negativePrompt": "blur" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "name": format!("models/{model}/operations/op1") })))
        .expect(1)
        .mount(&server)
        .await;
    let op_path = format!("/models/{model}/operations/op1");
    Mock::given(method("GET"))
        .and(path(op_path.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "name": "op1", "done": false })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let video_uri = format!("{}/files/vid1:download?alt=media", server.uri());
    Mock::given(method("GET"))
        .and(path(op_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "op1",
            "done": true,
            "response": { "generateVideoResponse": { "generatedSamples": [{ "video": { "uri": video_uri } }] } },
        })))
        .mount(&server)
        .await;

    let mut req = GenRequest::new(model, Task::ImageToVideo, "the flower blooms");
    req.aspect_ratio = Some("9:16".into());
    req.resolution = Some("1080p".into());
    req.duration = Some(4.0); // 1080p forces 8 s
    req.negative_prompt = Some("blur".into());
    req.images = vec![
        InputImage::from_bytes(ImageRole::StartFrame, PNG, "image/png"),
        InputImage::from_bytes(ImageRole::EndFrame, PNG, "image/png"),
    ];
    let out = Google.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 1);
    assert_eq!(key_header(&out.items[0]), (video_uri.as_str(), Some("test")));

    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let instance = &sent["instances"][0];
    assert_eq!(instance["image"]["inlineData"]["mimeType"], "image/png");
    assert!(instance["lastFrame"]["inlineData"]["data"].is_string());
}

#[tokio::test]
async fn veo_filtered_output_is_moderated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/veo-3.1-generate-preview:predictLongRunning"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "name": "operations/x" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/operations/x"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "done": true,
            "response": { "generateVideoResponse": {
                "raiMediaFilteredCount": 1,
                "raiMediaFilteredReasons": ["The video was filtered for safety."],
            }},
        })))
        .mount(&server)
        .await;
    let req = GenRequest::new("veo-3.1-generate-preview", Task::TextToVideo, "x");
    let err = Google.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(ref m) if m.contains("filtered")), "{err:?}");
}

#[tokio::test(start_paused = true)]
async fn omni_waits_for_the_file_then_returns_a_keyed_download() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/interactions"))
        .and(header("x-goog-api-key", "test"))
        .and(body_partial_json(json!({
            "model": "gemini-omni-1.1-flash",
            "response_format": { "type": "video", "aspect_ratio": "16:9", "resolution": "4k", "delivery": "uri" },
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "v1_abc",
            "status": "completed",
            "steps": [
                { "type": "user_input", "content": [{ "type": "text", "text": "..." }] },
                { "type": "model_output", "content": [{ "type": "video", "mime_type": "video/mp4", "uri": "files/abc123" }] },
            ],
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files/abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "PROCESSING" })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files/abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "ACTIVE" })))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("gemini-omni-1.1-flash", Task::ImageToVideo, "it starts to rain");
    req.resolution = Some("4K".into());
    req.images = vec![
        InputImage::from_bytes(ImageRole::StartFrame, PNG, "image/png"),
        InputImage::from_bytes(ImageRole::Reference, PNG, "image/png"),
    ];
    let out = Google.generate(&ctx(&server), &req).await.unwrap();
    let expected = format!("{}/files/abc123:download?alt=media", server.uri());
    assert_eq!(key_header(&out.items[0]), (expected.as_str(), Some("test")));

    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let input = sent["input"].as_array().unwrap();
    assert_eq!(input.len(), 3); // start frame, reference, prompt
    assert_eq!(input[0]["type"], "image");
    assert_eq!(input[2], json!({ "type": "text", "text": "it starts to rain" }));
}

#[tokio::test]
async fn omni_inline_video_is_returned_as_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "completed",
            "steps": [{ "type": "model_output", "content": [
                { "type": "video", "mime_type": "video/mp4", "data": util::b64(MP4) }
            ]}],
        })))
        .mount(&server)
        .await;
    let req = GenRequest::new("gemini-omni-1.1-flash", Task::TextToVideo, "waves");
    let out = Google.generate(&ctx(&server), &req).await.unwrap();
    assert!(
        matches!(&out.items[0].source, OutputSource::Bytes { mime, data } if mime == "video/mp4" && &data[..] == MP4)
    );
}

#[tokio::test]
async fn check_lists_models_with_the_key_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("x-goog-api-key", "test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": [{ "name": "models/gemini-3.1-flash-image" }, { "name": "models/gemini-omni-1.1-flash" }]
        })))
        .mount(&server)
        .await;
    assert!(Google.check(&ctx(&server)).await.unwrap().contains("2 image/video"));
    let models = Google.models(&ctx(&server)).await.unwrap();
    assert!(models.iter().all(|m| !m.id.starts_with("imagen")));
    assert!(models.iter().any(|m| m.id == "gemini-omni-1.1-flash" && m.featured));
}

/// Real API call. Run with `GEMINI_API_KEY=… cargo test -p kimchi-gen --test google -- --ignored`.
#[tokio::test]
#[ignore = "hits the real Gemini API and costs money"]
async fn live_smoke() {
    let Ok(key) = std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY")) else { return };
    let cx = Ctx::new("google", reqwest::Client::new(), Google.info().default_base_url).with_key(Some(key));
    println!("{}", Google.check(&cx).await.unwrap());
    let req = GenRequest::new("gemini-3.1-flash-lite-image", Task::TextToImage, "a small kimchi jar, studio photo");
    let out = Google.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
