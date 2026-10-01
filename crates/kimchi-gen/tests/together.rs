//! Together AI provider against a mock server.

use kimchi_gen::providers::together::Together;
use kimchi_gen::*;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// Like the real default, the base ends in `/v1`; video lives under `/v2`.
fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("together", reqwest::Client::new(), format!("{}/v1", server.uri())).with_key(Some("test".into()))
}

fn images(n: usize) -> Value {
    json!({
        "id": "x",
        "model": "m",
        "object": "list",
        "data": (0..n).map(|i| json!({ "index": i, "b64_json": util::b64(PNG), "type": "b64_json" })).collect::<Vec<_>>(),
    })
}

#[tokio::test]
async fn flux2_uses_pixels_and_reference_images() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("authorization", "Bearer test"))
        .and(body_partial_json(json!({
            "model": "black-forest-labs/FLUX.2-pro",
            "response_format": "base64",
            "width": 1328,
            "height": 752,
            "seed": 42,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(images(4)))
        .expect(2)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("black-forest-labs/FLUX.2-pro", Task::ImageToImage, "same room at dusk");
    req.aspect_ratio = Some("16:9".into());
    req.seed = Some(42);
    req.count = 6; // four per call at most
    req.images = vec![InputImage::from_bytes(ImageRole::Reference, PNG, "image/png")];
    let out = Together.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 8); // the mock always answers with four

    let bodies: Vec<Value> = server.received_requests().await.unwrap().iter().map(|r| r.body_json().unwrap()).collect();
    let mut ns: Vec<u64> = bodies.iter().map(|b| b["n"].as_u64().unwrap()).collect();
    ns.sort();
    assert_eq!(ns, [2, 4]);
    assert!(bodies[0]["reference_images"][0].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert!(bodies[0].get("image_url").is_none());
}

#[tokio::test]
async fn kontext_uses_aspect_ratio_and_image_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(body_partial_json(
            json!({ "model": "black-forest-labs/FLUX.1-kontext-pro", "aspect_ratio": "9:16", "n": 1 }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(images(1)))
        .expect(1)
        .mount(&server)
        .await;
    let mut req = GenRequest::new("black-forest-labs/FLUX.1-kontext-pro", Task::ImageToImage, "make it a poster");
    req.width = Some(1080);
    req.height = Some(1920);
    req.images = vec![InputImage::from_bytes(ImageRole::Reference, PNG, "image/png")];
    Together.generate(&ctx(&server), &req).await.unwrap();

    let body: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    assert!(body["image_url"].as_str().unwrap().starts_with("data:image/png"));
    assert!(body.get("width").is_none());
}

#[tokio::test]
async fn nsfw_rejection_is_moderated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "error": { "message": "NSFW content detected in the generated image", "type": "invalid_request_error" }
        })))
        .mount(&server)
        .await;
    let req = GenRequest::new("Qwen/Qwen-Image-2.0", Task::TextToImage, "x");
    let err = Together.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(_)), "{err:?}");
}

#[tokio::test(start_paused = true)]
async fn video_job_on_v2_is_polled_until_completed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/videos"))
        .and(header("authorization", "Bearer test"))
        .and(body_partial_json(json!({
            "model": "Wan-AI/wan2.7-i2v",
            "seconds": "10",
            "resolution": "1080P",
            "negative_prompt": "text",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "vid_1", "status": "queued" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/videos/vid_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "vid_1", "status": "in_progress" })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/videos/vid_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "vid_1",
            "status": "completed",
            "outputs": { "cost": 0.1, "video_url": "https://cdn.together.ai/v/vid_1.mp4" },
        })))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("Wan-AI/wan2.7-i2v", Task::ImageToVideo, "leaves start to fall");
    req.duration = Some(9.0);
    req.resolution = Some("1080p".into());
    req.negative_prompt = Some("text".into());
    req.images = vec![
        InputImage::from_bytes(ImageRole::StartFrame, PNG, "image/png"),
        InputImage::from_bytes(ImageRole::EndFrame, PNG, "image/png"),
    ];
    let out = Together.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.cost_usd, Some(0.1));
    assert!(
        matches!(&out.items[0].source, OutputSource::Url { url, headers } if url.ends_with("vid_1.mp4") && headers.is_empty())
    );

    let body: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let frames = body["media"]["frame_images"].as_array().unwrap();
    assert_eq!(frames[0], json!({ "input_image": util::b64(PNG), "frame": "first" }));
    assert_eq!(frames[1]["frame"], "last");
    assert!(body.get("ratio").is_none(), "no explicit ratio: follow the start frame");
}

#[tokio::test]
async fn failed_video_surfaces_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/videos"))
        .and(body_partial_json(json!({ "width": 1280, "height": 720, "seconds": "8" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "v", "status": "queued" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/videos/v"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "v",
            "status": "failed",
            "outputs": null,
            "error": { "code": "invalidDuration", "message": "seconds must be 4, 6 or 8" },
        })))
        .mount(&server)
        .await;
    let mut req = GenRequest::new("google/veo-3.1", Task::TextToVideo, "x");
    req.duration = Some(10.0);
    let err = Together.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Provider(ref m) if m.contains("seconds must be")), "{err:?}");
}

#[tokio::test]
async fn models_merge_the_live_list_and_check_counts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "black-forest-labs/FLUX.2-pro", "type": "image", "display_name": "FLUX.2 [pro]" },
            { "id": "acme/new-image-model", "type": "image", "display_name": "Acme Image" },
            { "id": "acme/new-t2v", "type": "video" },
            { "id": "Wan-AI/wan2.7-videoedit", "type": "video" },
            { "id": "meta/llama", "type": "chat" },
        ])))
        .mount(&server)
        .await;
    let models = Together.models(&ctx(&server)).await.unwrap();
    assert_eq!(models.iter().filter(|m| m.id == "black-forest-labs/FLUX.2-pro").count(), 1);
    let acme = models.iter().find(|m| m.id == "acme/new-image-model").unwrap();
    assert_eq!((acme.name.as_str(), acme.tasks.as_slice()), ("Acme Image", &[Task::TextToImage][..]));
    assert_eq!(models.iter().find(|m| m.id == "acme/new-t2v").unwrap().tasks, [Task::TextToVideo]);
    assert!(!models.iter().any(|m| m.id.contains("videoedit") || m.id == "meta/llama"));

    assert_eq!(Together.check(&ctx(&server)).await.unwrap(), "Key works · 2 image and 2 video models");
}

/// Real API call. Run with `TOGETHER_API_KEY=… cargo test -p kimchi-gen --test together -- --ignored`.
#[tokio::test]
#[ignore = "hits the real Together API and costs money"]
async fn live_smoke() {
    let Ok(key) = std::env::var("TOGETHER_API_KEY") else { return };
    let cx = Ctx::new("together", reqwest::Client::new(), Together.info().default_base_url).with_key(Some(key));
    println!("{}", Together.check(&cx).await.unwrap());
    let req = GenRequest::new("black-forest-labs/FLUX.2-dev", Task::TextToImage, "a small kimchi jar, studio photo");
    let out = Together.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
