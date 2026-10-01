use kimchi_gen::providers::comfyui::ComfyUi;
use kimchi_gen::*;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx(server: &MockServer, workflows: &std::path::Path) -> Ctx {
    let mut options = serde_json::Map::new();
    options.insert("workflows_dir".into(), json!(workflows.to_string_lossy()));
    Ctx::new("comfyui", reqwest::Client::new(), server.uri()).with_options(options)
}

/// A minimal PNG header: enough for size sniffing.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut d = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    d.extend_from_slice(&w.to_be_bytes());
    d.extend_from_slice(&h.to_be_bytes());
    d.extend_from_slice(&[8, 6, 0, 0, 0]);
    d
}

/// An image-to-video workflow using most placeholders, plus its sidecar.
fn write_video_workflow(dir: &std::path::Path) {
    let graph = json!({
        "1": {"class_type": "LoadImage", "inputs": {"image": "{{start_image}}"}},
        "2": {"class_type": "CLIPTextEncode", "inputs": {"text": "cinematic, {{prompt}}", "clip": ["9", 0]}},
        "3": {"class_type": "WanImageToVideo", "inputs": {
            "width": "{{width}}", "height": "{{height}}", "length": "{{frames}}", "start_image": ["1", 0]}},
        "4": {"class_type": "KSampler", "inputs": {"seed": "{{seed}}", "steps": "{{steps}}", "denoise": "{{ denoise }}"}},
        "5": {"class_type": "VHS_VideoCombine", "inputs": {"frame_rate": "{{fps}}", "filename_prefix": "wan"}},
    });
    std::fs::write(dir.join("wan_i2v.json"), graph.to_string()).unwrap();
    let side = json!({"name": "Wan 2.2 I2V", "defaults": {"fps": 16, "steps": 6, "megapixels": 0.5}});
    std::fs::write(dir.join("wan_i2v.kimchi.json"), side.to_string()).unwrap();
    // UI-format workflows are skipped.
    std::fs::write(dir.join("ui_format.json"), r#"{"nodes": [], "links": []}"#).unwrap();
}

#[tokio::test]
async fn lists_checkpoints_and_workflows() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    write_video_workflow(dir.path());
    Mock::given(method("GET"))
        .and(path("/models/checkpoints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(["sd_xl_base_1.0.safetensors", "v1-5.ckpt"])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/object_info/KSampler"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"KSampler": {"input": {"required": {
            "sampler_name": [["euler", "dpmpp_2m"], {}],
            "scheduler": ["COMBO", {"options": ["normal", "karras"]}],
        }}}})))
        .mount(&server)
        .await;

    let models = ComfyUi.models(&ctx(&server, dir.path())).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["workflow:wan_i2v.json", "ckpt:sd_xl_base_1.0.safetensors", "ckpt:v1-5.ckpt"]);

    let wan = &models[0];
    assert_eq!(wan.name, "Wan 2.2 I2V");
    assert_eq!(wan.tasks, [Task::ImageToVideo]);
    assert!(wan.seed && !wan.negative_prompt && !wan.end_frame);
    let fps = wan.params.iter().find(|p| p.key == "fps").unwrap();
    assert_eq!(fps.default, json!(16));

    let xl = &models[1];
    assert_eq!(xl.tasks, [Task::TextToImage, Task::ImageToImage]);
    let sampler = xl.params.iter().find(|p| p.key == "sampler").unwrap();
    assert_eq!(sampler.default, json!("dpmpp_2m"));
    let mp = xl.params.iter().find(|p| p.key == "megapixels").unwrap();
    assert_eq!(mp.default, json!(1.0));
}

#[tokio::test]
async fn runs_builtin_text_to_image() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    // An old build that ignores our prompt_id and picks its own.
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "p1", "number": 0, "node_errors": {}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/history/p1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"p1": {
            "status": {"status_str": "success", "completed": true, "messages": []},
            "outputs": {"7": {"images": [
                {"filename": "img_00001_.png", "subfolder": "kimchi", "type": "output"},
                {"filename": "img_00002_.png", "subfolder": "kimchi", "type": "output"},
            ]}},
        }})))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("ckpt:sd_xl_base_1.0.safetensors", Task::TextToImage, "a red fox");
    req.aspect_ratio = Some("16:9".into());
    req.seed = Some(7);
    req.count = 2;
    req.params.insert("steps".into(), json!(12.0));
    let out = ComfyUi.generate(&ctx(&server, dir.path()), &req).await.unwrap();
    assert_eq!(out.seed, Some(7));
    assert_eq!(out.items.len(), 2);
    match &out.items[0].source {
        OutputSource::Url { url, .. } => {
            assert_eq!(url, &format!("{}/view?filename=img_00001_.png&subfolder=kimchi&type=output", server.uri()))
        }
        other => panic!("expected a URL, got {other:?}"),
    }

    let prompt = sent_json(&server, "/prompt").await;
    let g = &prompt["prompt"];
    assert_eq!(g["1"]["inputs"]["ckpt_name"], "sd_xl_base_1.0.safetensors");
    assert_eq!(g["2"]["inputs"]["text"], "a red fox");
    assert_eq!(g["3"]["inputs"]["text"], "");
    assert_eq!(g["4"]["inputs"], json!({"width": 1328, "height": 752, "batch_size": 2}));
    let ks = &g["5"]["inputs"];
    assert_eq!((ks["seed"].clone(), ks["steps"].clone(), ks["cfg"].clone()), (json!(7), json!(12), json!(6.0)));
    assert_eq!(ks["sampler_name"], "dpmpp_2m");
    assert!(prompt["client_id"].is_string() && prompt["prompt_id"].is_string());
}

#[tokio::test]
async fn runs_custom_video_workflow() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    write_video_workflow(dir.path());
    Mock::given(method("POST"))
        .and(path("/upload/image"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"name": "kimchi_abc.png", "subfolder": "", "type": "input"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "v1", "number": 1, "node_errors": {}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/history/v1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"v1": {
            "status": {"status_str": "success", "completed": true},
            "outputs": {
                "5": {"gifs": [{"filename": "wan_00001.mp4", "subfolder": "", "type": "output", "format": "video/h264-mp4"}]},
                "6": {"images": [{"filename": "wan_00001.png", "subfolder": "", "type": "output"}]},
            },
        }})))
        .mount(&server)
        .await;

    let mut req = GenRequest::new("workflow:wan_i2v.json", Task::ImageToVideo, "a boat at dawn");
    req.images.push(InputImage::from_bytes(ImageRole::StartFrame, png(1080, 1920), "image/png"));
    req.duration = Some(5.0);
    let out = ComfyUi.generate(&ctx(&server, dir.path()), &req).await.unwrap();
    assert_eq!(out.items.len(), 1, "the still frame is dropped for a video task");
    assert_eq!(out.items[0].kind, OutputKind::Video);
    let seed = out.seed.expect("a random seed is reported");

    let g = sent_json(&server, "/prompt").await["prompt"].clone();
    assert_eq!(g["1"]["inputs"]["image"], "kimchi_abc.png");
    assert_eq!(g["2"]["inputs"]["text"], "cinematic, a boat at dawn");
    // Portrait input at 0.5 MP, frames = 5 s × 16 fps.
    assert_eq!(g["3"]["inputs"], json!({"width": 528, "height": 944, "length": 80, "start_image": ["1", 0]}));
    assert_eq!(g["4"]["inputs"], json!({"seed": seed, "steps": 6, "denoise": 0.75}));
    assert_eq!(g["5"]["inputs"]["frame_rate"], 16);
}

#[tokio::test]
async fn reports_validation_errors() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"type": "prompt_outputs_failed_validation", "message": "Prompt outputs failed validation"},
            "node_errors": {"1": {"class_type": "CheckpointLoaderSimple", "errors": [
                {"message": "Value not in list", "details": "ckpt_name: 'gone.safetensors' not in []"}]}},
        })))
        .mount(&server)
        .await;
    let req = GenRequest::new("ckpt:gone.safetensors", Task::TextToImage, "x");
    let err = ComfyUi.generate(&ctx(&server, dir.path()), &req).await.unwrap_err().to_string();
    assert!(err.contains("Prompt outputs failed validation"), "{err}");
    assert!(err.contains("CheckpointLoaderSimple: ckpt_name: 'gone.safetensors' not in []"), "{err}");
}

#[tokio::test]
async fn surfaces_execution_errors_from_history() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "e1"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/history/e1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"e1": {
            "status": {"status_str": "error", "completed": false, "messages": [
                ["execution_start", {"prompt_id": "e1"}],
                ["execution_error", {"prompt_id": "e1", "node_type": "VAEDecode", "exception_message": "out of memory"}],
            ]},
            "outputs": {},
        }})))
        .mount(&server)
        .await;
    let req = GenRequest::new("ckpt:a.safetensors", Task::TextToImage, "x");
    let err = ComfyUi.generate(&ctx(&server, dir.path()), &req).await.unwrap_err().to_string();
    assert_eq!(err, "VAEDecode failed: out of memory");
}

#[tokio::test]
async fn view_urls_escape_names() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "q"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/history/q"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"q": {"status": {"status_str": "success"}, "outputs": {
            "9": {"images": [{"filename": "a b&c.png", "subfolder": "x/y", "type": "temp"}]}}}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/view"))
        .and(query_param("filename", "a b&c.png"))
        .and(query_param("subfolder", "x/y"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png(1, 1)))
        .mount(&server)
        .await;
    let cx = ctx(&server, dir.path());
    let out = ComfyUi.generate(&cx, &GenRequest::new("ckpt:a.safetensors", Task::TextToImage, "x")).await.unwrap();
    let OutputSource::Url { url, .. } = &out.items[0].source else { panic!("expected a URL") };
    let (data, mime) = util::download(&cx, url, &[]).await.unwrap();
    assert_eq!((data.len(), mime.as_str()), (29, "image/png"));
}

#[tokio::test]
async fn unreachable_server_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let cx = Ctx::new("comfyui", reqwest::Client::new(), "http://127.0.0.1:9")
        .with_options(json!({"workflows_dir": dir.path().to_string_lossy()}).as_object().unwrap().clone());
    for err in [ComfyUi.models(&cx).await.unwrap_err(), ComfyUi.check(&cx).await.unwrap_err()] {
        assert!(matches!(err, GenError::Network { .. }));
        assert!(err.to_string().contains("ComfyUI isn't running at http://127.0.0.1:9"), "{err}");
    }
}

#[tokio::test]
async fn bad_workflow_names_are_rejected() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let req = GenRequest::new("workflow:../secrets.json", Task::TextToImage, "x");
    assert!(ComfyUi.generate(&ctx(&server, dir.path()), &req).await.is_err());
}

async fn sent_json(server: &MockServer, p: &str) -> Value {
    let reqs = server.received_requests().await.unwrap();
    let r = reqs.iter().rev().find(|r| r.url.path() == p).expect("request sent");
    serde_json::from_slice(&r.body).unwrap()
}

/// Needs ComfyUI on the default port with at least one checkpoint:
/// `cargo test -p kimchi-gen --test comfyui -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn live() {
    let cx = Ctx::new("comfyui", reqwest::Client::new(), "http://127.0.0.1:8188")
        .with_progress(|p| println!("{:?} {}", p.fraction, p.message.unwrap_or_default()));
    println!("{}", ComfyUi.check(&cx).await.unwrap());
    let models = ComfyUi.models(&cx).await.unwrap();
    let m = models.iter().find(|m| m.id.starts_with("ckpt:")).expect("a checkpoint");
    let mut req = GenRequest::new(m.id.clone(), Task::TextToImage, "a lighthouse at dusk, oil painting");
    req.aspect_ratio = Some("1:1".into());
    req.params.insert("steps".into(), json!(8));
    let out = ComfyUi.generate(&cx, &req).await.unwrap();
    assert!(!out.items.is_empty());
}
