use kimchi_gen::providers::bfl::Bfl;
use kimchi_gen::{Ctx, GenError, GenRequest, ImageRole, InputImage, OutputSource, Provider, Task};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn ctx(server: &MockServer) -> Ctx {
    Ctx::new("bfl", reqwest::Client::new(), server.uri()).with_key(Some("test".into()))
}

fn body(r: &Request) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

#[tokio::test(start_paused = true)]
async fn text_to_image_submits_then_polls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/flux-2-pro"))
        .and(header("x-key", "test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "abc",
            "polling_url": format!("{}/get_result?id=abc", server.uri()),
            "cost": 3.0
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/get_result"))
        .and(query_param("id", "abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "abc", "status": "Pending"})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/get_result"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "abc", "status": "Ready",
            "result": {"sample": "https://delivery.bfl.ai/x.png", "seed": 42}
        })))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("flux-2-pro", Task::TextToImage, "a red fox");
    req.aspect_ratio = Some("16:9".into());
    req.params.insert("prompt_upsampling".into(), json!(false));
    let out = Bfl.generate(&ctx(&server), &req).await.unwrap();

    assert_eq!(out.items.len(), 1);
    assert!(matches!(&out.items[0].source, OutputSource::Url { url, .. } if url == "https://delivery.bfl.ai/x.png"));
    assert_eq!(out.seed, Some(42));
    assert_eq!(out.cost_usd, Some(0.03));

    let reqs = server.received_requests().await.unwrap();
    let b = body(&reqs[0]);
    assert_eq!(b["prompt"], "a red fox");
    assert_eq!((b["width"].as_u64().unwrap(), b["height"].as_u64().unwrap()), (1328, 752));
    assert_eq!(b["disable_pup"], true, "FLUX.2 [pro] takes upsampling as disable_pup");
    assert_eq!(b["output_format"], "png");
    assert_eq!(reqs.iter().filter(|r| r.url.path() == "/get_result").count(), 2);
}

#[tokio::test(start_paused = true)]
async fn edit_sends_base64_images_and_ratio() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/flux-kontext-pro"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "k1"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/get_result"))
        .and(query_param("id", "k1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "Ready", "result": {"sample": "https://x/y.png"}})))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("flux-kontext-pro", Task::ImageToImage, "make it night");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, &b"\x89PNGdata"[..], "image/png"));
    req.width = Some(1080);
    req.height = Some(1920);
    req.seed = Some(7);
    Bfl.generate(&ctx(&server), &req).await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let b = body(&reqs[0]);
    assert_eq!(b["input_image"], "iVBOR2RhdGE=");
    assert_eq!(b["aspect_ratio"], "9:16");
    assert_eq!(b["seed"], 7);
}

#[tokio::test(start_paused = true)]
async fn moderation_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "m"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "Content Moderated", "details": {"Moderation Reasons": ["Derivative Works Filter"]}
        })))
        .mount(&server)
        .await;
    let err = Bfl.generate(&ctx(&server), &GenRequest::new("flux-pro-1.1", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(err, GenError::Moderated(ref m) if m.contains("Derivative")), "{err:?}");
}

#[tokio::test]
async fn check_reads_credits_and_maps_bad_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/credits"))
        .and(header("x-key", "test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"credits": 123.4})))
        .mount(&server)
        .await;
    assert_eq!(Bfl.check(&ctx(&server)).await.unwrap(), "123 credits left");

    let bad = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(403)).mount(&bad).await;
    assert!(matches!(Bfl.check(&ctx(&bad)).await, Err(GenError::Unauthorized { status: 403, .. })));
}

#[tokio::test]
async fn models_are_listed() {
    let server = MockServer::start().await;
    let models = Bfl.models(&ctx(&server)).await.unwrap();
    assert!(models.iter().any(|m| m.featured));
    assert!(models.iter().all(|m| m.provider == "bfl" && m.supports(Task::TextToImage)));
}

#[tokio::test]
#[ignore = "hits the real API; needs BFL_API_KEY"]
async fn live_smoke() {
    let key = std::env::var("BFL_API_KEY").expect("BFL_API_KEY");
    let cx = Ctx::new("bfl", reqwest::Client::new(), "https://api.bfl.ai/v1").with_key(Some(key));
    println!("{}", Bfl.check(&cx).await.unwrap());
    let out = Bfl.generate(&cx, &GenRequest::new("flux-2-pro", Task::TextToImage, "a lighthouse at dusk")).await.unwrap();
    assert_eq!(out.items.len(), 1);
}
