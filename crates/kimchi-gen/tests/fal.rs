use std::sync::{Arc, Mutex};

use kimchi_gen::providers::fal::Fal;
use kimchi_gen::{Ctx, GenError, GenRequest, ImageRole, InputImage, OutputKind, OutputSource, Progress, Provider, Task};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("fal", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

/// Mounts submit → status (queued, running, done) → result for `endpoint`,
/// with status/result living under the two-segment app id.
async fn mount_queue(server: &MockServer, endpoint: &str, app: &str, result: Value) {
    let base = format!("{}/{app}/requests/r1", server.uri());
    Mock::given(method("POST"))
        .and(path(format!("/{endpoint}")))
        .and(header("authorization", "Key test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "r1",
            "status_url": format!("{base}/status"),
            "response_url": base,
            "queue_position": 2
        })))
        .expect(1)
        .mount(server)
        .await;
    let status = format!("/{app}/requests/r1/status");
    Mock::given(method("GET"))
        .and(path(status.clone()))
        .and(query_param("logs", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "IN_QUEUE", "queue_position": 2})))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(status.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "IN_PROGRESS", "logs": [{"message": " 40%|████      | 12/30"}]
        })))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(status))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "COMPLETED", "response_url": base})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/{app}/requests/r1")))
        .and(header("authorization", "Key test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(result))
        .mount(server)
        .await;
}

#[tokio::test(start_paused = true)]
async fn image_edit_runs_through_the_queue() {
    let server = MockServer::start().await;
    mount_queue(
        &server,
        "fal-ai/nano-banana-pro/edit",
        "fal-ai/nano-banana-pro",
        json!({"images": [{"url": "https://cdn/a.png"}, {"url": "https://cdn/b.png"}], "description": "ok"}),
    )
    .await;

    let seen = Arc::new(Mutex::new(Vec::<Progress>::new()));
    let sink = seen.clone();
    let cx = ctx(&server).with_progress(move |p| sink.lock().unwrap().push(p));
    let mut req = GenRequest::new("fal-ai/nano-banana-pro", Task::ImageToImage, "make it snowy");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG1"[..], "image/png"));
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG2"[..], "image/png"));
    req.aspect_ratio = Some("1920:1080".into());
    req.resolution = Some("2k".into());
    req.count = 2;
    let out = Fal.generate(&cx, &req).await.unwrap();

    assert_eq!(out.items.len(), 2);
    assert!(matches!(&out.items[1].source, OutputSource::Url { url, .. } if url == "https://cdn/b.png"));
    let reqs = server.received_requests().await.unwrap();
    let b = body(&reqs[0]);
    assert_eq!(b["image_urls"].as_array().unwrap().len(), 2);
    assert!(b["image_urls"][0].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert_eq!(b["aspect_ratio"], "16:9");
    assert_eq!(b["resolution"], "2K");
    assert_eq!(b["num_images"], 2);
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|p| p.message.as_deref() == Some("In queue (2 ahead)")));
    assert!(seen.iter().any(|p| p.fraction.is_some_and(|f| (f - 0.41).abs() < 0.01)));
}

#[tokio::test(start_paused = true)]
async fn video_with_end_frame_uses_first_last_endpoint() {
    let server = MockServer::start().await;
    mount_queue(
        &server,
        "fal-ai/veo3.1/first-last-frame-to-video",
        "fal-ai/veo3.1",
        json!({"video": {"url": "https://cdn/v.mp4", "content_type": "video/mp4"}}),
    )
    .await;

    let mut req = GenRequest::new("fal-ai/veo3.1", Task::ImageToVideo, "a timelapse");
    req.images.push(InputImage::from_bytes(ImageRole::StartFrame, &b"\x89PNGs"[..], "image/png"));
    req.images.push(InputImage::from_bytes(ImageRole::EndFrame, &b"\xFF\xD8\xFFe"[..], "image/jpeg"));
    req.duration = Some(7.0);
    req.audio = Some(false);
    req.seed = Some(3);
    req.negative_prompt = Some("text".into());
    let out = Fal.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].kind, OutputKind::Video);
    let b = body(&server.received_requests().await.unwrap()[0]);
    assert!(b["first_frame_url"].as_str().unwrap().starts_with("data:image/png"));
    assert!(b["last_frame_url"].as_str().unwrap().starts_with("data:image/jpeg"));
    assert_eq!(b["duration"], "6s");
    assert_eq!(b["generate_audio"], false);
    assert_eq!(b["seed"], 3);
    assert_eq!(b["negative_prompt"], "text");
}

#[tokio::test(start_paused = true)]
async fn per_model_field_mapping() {
    let server = MockServer::start().await;
    mount_queue(&server, "fal-ai/kling-video/v3/pro/image-to-video", "fal-ai/kling-video", json!({"video": {"url": "https://cdn/k.mp4"}}))
        .await;
    let mut req = GenRequest::new("fal-ai/kling-video/v3/pro/text-to-video", Task::ImageToVideo, "go");
    req.images.push(InputImage::from_bytes(ImageRole::StartFrame, &b"\x89PNG"[..], "image/png"));
    req.duration = Some(12.4);
    req.seed = Some(1);
    Fal.generate(&ctx(&server), &req).await.unwrap();
    let b = body(&server.received_requests().await.unwrap()[0]);
    assert!(b["start_image_url"].is_string());
    assert_eq!(b["duration"], "12");
    assert!(b.get("seed").is_none(), "Kling has no seed");
    assert!(b.get("aspect_ratio").is_none(), "i2v follows the image");
}

#[tokio::test(start_paused = true)]
async fn flux2_uses_pixel_size_and_loops_for_count() {
    let server = MockServer::start().await;
    let base = format!("{}/fal-ai/flux-2-pro/requests/r1", server.uri());
    Mock::given(method("POST"))
        .and(path("/fal-ai/flux-2-pro"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"request_id": "r1", "status_url": format!("{base}/status"), "response_url": base})))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fal-ai/flux-2-pro/requests/r1/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "COMPLETED"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fal-ai/flux-2-pro/requests/r1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"images": [{"url": "https://cdn/x.jpg"}], "seed": 99})))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("fal-ai/flux-2-pro", Task::TextToImage, "a fox");
    req.width = Some(1080);
    req.height = Some(1920);
    req.count = 2;
    req.seed = Some(10);
    let out = Fal.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 2);
    assert_eq!(out.seed, Some(99));
    let posts: Vec<Value> = server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "POST").map(body).collect();
    assert_eq!(posts[0]["image_size"], json!({"width": 752, "height": 1328}));
    assert!(posts[0].get("num_images").is_none());
    assert_eq!((posts[0]["seed"].as_i64(), posts[1]["seed"].as_i64()), (Some(10), Some(11)));
}

#[tokio::test(start_paused = true)]
async fn failures_and_moderation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fal-ai/veo3.1"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "detail": [{"loc": ["body", "prompt"], "msg": "The prompt could not be processed.", "type": "content_policy_violation"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/fal-ai/flux-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"request_id": "z"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fal-ai/flux-2/requests/z/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "COMPLETED", "error": "Runner crashed", "error_type": "runner_server_error"})))
        .mount(&server)
        .await;
    let cx = ctx(&server);

    let e = Fal.generate(&cx, &GenRequest::new("fal-ai/veo3.1", Task::TextToVideo, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Moderated(ref m) if m.contains("could not be processed")), "{e:?}");
    let e = Fal.generate(&cx, &GenRequest::new("fal-ai/flux-2", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Provider(ref m) if m.contains("Runner crashed")), "{e:?}");
}

#[tokio::test(start_paused = true)]
async fn nsfw_flagged_images_are_dropped() {
    let server = MockServer::start().await;
    mount_queue(
        &server,
        "fal-ai/flux-2",
        "fal-ai/flux-2",
        json!({"images": [{"url": "https://cdn/black.png"}], "has_nsfw_concepts": [true], "seed": 1}),
    )
    .await;
    let e = Fal.generate(&ctx(&server), &GenRequest::new("fal-ai/flux-2", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Moderated(_)), "{e:?}");
}

#[tokio::test(start_paused = true)]
async fn large_images_are_uploaded_to_storage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/storage/upload/initiate"))
        .and(query_param("storage_type", "fal-cdn-v3"))
        .and(header("authorization", "Key test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "upload_url": format!("{}/put/abc", server.uri()), "file_url": "https://v3.fal.media/files/abc.png"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT")).and(path("/put/abc")).respond_with(ResponseTemplate::new(200)).expect(1).mount(&server).await;
    mount_queue(&server, "fal-ai/flux-pro/kontext", "fal-ai/flux-pro", json!({"images": [{"url": "https://cdn/k.png"}]})).await;

    let mut big = b"\x89PNG".to_vec();
    big.resize(2_000_000, 0);
    let mut req = GenRequest::new("fal-ai/flux-pro/kontext", Task::ImageToImage, "x");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, big, "image/png"));
    Fal.generate(&ctx(&server), &req).await.unwrap();
    let submit = server.received_requests().await.unwrap().into_iter().find(|r| r.url.path() == "/fal-ai/flux-pro/kontext").unwrap();
    assert_eq!(body(&submit)["image_url"], "https://v3.fal.media/files/abc.png");
}

#[tokio::test(start_paused = true)]
async fn custom_endpoint_from_options_uses_generic_mapping() {
    let server = MockServer::start().await;
    mount_queue(&server, "acme/new-model/v2", "acme/new-model", json!({"images": [{"url": "https://cdn/c.png"}]})).await;
    let cx = ctx(&server).with_options(serde_json::from_value(json!({"models": "acme/new-model/v2"})).unwrap());
    let models = Fal.models(&cx).await.unwrap();
    assert!(models.iter().any(|m| m.id == "acme/new-model/v2"));
    let out = Fal.generate(&cx, &GenRequest::new("acme/new-model/v2", Task::TextToImage, "x")).await.unwrap();
    assert_eq!(out.items.len(), 1);
}

#[tokio::test]
async fn models_are_curated() {
    let server = MockServer::start().await;
    let models = Fal.models(&ctx(&server)).await.unwrap();
    let veo = models.iter().find(|m| m.id == "fal-ai/veo3.1").unwrap();
    assert!(veo.featured && veo.audio && veo.end_frame);
    assert_eq!(veo.durations, vec![4.0, 6.0, 8.0]);
    let kontext = models.iter().find(|m| m.id == "fal-ai/flux-pro/kontext").unwrap();
    assert_eq!(kontext.tasks, vec![Task::ImageToImage]);
    assert!(models.iter().any(|m| m.supports(Task::TextToImage)) && models.iter().any(|m| m.supports(Task::ImageToVideo)));
}

#[tokio::test]
async fn check_uses_platform_api() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models/pricing"))
        .and(header("authorization", "Key test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prices": []})))
        .mount(&server)
        .await;
    assert_eq!(Fal.check(&ctx(&server)).await.unwrap(), "Key works");

    let bad = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": {"type": "authorization_error", "message": "Invalid API key"}})))
        .mount(&bad)
        .await;
    assert!(matches!(Fal.check(&ctx(&bad)).await, Err(GenError::Unauthorized { status: 401, .. })));
}

#[tokio::test]
#[ignore = "hits the real API; needs FAL_KEY"]
async fn live_smoke() {
    let key = std::env::var("FAL_KEY").expect("FAL_KEY");
    let cx = Ctx::new("fal", reqwest::Client::new(), "https://queue.fal.run").with_key(Some(key));
    println!("{}", Fal.check(&cx).await.unwrap());
    let out = Fal.generate(&cx, &GenRequest::new("fal-ai/flux-2", Task::TextToImage, "a lighthouse at dusk")).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
