//! xAI provider against a mock server.

use std::sync::{Arc, Mutex};

use kimchi_gen::providers::xai::Xai;
use kimchi_gen::*;
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0\0\x10JFIF";

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("xai", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

#[tokio::test]
async fn text_to_image_maps_ratio_and_reports_cost() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .and(header("authorization", "Bearer test"))
        .and(body_partial_json(json!({
            "model": "grok-imagine-image-2.0",
            "n": 2,
            "response_format": "b64_json",
            "aspect_ratio": "21:9",
            "resolution": "2k",
            "quality": "medium",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                { "b64_json": util::b64(JPEG), "mime_type": "image/jpeg" },
                { "b64_json": util::b64(JPEG), "mime_type": "image/jpeg" },
            ],
            "usage": { "cost_in_usd_ticks": 800_000_000u64 },
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("grok-imagine-image-2.0", Task::TextToImage, "a neon street");
    req.width = Some(2560);
    req.height = Some(1080); // 64:27 → closest supported is 21:9
    req.resolution = Some("2K".into());
    req.count = 2;
    req.params.insert("quality".into(), json!("medium"));
    let out = Xai.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 2);
    assert!(matches!(&out.items[0].source, OutputSource::Bytes { mime, .. } if mime == "image/jpeg"));
    assert!((out.cost_usd.unwrap() - 0.08).abs() < 1e-9);
}

#[tokio::test]
async fn several_references_use_the_edits_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "b64_json": util::b64(PNG) }] })))
        .expect(1)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("grok-imagine-image", Task::ImageToImage, "<IMAGE_0> wearing <IMAGE_1>");
    req.images = vec![
        InputImage::from_bytes(ImageRole::Reference, PNG, "image/png"),
        InputImage::from_bytes(ImageRole::Reference, JPEG, "image/jpeg"),
    ];
    Xai.generate(&ctx(&server), &req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    let images = sent["images"].as_array().unwrap();
    assert_eq!(images.len(), 2);
    assert!(images[1]["url"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
    assert!(sent.get("image").is_none());
}

#[tokio::test(start_paused = true)]
async fn image_to_video_polls_until_done() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/generations"))
        .and(header("authorization", "Bearer test"))
        .and(body_partial_json(json!({
            "model": "grok-imagine-video-1.5",
            "duration": 7,
            "resolution": "1080p",
            "generate_audio": false,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "request_id": "req-1" })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/req-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "pending", "progress": 40 })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/req-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "done",
            "progress": 100,
            "video": { "url": "https://vidgen.x.ai/out.mp4", "duration": 7, "respect_moderation": true },
            "usage": { "cost_in_usd_ticks": 5_600_000_000u64 },
        })))
        .mount(&server)
        .await;

    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let cx = ctx(&server).with_progress(move |p| sink.lock().unwrap().push(p));
    let mut req = GenRequest::new("grok-imagine-video-1.5", Task::ImageToVideo, "she turns around");
    req.images = vec![
        InputImage::from_bytes(ImageRole::StartFrame, PNG, "image/png"),
        InputImage::from_bytes(ImageRole::EndFrame, JPEG, "image/jpeg"),
    ];
    req.duration = Some(7.0);
    req.resolution = Some("1080p".into());
    req.audio = Some(false);
    let out = Xai.generate(&cx, &req).await.unwrap();

    assert_eq!(out.items.len(), 1);
    assert!(matches!(&out.items[0].source, OutputSource::Url { url, .. } if url == "https://vidgen.x.ai/out.mp4"));
    assert!((out.cost_usd.unwrap() - 0.56).abs() < 1e-9);
    let fractions: Vec<f64> = seen.lock().unwrap().iter().filter_map(|p| p.fraction).collect();
    assert!(fractions.iter().any(|f| (0.3..0.5).contains(f)), "{fractions:?}");

    let sent: Value = server.received_requests().await.unwrap()[0].body_json().unwrap();
    assert!(sent["image"]["url"].as_str().unwrap().starts_with("data:image/png"));
    assert!(sent["last_frame"]["url"].as_str().unwrap().starts_with("data:image/jpeg"));
    // No explicit ratio: the video follows the start frame.
    assert!(sent.get("aspect_ratio").is_none());
}

#[tokio::test]
async fn filtered_video_is_moderated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/videos/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "request_id": "r" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos/r"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "done",
            "video": { "url": "", "respect_moderation": false },
        })))
        .mount(&server)
        .await;
    let req = GenRequest::new("grok-imagine-video", Task::TextToVideo, "x");
    let err = Xai.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(_)), "{err:?}");
}

#[tokio::test]
async fn check_reads_the_key_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api-key"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "kimchi",
            "api_key_blocked": false,
            "api_key_disabled": false,
            "team_blocked": false,
        })))
        .mount(&server)
        .await;
    assert!(Xai.check(&ctx(&server)).await.unwrap().contains("kimchi"));

    let models = Xai.models(&ctx(&server)).await.unwrap();
    let video = models.iter().find(|m| m.id == "grok-imagine-video-1.5").unwrap();
    assert!(video.end_frame && video.audio && video.resolutions.contains(&"1080p".to_string()));
}

/// Real API call. Run with `XAI_API_KEY=… cargo test -p kimchi-gen --test xai -- --ignored`.
#[tokio::test]
#[ignore = "hits the real xAI API and costs money"]
async fn live_smoke() {
    let Ok(key) = std::env::var("XAI_API_KEY") else { return };
    let cx = Ctx::new("xai", reqwest::Client::new(), Xai.info().default_base_url).with_key(Some(key));
    println!("{}", Xai.check(&cx).await.unwrap());
    let req = GenRequest::new("grok-imagine-image", Task::TextToImage, "a small kimchi jar, studio photo");
    let out = Xai.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
