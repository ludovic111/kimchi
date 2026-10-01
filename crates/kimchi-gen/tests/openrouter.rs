//! OpenRouter provider against a mock server.

use kimchi_gen::providers::openrouter::OpenRouter;
use kimchi_gen::*;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("openrouter", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn image_models() -> Value {
    json!({ "data": [
        {
            "id": "recraft/recraft-v4-vector",
            "name": "Recraft: V4 Vector",
            "supported_parameters": { "output_format": { "type": "enum", "values": ["svg"] } },
        },
        {
            "id": "bytedance-seed/seedream-4.5",
            "name": "ByteDance Seed: Seedream 4.5",
            "description": "Seedream.",
            "supported_parameters": {
                "resolution": { "type": "enum", "values": ["1K", "2K", "4K"] },
                "aspect_ratio": { "type": "enum", "values": ["1:1", "16:9", "9:16", "auto"] },
                "n": { "type": "range", "min": 1, "max": 3 },
                "input_references": { "type": "range", "min": 0, "max": 14 },
                "seed": { "type": "boolean" },
            },
        },
        {
            "id": "inclusionai/layers",
            "name": "inclusionAI: Layers",
            "supported_parameters": { "input_references": { "type": "range", "min": 1, "max": 1 } },
        },
    ]})
}

fn video_models() -> Value {
    json!({ "data": [
        {
            "id": "google/veo-3.1",
            "name": "Google: Veo 3.1",
            "supported_durations": [4, 6, 8],
            "supported_resolutions": ["720p", "1080p", "4K"],
            "supported_aspect_ratios": ["16:9", "9:16"],
            "supported_frame_images": ["first_frame", "last_frame"],
            "generate_audio": true,
            "seed": true,
            "pricing_skus": { "duration_seconds_with_audio": "0.40", "duration_seconds_without_audio": "0.20" },
        },
        {
            "id": "runway/aleph-2",
            "name": "Runway: Aleph 2",
            "supported_durations": null,
            "supported_aspect_ratios": ["16:9"],
        },
    ]})
}

async fn mount_listings(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/images/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(image_models()))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(video_models()))
        .mount(server)
        .await;
}

#[tokio::test]
async fn models_come_from_the_listings() {
    let server = MockServer::start().await;
    mount_listings(&server).await;
    let models = OpenRouter.models(&ctx(&server)).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    // Featured first; SVG-only and video-input-only models are dropped.
    assert_eq!(ids, ["bytedance-seed/seedream-4.5", "google/veo-3.1", "inclusionai/layers"]);

    let seedream = &models[0];
    assert_eq!(seedream.name, "Seedream 4.5");
    assert_eq!(seedream.tasks, [Task::TextToImage, Task::ImageToImage]);
    assert_eq!((seedream.max_outputs, seedream.max_images, seedream.seed), (3, 14, true));
    assert_eq!(seedream.aspect_ratios, ["1:1", "16:9", "9:16"]);
    assert_eq!(seedream.resolutions, ["1K", "2K", "4K"]);

    let veo = &models[1];
    assert_eq!(veo.tasks, [Task::TextToVideo, Task::ImageToVideo]);
    assert_eq!(veo.durations, [4.0, 6.0, 8.0]);
    assert!(veo.end_frame && veo.audio && veo.seed);
    assert_eq!(veo.price.as_deref(), Some("$0.20–0.40 / s"));

    // An image model that requires an input image is edit-only.
    assert_eq!(models[2].tasks, [Task::ImageToImage]);
}

#[tokio::test]
async fn models_fall_back_to_built_ins_offline() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    let models = OpenRouter.models(&ctx(&server)).await.unwrap();
    assert!(models.len() >= 8);
    assert!(models[0].featured);
    assert!(models.iter().any(|m| m.supports(Task::ImageToVideo)));
}

#[tokio::test]
async fn image_request_is_snapped_to_the_model_and_batched() {
    let server = MockServer::start().await;
    mount_listings(&server).await;
    Mock::given(method("POST"))
        .and(path("/images"))
        .and(header("authorization", "Bearer test"))
        .and(body_partial_json(json!({
            "model": "bytedance-seed/seedream-4.5",
            "aspect_ratio": "16:9",
            "resolution": "2K",
            "seed": 7,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "created": 1,
            "data": [{ "b64_json": util::b64(PNG), "media_type": "image/png" }],
            "usage": { "cost": 0.04 },
        })))
        .expect(2)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("bytedance-seed/seedream-4.5", Task::ImageToImage, "the same cat, in snow");
    req.width = Some(1920);
    req.height = Some(1200); // 16:10 → 16:9
    req.resolution = Some("2k".into());
    req.seed = Some(7);
    req.count = 4; // the model allows 3 per call → two calls (3 + 1)
    req.images = vec![InputImage::from_bytes(ImageRole::Reference, PNG, "image/png")];
    let out = OpenRouter.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 2); // the mock returns one image per call
    assert!((out.cost_usd.unwrap() - 0.08).abs() < 1e-9);

    let posts: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .map(|r| r.body_json().unwrap())
        .collect();
    let mut ns: Vec<u64> = posts.iter().map(|b| b["n"].as_u64().unwrap()).collect();
    ns.sort();
    assert_eq!(ns, [1, 3]);
    let reference = &posts[0]["input_references"][0];
    assert_eq!(reference["type"], "image_url");
    assert!(reference["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn forbidden_is_moderation_only_when_flagged() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {
                "code": 403,
                "message": "Input was flagged",
                "metadata": { "reasons": ["violence"], "flagged_input": "…", "provider_name": "x" },
            }
        })))
        .mount(&server)
        .await;
    let req = GenRequest::new("unknown/model", Task::TextToImage, "x");
    let err = OpenRouter.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(ref m) if m.contains("violence")), "{err:?}");

    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/images"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({ "error": { "code": 403, "message": "Key disabled" } })),
        )
        .mount(&server)
        .await;
    let err = OpenRouter.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Unauthorized { status: 403, .. }), "{err:?}");
}

#[tokio::test(start_paused = true)]
async fn video_job_is_polled_and_downloaded_with_the_key() {
    let server = MockServer::start().await;
    mount_listings(&server).await;
    Mock::given(method("POST"))
        .and(path("/videos"))
        .and(header("authorization", "Bearer test"))
        .and(body_partial_json(json!({
            "model": "google/veo-3.1",
            "duration": 6,
            "resolution": "4K",
            "generate_audio": true,
        })))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({
            "id": "job1",
            "polling_url": "https://openrouter.ai/api/v1/videos/job1",
            "status": "pending",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/job1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "job1", "status": "in_progress" })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/job1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "job1",
            "status": "completed",
            "unsigned_urls": ["https://openrouter.ai/api/v1/videos/job1/content?index=0"],
            "usage": { "cost": 3.2, "is_byok": false },
        })))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("google/veo-3.1", Task::ImageToVideo, "camera pushes in");
    req.duration = Some(7.0); // between 6 and 8 → first closest
    req.resolution = Some("2160p".into()); // → 4K
    req.audio = Some(true);
    req.images = vec![
        InputImage::from_bytes(ImageRole::StartFrame, PNG, "image/png"),
        InputImage::from_bytes(ImageRole::EndFrame, PNG, "image/png"),
    ];
    let out = OpenRouter.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.cost_usd, Some(3.2));
    match &out.items[0].source {
        OutputSource::Url { url, headers } => {
            assert_eq!(url, &format!("{}/videos/job1/content?index=0", server.uri()));
            assert_eq!(headers, &[("Authorization".to_string(), "Bearer test".to_string())]);
        }
        other => panic!("expected a URL, got {other:?}"),
    }

    let post = server.received_requests().await.unwrap().into_iter().find(|r| r.method.as_str() == "POST").unwrap();
    let body: Value = post.body_json().unwrap();
    let frames = body["frame_images"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["frame_type"], "last_frame");
    assert!(body.get("aspect_ratio").is_none(), "image-to-video follows the frame by default");
}

#[tokio::test]
async fn failed_video_reports_the_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "j", "status": "pending" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/j"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": "j", "status": "failed", "error": "Prompt rejected by safety filter" })),
        )
        .mount(&server)
        .await;
    let req = GenRequest::new("some/video", Task::TextToVideo, "x");
    let err = OpenRouter.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(_)), "{err:?}");
}

#[tokio::test]
async fn check_reads_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "label": "kimchi", "usage": 1.5, "limit": 10.0, "limit_remaining": 8.5 }
        })))
        .mount(&server)
        .await;
    assert_eq!(OpenRouter.check(&ctx(&server)).await.unwrap(), "Key “kimchi” works · $8.50 left");
}

/// Real API call. Run with `OPENROUTER_API_KEY=… cargo test -p kimchi-gen --test openrouter -- --ignored`.
#[tokio::test]
#[ignore = "hits the real OpenRouter API and costs money"]
async fn live_smoke() {
    let Ok(key) = std::env::var("OPENROUTER_API_KEY") else { return };
    let cx = Ctx::new("openrouter", reqwest::Client::new(), OpenRouter.info().default_base_url).with_key(Some(key));
    println!("{}", OpenRouter.check(&cx).await.unwrap());
    let models = OpenRouter.models(&cx).await.unwrap();
    println!("{} models", models.len());
    let req = GenRequest::new("google/gemini-3.1-flash-image", Task::TextToImage, "a small kimchi jar, studio photo");
    let out = OpenRouter.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
