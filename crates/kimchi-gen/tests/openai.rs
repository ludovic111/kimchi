//! OpenAI provider against a mock server.

use kimchi_gen::providers::openai::OpenAi;
use kimchi_gen::*;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("openai", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn image_data(n: usize) -> Value {
    json!({ "created": 1, "data": vec![json!({ "b64_json": util::b64(PNG) }); n] })
}

#[tokio::test]
async fn text_to_image_sends_json_and_decodes_base64() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .and(header("authorization", "Bearer test"))
        .and(body_partial_json(json!({
            "model": "gpt-image-2.5-sunburst",
            "n": 2,
            "size": "1360x768",
            "quality": "high",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(image_data(2)))
        .expect(1)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("gpt-image-2.5-sunburst", Task::TextToImage, "a red fox");
    req.aspect_ratio = Some("16:9".into());
    req.count = 2;
    req.params.insert("quality".into(), json!("high"));
    req.params.insert("background".into(), json!("auto")); // "auto" is left out
    let out = OpenAi.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 2);
    match &out.items[0].source {
        OutputSource::Bytes { data, mime } => assert_eq!((&data[..], mime.as_str()), (PNG, "image/png")),
        other => panic!("expected bytes, got {other:?}"),
    }
    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    assert!(sent.get("background").is_none());
}

#[tokio::test]
async fn references_go_to_edits_as_multipart() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/edits"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(image_data(1)))
        .expect(1)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("gpt-image-1.5", Task::ImageToImage, "make it night");
    req.aspect_ratio = Some("9:16".into());
    req.images = vec![
        InputImage::from_bytes(ImageRole::Reference, PNG, "image/png"),
        InputImage::from_bytes(ImageRole::Reference, PNG, "image/png"),
    ];
    let out = OpenAi.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 1);

    let r = &server.received_requests().await.unwrap()[0];
    let ct = r.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("multipart/form-data"));
    let body = String::from_utf8_lossy(&r.body);
    assert_eq!(body.matches("name=\"image[]\"").count(), 2);
    // GPT Image 1.x only has fixed sizes: 9:16 snaps to portrait.
    assert!(body.contains("1024x1536"));
    assert!(body.contains("make it night"));
}

#[tokio::test]
async fn moderation_block_is_reported_as_moderated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "type": "image_generation_user_error",
                "code": "moderation_blocked",
                "message": "Your request was rejected by the safety system (moderation_blocked).",
            }
        })))
        .mount(&server)
        .await;
    let req = GenRequest::new("gpt-image-2.5-flare", Task::TextToImage, "something");
    let err = OpenAi.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(_)), "{err:?}");
}

#[tokio::test]
async fn bad_key_is_unauthorized_and_video_is_unsupported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": { "message": "bad key" } })))
        .mount(&server)
        .await;
    let err = OpenAi.check(&ctx(&server)).await.unwrap_err();
    assert!(matches!(err, GenError::Unauthorized { status: 401, .. }), "{err:?}");

    let req = GenRequest::new("sora-2", Task::TextToVideo, "waves");
    assert!(matches!(OpenAi.generate(&ctx(&server), &req).await, Err(GenError::Unsupported(_))));
    assert!(!OpenAi.info().tasks.contains(&Task::TextToVideo));
}

#[tokio::test]
async fn check_counts_image_models_and_catalog_is_sane() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [{ "id": "gpt-image-2.5-sunburst" }, { "id": "gpt-image-2" }, { "id": "gpt-5" }],
        })))
        .mount(&server)
        .await;
    let status = OpenAi.check(&ctx(&server)).await.unwrap();
    assert!(status.contains("2 GPT Image"), "{status}");

    let models = OpenAi.models(&ctx(&server)).await.unwrap();
    let sunburst = models.iter().find(|m| m.id == "gpt-image-2.5-sunburst").unwrap();
    assert!(sunburst.featured && sunburst.supports(Task::ImageToImage));
    assert_eq!(sunburst.max_images, 16);
    assert!(models.iter().all(|m| m.tasks.iter().all(|t| t.output() == OutputKind::Image)));
}

/// Real API call. Run with `OPENAI_API_KEY=… cargo test -p kimchi-gen --test openai -- --ignored`.
#[tokio::test]
#[ignore = "hits the real OpenAI API and costs money"]
async fn live_smoke() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else { return };
    let cx = Ctx::new("openai", reqwest::Client::new(), OpenAi.info().default_base_url).with_key(Some(key));
    println!("{}", OpenAi.check(&cx).await.unwrap());
    let mut req = GenRequest::new("gpt-image-2.5-flare", Task::TextToImage, "a small kimchi jar, studio photo");
    req.params.insert("quality".into(), json!("low"));
    let out = OpenAi.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
