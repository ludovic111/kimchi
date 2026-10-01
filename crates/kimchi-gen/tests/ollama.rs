use std::sync::Arc;

use kimchi_gen::providers::ollama::Ollama;
use kimchi_gen::*;
use parking_lot::Mutex;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn png() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x08\0\0\0\x08\x08\x06\0\0\0".to_vec()
}

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("ollama", reqwest::Client::new(), server.uri())
}

#[tokio::test]
async fn lists_image_models_and_suggestions() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"name": "llama3.2:latest", "capabilities": ["completion", "tools"]},
            {"name": "x/flux2-klein:9b", "capabilities": ["image"]},
            {"name": "someone/sdxl-mlx:latest", "capabilities": ["image"]},
        ]})))
        .mount(&server)
        .await;
    let models = Ollama.models(&ctx(&server)).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["x/flux2-klein:9b", "someone/sdxl-mlx:latest", "x/z-image-turbo"]);
    assert_eq!(models[0].name, "FLUX.2 Klein (9b)");
    assert_eq!(models[0].tasks, [Task::TextToImage, Task::ImageToImage]);
    assert_eq!(models[1].tasks, [Task::TextToImage]);
    let suggestion = models[2].description.as_deref().unwrap();
    assert!(suggestion.contains("ollama pull x/z-image-turbo") && suggestion.contains("macOS"), "{suggestion}");
}

#[tokio::test]
async fn streams_progress_and_decodes_the_image() {
    let server = MockServer::start().await;
    let lines = [
        json!({"model": "x/z-image-turbo", "done": false, "completed": 1, "total": 9}),
        json!({"model": "x/z-image-turbo", "done": false, "completed": 9, "total": 9}),
        json!({"model": "x/z-image-turbo", "done": true, "done_reason": "stop", "image": util::b64(&png())}),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/x-ndjson"))
        .expect(2)
        .mount(&server)
        .await;

    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let cx = ctx(&server).with_progress(move |p| sink.lock().push(p));
    let mut req = GenRequest::new("x/z-image-turbo", Task::TextToImage, "a neon street");
    req.aspect_ratio = Some("16:9".into());
    req.seed = Some(10);
    req.count = 2;
    req.params.insert("steps".into(), json!(9));
    let out = Ollama.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 2);
    assert_eq!(out.seed, Some(10));
    assert!(matches!(&out.items[1].source, OutputSource::Bytes { data, .. } if data[..] == png()[..]));
    let msgs: Vec<String> = seen.lock().iter().filter_map(|p| p.message.clone()).collect();
    assert!(msgs.contains(&"Image 2/2 · Step 1/9".to_string()), "{msgs:?}");

    let reqs = server.received_requests().await.unwrap();
    let bodies: Vec<Value> = reqs.iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect();
    assert_eq!(bodies[0]["width"], 1328);
    assert_eq!(bodies[0]["height"], 752);
    assert_eq!(bodies[0]["steps"], 9);
    assert_eq!((bodies[0]["options"]["seed"].clone(), bodies[1]["options"]["seed"].clone()), (json!(10), json!(11)));
}

#[tokio::test]
async fn edits_send_reference_images() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("{}\n", json!({"done": true, "image": util::b64(&png())}))),
        )
        .mount(&server)
        .await;
    let mut req = GenRequest::new("x/flux2-klein", Task::ImageToImage, "make it night");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, png(), "image/png"));
    Ollama.generate(&ctx(&server), &req).await.unwrap();
    let r = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(body["images"], json!([util::b64(&png())]));
    assert!(body.get("width").is_none(), "edits keep the reference size");
}

#[tokio::test]
async fn explains_disabled_image_generation_and_missing_models() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": "image generation models are not currently supported"})),
        )
        .mount(&server)
        .await;
    let err =
        Ollama.generate(&ctx(&server), &GenRequest::new("x/z-image-turbo", Task::TextToImage, "x")).await.unwrap_err();
    assert!(err.to_string().contains("Install Ollama 0.32.5"), "{err}");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "model 'x/flux2-klein' not found"})))
        .mount(&server)
        .await;
    let err =
        Ollama.generate(&ctx(&server), &GenRequest::new("x/flux2-klein", Task::TextToImage, "x")).await.unwrap_err();
    assert!(err.to_string().contains("ollama pull x/flux2-klein"), "{err}");
}

#[tokio::test]
async fn stream_errors_surface() {
    let server = MockServer::start().await;
    let body = format!("{}\n{}\n", json!({"completed": 1, "total": 4}), json!({"error": "out of memory"}));
    Mock::given(method("POST"))
        .and(path("/api/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let err =
        Ollama.generate(&ctx(&server), &GenRequest::new("x/z-image-turbo", Task::TextToImage, "x")).await.unwrap_err();
    assert_eq!(err.to_string(), "out of memory");
}

#[tokio::test]
async fn check_warns_about_versions_without_image_generation() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"version": "0.34.4"})))
        .mount(&server)
        .await;
    let line = Ollama.check(&ctx(&server)).await.unwrap();
    assert!(line.starts_with("Ollama 0.34.4") && line.contains("0.32.5"), "{line}");
}

#[tokio::test]
async fn unreachable_server_is_explained() {
    let cx = Ctx::new("ollama", reqwest::Client::new(), "http://127.0.0.1:9");
    for err in [Ollama.models(&cx).await.unwrap_err(), Ollama.check(&cx).await.unwrap_err()] {
        assert!(err.to_string().contains("Ollama isn't running at http://127.0.0.1:9"), "{err}");
    }
}

/// Needs Ollama ≤ 0.32.5 on macOS with an image model pulled:
/// `cargo test -p kimchi-gen --test ollama -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn live() {
    let cx = Ctx::new("ollama", reqwest::Client::new(), "http://127.0.0.1:11434")
        .with_progress(|p| println!("{:?} {}", p.fraction, p.message.unwrap_or_default()));
    println!("{}", Ollama.check(&cx).await.unwrap());
    let models = Ollama.models(&cx).await.unwrap();
    let m = models
        .iter()
        .find(|m| !m.description.as_deref().unwrap_or("").contains("Not installed"))
        .expect("an image model");
    let mut req = GenRequest::new(m.id.clone(), Task::TextToImage, "a lighthouse at dusk");
    req.aspect_ratio = Some("1:1".into());
    req.params.insert("megapixels".into(), json!(0.5));
    let out = Ollama.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
