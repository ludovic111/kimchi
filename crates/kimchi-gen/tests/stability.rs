use kimchi_gen::providers::stability::Stability;
use kimchi_gen::{Ctx, GenError, GenRequest, ImageRole, InputImage, OutputSource, Provider, Task};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("stability", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

const PNG_B64: &str = "iVBORw0KGgo="; // PNG magic

fn field<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("name=\"{name}\"");
    let at = body.find(&marker)?;
    let rest = &body[at + marker.len()..];
    let start = rest.find("\r\n\r\n")? + 4;
    let end = rest[start..].find("\r\n--")?;
    Some(&rest[start..start + end])
}

#[tokio::test(start_paused = true)]
async fn ultra_text_to_image_multipart() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2beta/stable-image/generate/ultra"))
        .and(header("authorization", "Bearer test"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "image": PNG_B64, "finish_reason": "SUCCESS", "seed": 1234
        })))
        .expect(2)
        .mount(&server)
        .await;

    let mut req = GenRequest::new("ultra", Task::TextToImage, "a castle");
    req.negative_prompt = Some("blurry".into());
    req.aspect_ratio = Some("1920:1080".into());
    req.seed = Some(10);
    req.count = 2;
    let out = Stability.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 2);
    assert!(matches!(&out.items[0].source, OutputSource::Bytes { mime, .. } if mime == "image/png"));
    assert_eq!(out.seed, Some(1234));
    assert_eq!(out.cost_usd, Some(0.16));

    let reqs = server.received_requests().await.unwrap();
    let b = String::from_utf8_lossy(&reqs[0].body);
    assert_eq!(field(&b, "prompt"), Some("a castle"));
    assert_eq!(field(&b, "negative_prompt"), Some("blurry"));
    assert_eq!(field(&b, "aspect_ratio"), Some("16:9"));
    assert_eq!(field(&b, "seed"), Some("10"));
    assert_eq!(field(&String::from_utf8_lossy(&reqs[1].body), "seed"), Some("11"));
}

#[tokio::test(start_paused = true)]
async fn sd3_image_to_image_sends_mode_and_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2beta/stable-image/generate/sd3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"image": PNG_B64, "finish_reason": "SUCCESS", "seed": 5})))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("sd3.5-large", Task::ImageToImage, "watercolor");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNG...."[..], "image/png"));
    req.params.insert("strength".into(), json!(0.4));
    Stability.generate(&ctx(&server), &req).await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let b = String::from_utf8_lossy(&reqs[0].body);
    assert_eq!(field(&b, "model"), Some("sd3.5-large"));
    assert_eq!(field(&b, "mode"), Some("image-to-image"));
    assert_eq!(field(&b, "strength"), Some("0.4"));
    assert!(b.contains("name=\"image\"; filename=\"input.png\""));
    assert!(field(&b, "aspect_ratio").is_none());
}

#[tokio::test(start_paused = true)]
async fn content_filter_and_moderation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2beta/stable-image/generate/core"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"image": PNG_B64, "finish_reason": "CONTENT_FILTERED", "seed": 1})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2beta/stable-image/generate/ultra"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "id": "x", "name": "content_moderation", "errors": ["Your request was flagged by our content moderation system"]
        })))
        .mount(&server)
        .await;
    let cx = ctx(&server);
    let e = Stability.generate(&cx, &GenRequest::new("core", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Moderated(_)), "{e:?}");
    let e = Stability.generate(&cx, &GenRequest::new("ultra", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(e, GenError::Moderated(ref m) if m.contains("flagged")), "{e:?}");
}

#[tokio::test]
async fn check_reads_balance() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/user/balance"))
        .and(header("authorization", "Bearer test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"credits": 42.5})))
        .mount(&server)
        .await;
    assert_eq!(Stability.check(&ctx(&server)).await.unwrap(), "42.5 credits left");

    let bad = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(401).set_body_json(json!({"name": "unauthorized", "errors": ["bad key"]}))).mount(&bad).await;
    assert!(matches!(Stability.check(&ctx(&bad)).await, Err(GenError::Unauthorized { status: 401, .. })));
}

#[tokio::test]
async fn models_are_listed() {
    let server = MockServer::start().await;
    let models = Stability.models(&ctx(&server)).await.unwrap();
    let ultra = models.iter().find(|m| m.id == "ultra").unwrap();
    assert!(ultra.featured && ultra.negative_prompt && ultra.supports(Task::ImageToImage));
    assert!(!models.iter().find(|m| m.id == "core").unwrap().supports(Task::ImageToImage));
}

#[tokio::test]
#[ignore = "hits the real API; needs STABILITY_API_KEY"]
async fn live_smoke() {
    let key = std::env::var("STABILITY_API_KEY").expect("STABILITY_API_KEY");
    let cx = Ctx::new("stability", reqwest::Client::new(), "https://api.stability.ai").with_key(Some(key));
    println!("{}", Stability.check(&cx).await.unwrap());
    let out = Stability.generate(&cx, &GenRequest::new("core", Task::TextToImage, "a lighthouse at dusk")).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
