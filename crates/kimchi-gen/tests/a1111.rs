use std::sync::Arc;

use kimchi_gen::providers::a1111::{A1111, CURRENT};
use kimchi_gen::*;
use parking_lot::Mutex;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut d = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    d.extend_from_slice(&w.to_be_bytes());
    d.extend_from_slice(&h.to_be_bytes());
    d.extend_from_slice(&[8, 6, 0, 0, 0]);
    d
}

async fn mock_json(server: &MockServer, m: &str, p: &str, status: u16, body: Value) {
    Mock::given(method(m))
        .and(path(p))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

async fn sent_json(server: &MockServer, p: &str) -> Value {
    let reqs = server.received_requests().await.unwrap();
    let r = reqs.iter().rev().find(|r| r.url.path() == p).expect("request sent");
    serde_json::from_slice(&r.body).unwrap()
}

#[tokio::test]
async fn lists_checkpoints_with_samplers() {
    let server = MockServer::start().await;
    mock_json(&server, "GET", "/sdapi/v1/sd-models", 200, json!([
        {"title": "sd_xl_base_1.0.safetensors [31e35c80fc]", "model_name": "sd_xl_base_1.0", "filename": "/m/sd_xl_base_1.0.safetensors"},
        {"title": "v1-5-pruned-emaonly.safetensors [6ce0161689]", "model_name": "v1-5-pruned-emaonly"},
    ]))
    .await;
    mock_json(
        &server,
        "GET",
        "/sdapi/v1/samplers",
        200,
        json!([{"name": "Euler a", "aliases": []}, {"name": "DPM++ 2M"}]),
    )
    .await;
    mock_json(&server, "GET", "/sdapi/v1/schedulers", 404, json!({"detail": "Not Found"})).await;

    let models = A1111.models(&Ctx::new("a1111", reqwest::Client::new(), server.uri())).await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "sd_xl_base_1.0.safetensors [31e35c80fc]");
    assert_eq!(models[0].name, "sd_xl_base_1.0");
    let param = |m: &ModelInfo, k: &str| m.params.iter().find(|p| p.key == k).cloned();
    assert_eq!(param(&models[0], "megapixels").unwrap().default, json!(1.0));
    assert_eq!(param(&models[1], "megapixels").unwrap().default, json!(0.4));
    match param(&models[0], "sampler").unwrap().kind {
        ParamKind::Select { options } => assert_eq!(options.len(), 3, "Default + 2 samplers"),
        k => panic!("{k:?}"),
    }
    assert!(param(&models[0], "scheduler").is_none(), "no picker without a scheduler list");
}

#[tokio::test]
async fn falls_back_to_the_loaded_model_for_draw_things() {
    let server = MockServer::start().await;
    mock_json(&server, "GET", "/sdapi/v1/options", 200, json!({"model": "flux_1_schnell_q5p.ckpt", "steps": 4})).await;
    let cx = Ctx::new("a1111", reqwest::Client::new(), server.uri());
    let models = A1111.models(&cx).await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!((models[0].id.as_str(), models[0].name.as_str()), (CURRENT, "flux_1_schnell_q5p.ckpt"));
    assert_eq!(A1111.check(&cx).await.unwrap(), "Connected · flux_1_schnell_q5p.ckpt");
}

#[tokio::test]
async fn txt2img_with_progress() {
    let server = MockServer::start().await;
    let info = json!({"seed": 1234, "all_seeds": [1234, 1235]}).to_string();
    let images = [util::b64(&png(8, 8)), util::b64(&png(8, 8)), util::b64(b"extra controlnet map")];
    Mock::given(method("POST"))
        .and(path("/sdapi/v1/txt2img"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(1200))
                .set_body_json(json!({"images": images, "parameters": {}, "info": info})),
        )
        .mount(&server)
        .await;
    mock_json(
        &server,
        "GET",
        "/sdapi/v1/progress",
        200,
        json!({
            "progress": 0.5, "eta_relative": 3.2, "state": {"sampling_step": 10, "sampling_steps": 20, "job_count": 1}
        }),
    )
    .await;

    let seen = Arc::new(Mutex::new(vec![]));
    let sink = seen.clone();
    let cx = Ctx::new("a1111", reqwest::Client::new(), server.uri()).with_progress(move |p| sink.lock().push(p));
    let mut req = GenRequest::new("sd_xl_base_1.0.safetensors [31e35c80fc]", Task::TextToImage, "a castle");
    req.negative_prompt = Some("blurry".into());
    req.width = Some(1920);
    req.height = Some(1080);
    req.count = 2;
    req.params.insert("sampler".into(), json!("DPM++ 2M"));
    req.params.insert("steps".into(), json!(20));
    let out = A1111.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 2, "extra images beyond count are dropped");
    assert_eq!(out.seed, Some(1234));
    assert!(matches!(&out.items[0].source, OutputSource::Bytes { mime, .. } if mime == "image/png"));
    assert!(seen.lock().iter().any(|p| p.message.as_deref() == Some("Sampling 10/20 · ~3s left")));

    let body = sent_json(&server, "/sdapi/v1/txt2img").await;
    assert_eq!(body["override_settings"]["sd_model_checkpoint"], "sd_xl_base_1.0.safetensors [31e35c80fc]");
    assert_eq!(body["override_settings_restore_afterwards"], false);
    assert_eq!((body["width"].clone(), body["height"].clone()), (json!(1344), json!(768)));
    assert_eq!((body["batch_size"].clone(), body["seed"].clone()), (json!(2), json!(-1)));
    assert_eq!((body["sampler_name"].clone(), body["steps"].clone()), (json!("DPM++ 2M"), json!(20)));
    assert_eq!(body["negative_prompt"], "blurry");
    assert!(body.get("scheduler").is_none());
}

#[tokio::test]
async fn img2img_keeps_input_size_for_the_loaded_model() {
    let server = MockServer::start().await;
    mock_json(&server, "POST", "/sdapi/v1/img2img", 200, json!({"images": [util::b64(&png(768, 512))]})).await;
    let cx = Ctx::new("a1111", reqwest::Client::new(), server.uri());
    let mut req = GenRequest::new(CURRENT, Task::ImageToImage, "make it winter");
    req.images.push(InputImage::from_bytes(ImageRole::Reference, png(768, 512), "image/png"));
    req.params.insert("denoise".into(), json!(0.35));
    let out = A1111.generate(&cx, &req).await.unwrap();
    assert_eq!(out.items.len(), 1);

    let body = sent_json(&server, "/sdapi/v1/img2img").await;
    assert_eq!(body["init_images"][0], util::b64(&png(768, 512)));
    assert_eq!(body["denoising_strength"], 0.35);
    assert_eq!((body["width"].clone(), body["height"].clone()), (json!(768), json!(512)));
    assert!(body.get("override_settings").is_none());
}

#[tokio::test]
async fn errors_are_passed_through() {
    let server = MockServer::start().await;
    mock_json(
        &server,
        "POST",
        "/sdapi/v1/txt2img",
        500,
        json!({"error": "OutOfMemoryError", "detail": "", "errors": "CUDA out of memory"}),
    )
    .await;
    let cx = Ctx::new("a1111", reqwest::Client::new(), server.uri());
    let err = A1111.generate(&cx, &GenRequest::new("m", Task::TextToImage, "x")).await.unwrap_err();
    assert!(matches!(err, GenError::Http { status: 500, .. }), "{err}");
}

#[tokio::test]
async fn unreachable_server_is_explained() {
    let cx = Ctx::new("a1111", reqwest::Client::new(), "http://127.0.0.1:9");
    for err in [A1111.models(&cx).await.unwrap_err(), A1111.check(&cx).await.unwrap_err()] {
        assert!(err.to_string().contains("Stable Diffusion WebUI isn't running at http://127.0.0.1:9"), "{err}");
    }
}

/// Needs A1111/Forge/SD.Next started with `--api` on the default port:
/// `cargo test -p kimchi-gen --test a1111 -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn live() {
    let cx = Ctx::new("a1111", reqwest::Client::new(), "http://127.0.0.1:7860")
        .with_progress(|p| println!("{:?} {}", p.fraction, p.message.unwrap_or_default()));
    println!("{}", A1111.check(&cx).await.unwrap());
    let models = A1111.models(&cx).await.unwrap();
    let mut req = GenRequest::new(models[0].id.clone(), Task::TextToImage, "a lighthouse at dusk, oil painting");
    req.aspect_ratio = Some("1:1".into());
    req.params.insert("steps".into(), json!(8));
    let out = A1111.generate(&cx, &req).await.unwrap();
    assert!(!out.items.is_empty());
}
