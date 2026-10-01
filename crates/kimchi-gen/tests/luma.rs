use kimchi_gen::providers::luma::Luma;
use kimchi_gen::{Ctx, GenError, GenRequest, ImageRole, InputImage, OutputKind, OutputSource, Provider, Task};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("luma", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

async fn mount(server: &MockServer, done: Value) {
    Mock::given(method("POST"))
        .and(path("/generations"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "g1", "state": "queued", "output": []})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/generations/g1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "g1", "state": "processing"})))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/generations/g1"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(done))
        .mount(server)
        .await;
}

#[tokio::test(start_paused = true)]
async fn image_to_video_sends_base64_frames() {
    let server = MockServer::start().await;
    mount(&server, json!({"id": "g1", "state": "completed", "output": [{"type": "video", "url": "https://cdn/v.mp4"}]})).await;

    let mut req = GenRequest::new("ray-3.2", Task::ImageToVideo, "a wave crashes");
    req.images.push(InputImage::from_bytes(ImageRole::StartFrame, &b"\x89PNG"[..], "image/png"));
    req.images.push(InputImage::from_bytes(ImageRole::EndFrame, &b"\xFF\xD8\xFF"[..], "image/jpeg"));
    req.duration = Some(10.0);
    req.resolution = Some("1080p".into());
    req.aspect_ratio = Some("2.39:1".into());
    let out = Luma.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].kind, OutputKind::Video);
    assert!(matches!(&out.items[0].source, OutputSource::Url { url, .. } if url == "https://cdn/v.mp4"));
    let b = body(&server.received_requests().await.unwrap()[0]);
    assert_eq!(b["type"], "video");
    assert_eq!(b["model"], "ray-3.2");
    assert_eq!(b["aspect_ratio"], "21:9");
    assert_eq!(b["video"]["start_frame"], json!({"data": "iVBORw==", "media_type": "image/png"}));
    assert_eq!(b["video"]["end_frame"]["media_type"], "image/jpeg");
    assert_eq!(b["video"]["duration"], "5s", "10 s clips can't use frames");
    assert_eq!(b["video"]["resolution"], "1080p");
}

#[tokio::test(start_paused = true)]
async fn image_edit_uses_source_and_refs() {
    let server = MockServer::start().await;
    mount(&server, json!({"id": "g1", "state": "completed", "output": [{"type": "image", "url": "https://cdn/i.png"}]})).await;
    let mut req = GenRequest::new("uni-1", Task::ImageToImage, "make it winter");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG1"[..], "image/png"));
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG2"[..], "image/png"));
    req.count = 2;
    let out = Luma.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 2);
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.iter().filter(|r| r.method.as_str() == "POST").count(), 2);
    let b = body(&reqs[0]);
    assert_eq!(b["type"], "image_edit");
    assert!(b["source"]["data"].is_string());
    assert_eq!(b["image_ref"].as_array().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn moderation_failure() {
    let server = MockServer::start().await;
    mount(&server, json!({"id": "g1", "state": "failed", "failure_reason": "Prompt not allowed", "failure_code": "content_moderated"})).await;
    let e = Luma.generate(&ctx(&server), &GenRequest::new("uni-1", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Moderated(ref m) if m == "Prompt not allowed"), "{e:?}");
}

#[tokio::test]
async fn video_model_rejects_image_tasks() {
    let server = MockServer::start().await;
    let e = Luma.generate(&ctx(&server), &GenRequest::new("ray-3.2", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Unsupported(_)));
}

#[tokio::test]
async fn check_lists_files() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param("limit", "1"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;
    assert_eq!(Luma.check(&ctx(&server)).await.unwrap(), "Key works");
}

#[tokio::test]
async fn models_are_listed() {
    let models = Luma.models(&ctx(&MockServer::start().await)).await.unwrap();
    let ray = models.iter().find(|m| m.id == "ray-3.2").unwrap();
    assert!(ray.end_frame && ray.featured);
    assert_eq!(ray.durations, vec![5.0, 10.0]);
}

#[tokio::test]
#[ignore = "hits the real API; needs LUMAAI_API_KEY"]
async fn live_smoke() {
    let key = std::env::var("LUMAAI_API_KEY").expect("LUMAAI_API_KEY");
    let cx = Ctx::new("luma", reqwest::Client::new(), "https://agents.lumalabs.ai/v1").with_key(Some(key));
    println!("{}", Luma.check(&cx).await.unwrap());
    let out = Luma.generate(&cx, &GenRequest::new("uni-1", Task::TextToImage, "a lighthouse at dusk")).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
