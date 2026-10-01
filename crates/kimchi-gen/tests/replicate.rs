use kimchi_gen::providers::replicate::Replicate;
use kimchi_gen::{Ctx, GenError, GenRequest, ImageRole, InputImage, OutputKind, OutputSource, Provider, Task};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("replicate", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

#[tokio::test(start_paused = true)]
async fn fast_image_finishes_with_prefer_wait() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/black-forest-labs/flux-schnell/predictions"))
        .and(header("authorization", "Bearer test"))
        .and(header("prefer", "wait=30"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "p1", "status": "succeeded",
            "output": ["https://replicate.delivery/a.png", "https://replicate.delivery/b.png"],
            "urls": {"get": "unused"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("black-forest-labs/flux-schnell", Task::TextToImage, "a cat");
    req.count = 2;
    req.seed = Some(4);
    req.aspect_ratio = Some("2.35:1".into());
    let out = Replicate.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 2);
    assert!(matches!(&out.items[1].source, OutputSource::Url { url, .. } if url.ends_with("b.png")));
    let b = body(&server.received_requests().await.unwrap()[0]);
    assert_eq!(b["input"]["num_outputs"], 2);
    assert_eq!(b["input"]["seed"], 4);
    assert_eq!(b["input"]["aspect_ratio"], "21:9");
    assert_eq!(b["input"]["output_format"], "png");
}

#[tokio::test(start_paused = true)]
async fn video_polls_urls_get() {
    let server = MockServer::start().await;
    let get = format!("{}/predictions/p2", server.uri());
    Mock::given(method("POST"))
        .and(path("/models/google/veo-3.1/predictions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "p2", "status": "starting", "output": null, "urls": {"get": get}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/predictions/p2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "processing", "logs": "Starting video generation..."})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/predictions/p2"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "succeeded", "output": "https://replicate.delivery/v.mp4"})))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("google/veo-3.1", Task::ImageToVideo, "the camera pans");
    req.images.push(InputImage::from_bytes(ImageRole::StartFrame, &b"\x89PNG"[..], "image/png"));
    req.images.push(InputImage::from_bytes(ImageRole::EndFrame, &b"\x89PNG"[..], "image/png"));
    req.duration = Some(10.0);
    req.resolution = Some("1080p".into());
    req.audio = Some(false);
    let out = Replicate.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].kind, OutputKind::Video);
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs[0].headers.get("prefer").is_none(), "videos don't hold the connection");
    let input = &body(&reqs[0])["input"];
    assert!(input["image"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert!(input["last_frame"].is_string());
    assert_eq!(input["duration"], 8);
    assert_eq!(input["resolution"], "1080p");
    assert_eq!(input["generate_audio"], false);
}

#[tokio::test(start_paused = true)]
async fn kling_maps_resolution_to_mode_and_wan_switches_model() {
    let server = MockServer::start().await;
    for model in ["kwaivgi/kling-v3-video", "wan-video/wan-2.7-i2v"] {
        Mock::given(method("POST"))
            .and(path(format!("/models/{model}/predictions")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"status": "succeeded", "output": "https://x/v.mp4"})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let mut req = GenRequest::new("kwaivgi/kling-v3-video", Task::TextToVideo, "x");
    req.resolution = Some("1080p".into());
    Replicate.generate(&ctx(&server), &req).await.unwrap();
    let mut req = GenRequest::new("wan-video/wan-2.7-t2v", Task::ImageToVideo, "x");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG"[..], "image/png"));
    Replicate.generate(&ctx(&server), &req).await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(body(&reqs[0])["input"]["mode"], "pro");
    assert!(body(&reqs[1])["input"]["first_frame"].is_string());
}

#[tokio::test(start_paused = true)]
async fn moderation_and_failures() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/bytedance/seedance-2.0/predictions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "status": "failed", "error": "The input or output was flagged as sensitive. Please try again with different inputs. (E005)"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/models/google/imagen-4-ultra/predictions"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"detail": "Invalid input: aspect_ratio"})))
        .mount(&server)
        .await;
    let cx = ctx(&server);
    let e = Replicate.generate(&cx, &GenRequest::new("bytedance/seedance-2.0", Task::TextToVideo, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Moderated(ref m) if m.contains("E005")), "{e:?}");
    let e = Replicate.generate(&cx, &GenRequest::new("google/imagen-4-ultra", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Http { status: 422, ref message, .. } if message.contains("aspect_ratio")), "{e:?}");
}

#[tokio::test(start_paused = true)]
async fn community_model_is_mapped_from_its_schema() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models/someone/cool-model"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "owner": "someone", "name": "cool-model", "is_official": false,
            "latest_version": {"id": "abc123", "openapi_schema": {"components": {"schemas": {
                "Input": {"properties": {
                    "prompt": {"type": "string"},
                    "input_image": {"type": "string", "format": "uri"},
                    "aspect_ratio": {"allOf": [{"$ref": "#/components/schemas/aspect_ratio"}]},
                    "num_outputs": {"type": "integer", "maximum": 2},
                    "seed": {"type": "integer"}
                }},
                "aspect_ratio": {"type": "string", "enum": ["1:1", "16:9", "9:16"]}
            }}}}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/predictions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"status": "succeeded", "output": ["https://x/1.png", "https://x/2.png"]})))
        .expect(2)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("someone/cool-model", Task::ImageToImage, "x");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG"[..], "image/png"));
    req.aspect_ratio = Some("4:5".into());
    req.count = 3;
    let out = Replicate.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 3);
    let post = server.received_requests().await.unwrap().into_iter().find(|r| r.url.path() == "/predictions").unwrap();
    let b = body(&post);
    assert_eq!(b["version"], "abc123");
    assert_eq!(b["input"]["aspect_ratio"], "1:1");
    assert_eq!(b["input"]["num_outputs"], 2);
    assert!(b["input"]["input_image"].as_str().unwrap().starts_with("data:"));
}

#[tokio::test]
async fn models_merge_collections() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/collections/text-to-image"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"owner": "google", "name": "nano-banana-pro", "is_official": true},
            {"owner": "acme", "name": "new-thing", "description": "New!", "is_official": true},
            {"owner": "someone", "name": "fork", "is_official": false}
        ]})))
        .mount(&server)
        .await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let models = Replicate.models(&ctx(&server)).await.unwrap();
    assert_eq!(models.iter().filter(|m| m.id == "google/nano-banana-pro").count(), 1);
    assert!(models.iter().any(|m| m.id == "acme/new-thing" && m.tasks == vec![Task::TextToImage]));
    assert!(!models.iter().any(|m| m.id == "someone/fork"));
    assert!(models.iter().find(|m| m.id == "google/veo-3.1").unwrap().end_frame);
}

#[tokio::test]
async fn check_reads_account() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"type": "user", "username": "ludo"})))
        .mount(&server)
        .await;
    assert_eq!(Replicate.check(&ctx(&server)).await.unwrap(), "Signed in as ludo");
}

#[tokio::test]
#[ignore = "hits the real API; needs REPLICATE_API_TOKEN"]
async fn live_smoke() {
    let key = std::env::var("REPLICATE_API_TOKEN").expect("REPLICATE_API_TOKEN");
    let cx = Ctx::new("replicate", reqwest::Client::new(), "https://api.replicate.com/v1").with_key(Some(key));
    println!("{}", Replicate.check(&cx).await.unwrap());
    let out = Replicate.generate(&cx, &GenRequest::new("black-forest-labs/flux-schnell", Task::TextToImage, "a lighthouse")).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
