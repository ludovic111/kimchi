use kimchi_gen::providers::runway::Runway;
use kimchi_gen::{Ctx, GenError, GenRequest, ImageRole, InputImage, OutputKind, OutputSource, Provider, Task};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("runway", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

async fn mount_task(server: &MockServer, endpoint: &str, done: Value) {
    Mock::given(method("POST"))
        .and(path(endpoint))
        .and(header("authorization", "Bearer test"))
        .and(header("x-runway-version", "2024-11-06"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "t1"})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/tasks/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "t1", "status": "PENDING"})))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/tasks/t1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "t1", "status": "RUNNING", "progress": 0.5})))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/tasks/t1"))
        .and(header("x-runway-version", "2024-11-06"))
        .respond_with(ResponseTemplate::new(200).set_body_json(done))
        .mount(server)
        .await;
}

#[tokio::test(start_paused = true)]
async fn image_to_video_with_last_frame() {
    let server = MockServer::start().await;
    mount_task(&server, "/image_to_video", json!({"status": "SUCCEEDED", "output": ["https://cdn/v.mp4"], "cost": {"credits": 160}})).await;

    let mut req = GenRequest::new("veo3.1", Task::ImageToVideo, "the sun sets");
    req.images.push(InputImage::from_bytes(ImageRole::StartFrame, &b"\x89PNGa"[..], "image/png"));
    req.images.push(InputImage::from_bytes(ImageRole::EndFrame, &b"\x89PNGb"[..], "image/png"));
    req.aspect_ratio = Some("9:16".into());
    req.resolution = Some("1080p".into());
    req.duration = Some(5.0);
    req.audio = Some(true);
    req.negative_prompt = Some("blur".into());
    let out = Runway.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].kind, OutputKind::Video);
    assert!(matches!(&out.items[0].source, OutputSource::Url { url, .. } if url == "https://cdn/v.mp4"));
    assert_eq!(out.cost_usd, Some(1.6));
    let b = body(&server.received_requests().await.unwrap()[0]);
    assert_eq!(b["model"], "veo3.1");
    assert_eq!(b["ratio"], "1080:1920");
    assert_eq!(b["duration"], 4);
    assert_eq!(b["audio"], true);
    assert_eq!(b["negativePrompt"], "blur");
    assert_eq!(b["promptImage"][0]["position"], "first");
    assert_eq!(b["promptImage"][1]["position"], "last");
    assert!(b["promptImage"][1]["uri"].as_str().unwrap().starts_with("data:image/png;base64,"));
}

#[tokio::test(start_paused = true)]
async fn text_to_video_gen45_requires_duration() {
    let server = MockServer::start().await;
    mount_task(&server, "/text_to_video", json!({"status": "SUCCEEDED", "output": ["https://cdn/g.mp4"]})).await;
    let mut req = GenRequest::new("gen4.5", Task::TextToVideo, "waves");
    req.width = Some(1920);
    req.height = Some(1080);
    req.seed = Some(5);
    req.params.insert("public_figures".into(), json!("low"));
    Runway.generate(&ctx(&server), &req).await.unwrap();
    let b = body(&server.received_requests().await.unwrap()[0]);
    assert_eq!(b["ratio"], "1280:720");
    assert_eq!(b["duration"], 5);
    assert_eq!(b["seed"], 5);
    assert_eq!(b["promptText"], "waves");
    assert_eq!(b["contentModeration"]["publicFigureThreshold"], "low");
}

#[tokio::test(start_paused = true)]
async fn image_references_are_tagged() {
    let server = MockServer::start().await;
    mount_task(&server, "/text_to_image", json!({"status": "SUCCEEDED", "output": ["https://cdn/i.png"]})).await;
    let mut req = GenRequest::new("gen4_image", Task::ImageToImage, "@image1 on a beach");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG"[..], "image/png"));
    req.aspect_ratio = Some("4:3".into());
    req.resolution = Some("720p".into());
    let out = Runway.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items[0].kind, OutputKind::Image);
    let b = body(&server.received_requests().await.unwrap()[0]);
    assert_eq!(b["ratio"], "960:720");
    assert_eq!(b["referenceImages"][0]["tag"], "image1");
}

#[tokio::test(start_paused = true)]
async fn safety_failures_are_moderation() {
    let server = MockServer::start().await;
    mount_task(&server, "/text_to_video", json!({"status": "FAILED", "failure": "Prompt flagged", "failureCode": "SAFETY.INPUT.TEXT"})).await;
    let e = Runway.generate(&ctx(&server), &GenRequest::new("veo3.1_fast", Task::TextToVideo, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Moderated(ref m) if m == "Prompt flagged"), "{e:?}");
}

#[tokio::test(start_paused = true)]
async fn wrong_task_is_rejected_before_any_call() {
    let server = MockServer::start().await;
    let e = Runway.generate(&ctx(&server), &GenRequest::new("gen4_turbo", Task::TextToVideo, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Unsupported(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn check_reads_organization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/organization"))
        .and(header("x-runway-version", "2024-11-06"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"creditBalance": 1200, "tier": {}})))
        .mount(&server)
        .await;
    assert_eq!(Runway.check(&ctx(&server)).await.unwrap(), "1200 credits left");
}

#[tokio::test]
async fn models_have_friendly_ratios() {
    let server = MockServer::start().await;
    let models = Runway.models(&ctx(&server)).await.unwrap();
    let turbo = models.iter().find(|m| m.id == "gen4_turbo").unwrap();
    assert_eq!(turbo.tasks, vec![Task::ImageToVideo]);
    assert_eq!(turbo.aspect_ratios, vec!["16:9", "9:16", "4:3", "1:1", "3:4", "21:9"]);
    assert!(models.iter().find(|m| m.id == "veo3.1").unwrap().end_frame);
}

#[tokio::test]
#[ignore = "hits the real API; needs RUNWAYML_API_SECRET"]
async fn live_smoke() {
    let key = std::env::var("RUNWAYML_API_SECRET").expect("RUNWAYML_API_SECRET");
    let cx = Ctx::new("runway", reqwest::Client::new(), "https://api.dev.runwayml.com/v1").with_key(Some(key));
    println!("{}", Runway.check(&cx).await.unwrap());
    let out = Runway.generate(&cx, &GenRequest::new("gen4_image", Task::TextToImage, "a lighthouse at dusk")).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
