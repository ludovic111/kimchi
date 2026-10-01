use kimchi_gen::providers::openai_compat::{DEFAULT, OpenAiCompat};
use kimchi_gen::*;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn png() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x02\0\0\0\x01\0\x08\x06\0\0\0".to_vec()
}

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("openai_compat", reqwest::Client::new(), format!("{}/v1", server.uri()))
}

#[tokio::test]
async fn lists_image_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object": "list", "data": [
            {"id": "llama-3.2-3b-instruct", "object": "model"},
            {"id": "stablediffusion", "object": "model"},
            {"id": "flux.1-dev", "object": "model"},
        ]})))
        .mount(&server)
        .await;
    let mut options = serde_json::Map::new();
    options.insert("model".into(), json!("my-custom-model"));
    let models = OpenAiCompat.models(&ctx(&server).with_options(options)).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["my-custom-model", "stablediffusion", "flux.1-dev"]);
    assert_eq!(models[1].tasks, [Task::TextToImage, Task::ImageToImage]);
    assert_eq!(OpenAiCompat.check(&ctx(&server)).await.unwrap(), "Connected · 2 image models");
}

#[tokio::test]
async fn generic_model_without_a_model_list() {
    let server = MockServer::start().await;
    let models = OpenAiCompat.models(&ctx(&server)).await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, DEFAULT);
    assert_eq!(OpenAiCompat.check(&ctx(&server)).await.unwrap(), "Connected (no model list)");
}

#[tokio::test]
async fn generates_with_bearer_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("authorization", "Bearer sk-local"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"created": 1, "data": [
            {"b64_json": util::b64(&png())},
            {"url": "/generated-images/b.png"},
        ]})))
        .mount(&server)
        .await;
    let cx = ctx(&server).with_key(Some("sk-local".into()));
    let mut req = GenRequest::new("stablediffusion", Task::TextToImage, "a koi pond");
    req.aspect_ratio = Some("1:1".into());
    req.count = 2;
    req.seed = Some(5);
    req.params.insert("steps".into(), json!(0));
    req.params.insert("cfg".into(), json!(4.5));
    req.params.insert("extra".into(), json!(r#"{"step": 25}"#));
    let out = OpenAiCompat.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 2);
    assert!(matches!(&out.items[0].source, OutputSource::Bytes { mime, .. } if mime == "image/png"));
    match &out.items[1].source {
        OutputSource::Url { url, .. } => assert_eq!(url, &format!("{}/generated-images/b.png", server.uri())),
        other => panic!("{other:?}"),
    }

    let r = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(
        body,
        json!({
            "model": "stablediffusion", "prompt": "a koi pond", "n": 2, "size": "1024x1024",
            "response_format": "b64_json", "seed": 5, "guidance_scale": 4.5, "step": 25,
        })
    );
}

#[tokio::test]
async fn edits_upload_multipart_without_model_for_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/edits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"b64_json": util::b64(&png())}]})))
        .mount(&server)
        .await;
    let mut req = GenRequest::new(DEFAULT, Task::ImageToImage, "watercolor style");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, png(), "image/png"));
    let out = OpenAiCompat.generate(&ctx(&server), &req).await.unwrap();
    assert_eq!(out.items.len(), 1);

    let r = &server.received_requests().await.unwrap()[0];
    let ct = r.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("multipart/form-data"), "{ct}");
    let body = String::from_utf8_lossy(&r.body);
    assert!(body.contains(r#"name="image"; filename="input.png""#), "{body}");
    assert!(body.contains("watercolor style"));
    // 512x256 input → 2:1 at 1 MP.
    assert!(body.contains("1408x704"), "{body}");
    assert!(!body.contains(r#"name="model""#));
}

#[tokio::test]
async fn bad_extra_json_is_rejected() {
    let server = MockServer::start().await;
    let mut req = GenRequest::new("m", Task::TextToImage, "x");
    req.params.insert("extra".into(), json!("[1, 2]"));
    let err = OpenAiCompat.generate(&ctx(&server), &req).await.unwrap_err();
    assert!(err.to_string().contains("Extra JSON"), "{err}");
}

#[tokio::test]
async fn unreachable_server_is_explained() {
    let cx = Ctx::new("openai_compat", reqwest::Client::new(), "http://127.0.0.1:9/v1");
    for err in [OpenAiCompat.models(&cx).await.unwrap_err(), OpenAiCompat.check(&cx).await.unwrap_err()] {
        assert!(err.to_string().contains("The server isn't running at http://127.0.0.1:9/v1"), "{err}");
    }
}

/// Needs an OpenAI-compatible image server on the default port (e.g. LocalAI or `sd-server`):
/// `cargo test -p kimchi-gen --test openai_compat -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn live() {
    let cx = Ctx::new("openai_compat", reqwest::Client::new(), "http://127.0.0.1:8080/v1");
    println!("{}", OpenAiCompat.check(&cx).await.unwrap());
    let models = OpenAiCompat.models(&cx).await.unwrap();
    let mut req = GenRequest::new(models[0].id.clone(), Task::TextToImage, "a lighthouse at dusk");
    req.aspect_ratio = Some("1:1".into());
    req.params.insert("megapixels".into(), json!(0.25));
    let out = OpenAiCompat.generate(&cx, &req).await.unwrap();
    assert!(!out.items.is_empty());
}
