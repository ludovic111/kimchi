//! Luma AI: Ray video and Uni images.
//!
//! Uses the Luma Agents API (`agents.lumalabs.ai/v1`), which replaced the
//! Dream Machine API: that one only took public image URLs and its Ray 2 /
//! Photon models are being retired. One endpoint, `POST /generations` with a
//! `type`, then poll `GET /generations/{id}` until `completed`. Images are sent
//! inline as base64 (`{data, media_type}`, up to 50 MB each).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "luma";

pub struct Luma;

const RAY_RATIOS: &[&str] = &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"];
const UNI_RATIOS: &[&str] = &["3:1", "2:1", "16:9", "3:2", "1:1", "2:3", "9:16", "1:2", "1:3"];
const RAY_RESOLUTIONS: &[&str] = &["360p", "540p", "720p", "1080p"];

fn image_model(id: &str, name: &str, description: &str, price: &str, featured: bool) -> ModelInfo {
    ModelInfo {
        description: Some(description.into()),
        aspect_ratios: UNI_RATIOS.iter().map(|s| s.to_string()).collect(),
        max_outputs: 4,
        max_images: 9,
        params: vec![ParamSpec {
            key: "style".into(),
            label: "Style".into(),
            kind: ParamKind::Select { options: vec![SelectOption::new("auto", "Auto"), SelectOption::new("manga", "Manga")] },
            default: json!("auto"),
            help: None,
        }],
        price: Some(price.into()),
        featured,
        ..ModelInfo::new(ID, id, name, &[Task::TextToImage, Task::ImageToImage])
    }
}

fn models() -> Vec<ModelInfo> {
    let bool_param = |key: &str, label: &str, help: &str| ParamSpec {
        key: key.into(),
        label: label.into(),
        kind: ParamKind::Bool,
        default: json!(false),
        help: Some(help.into()),
    };
    vec![
        ModelInfo {
            description: Some("Luma's video model: physical realism, start/end frames, HDR.".into()),
            aspect_ratios: RAY_RATIOS.iter().map(|s| s.to_string()).collect(),
            durations: vec![5.0, 10.0],
            resolutions: RAY_RESOLUTIONS.iter().map(|s| s.to_string()).collect(),
            end_frame: true,
            params: vec![
                bool_param("hdr", "HDR", "16-bit HDR output (720p/1080p, 5 s)."),
                bool_param("loop", "Loop", "Make the clip loop seamlessly (5 s, no end frame)."),
            ],
            price: Some("$0.06 (360p) – $1.20 (1080p) / 5 s".into()),
            featured: true,
            ..ModelInfo::new(ID, "ray-3.2", "Ray 3.2", &[Task::TextToVideo, Task::ImageToVideo])
        },
        image_model("uni-1", "Uni-1", "Fast image generation and editing with up to 9 references.", "≈ $0.04 / image", true),
        image_model("uni-1-max", "Uni-1 Max", "Luma's highest-quality image model.", "≈ $0.10 / image", false),
    ]
}

fn image_ref(img: &InputImage) -> Value {
    json!({"data": img.base64(), "media_type": img.mime})
}

fn body(req: &GenRequest) -> GenResult<Map<String, Value>> {
    let mut b = Map::new();
    b.insert("model".into(), json!(req.model));
    b.insert("prompt".into(), json!(req.prompt));
    match req.task {
        Task::TextToVideo | Task::ImageToVideo => {
            if req.model != "ray-3.2" {
                return Err(GenError::Unsupported(format!("{} only makes images", req.model)));
            }
            b.insert("type".into(), json!("video"));
            b.insert("aspect_ratio".into(), json!(util::closest_ratio(&req.aspect(), RAY_RATIOS)));
            let flag = |k: &str| req.param(k).and_then(Value::as_bool).unwrap_or(false);
            let (hdr, looped) = (flag("hdr"), flag("loop"));
            let mut v = Map::new();
            let start = req.task.needs_image().then(|| req.start_frame()).flatten();
            let end = req.task.needs_image().then(|| req.image(ImageRole::EndFrame)).flatten();
            if let Some(s) = start.filter(|s| s.role != ImageRole::EndFrame) {
                v.insert("start_frame".into(), image_ref(s));
            }
            if let Some(e) = end {
                v.insert("end_frame".into(), image_ref(e));
            }
            // 10 s clips can't use frames, HDR or looping.
            let ten_ok = v.is_empty() && !hdr && !looped;
            let wanted = req.duration.unwrap_or(5.0);
            v.insert("duration".into(), json!(if wanted >= 7.5 && ten_ok { "10s" } else { "5s" }));
            let mut res = req
                .resolution
                .as_deref()
                .and_then(|r| RAY_RESOLUTIONS.iter().find(|x| x.eq_ignore_ascii_case(r)).copied())
                .unwrap_or("720p");
            if hdr {
                // HDR needs 720p or 1080p.
                if !matches!(res, "720p" | "1080p") {
                    res = "720p";
                }
                v.insert("hdr".into(), json!(true));
            }
            v.insert("resolution".into(), json!(res));
            if looped && end.is_none() {
                v.insert("loop".into(), json!(true));
            }
            b.insert("video".into(), Value::Object(v));
        }
        Task::TextToImage | Task::ImageToImage => {
            if req.model == "ray-3.2" {
                return Err(GenError::Unsupported("Ray 3.2 only makes videos".into()));
            }
            b.insert("aspect_ratio".into(), json!(util::closest_ratio(&req.aspect(), UNI_RATIOS)));
            b.insert("output_format".into(), json!("png"));
            if let Some(s) = req.param("style").and_then(Value::as_str) {
                b.insert("style".into(), json!(s));
            }
            let refs: Vec<&InputImage> = req.references().collect();
            match (req.task, refs.split_first()) {
                (Task::ImageToImage, Some((source, rest))) => {
                    // Edit the first image; the others guide it.
                    b.insert("type".into(), json!("image_edit"));
                    b.insert("source".into(), image_ref(source));
                    if !rest.is_empty() {
                        b.insert("image_ref".into(), json!(rest.iter().take(8).map(|i| image_ref(i)).collect::<Vec<_>>()));
                    }
                }
                _ => {
                    b.insert("type".into(), json!("image"));
                    if !refs.is_empty() {
                        b.insert("image_ref".into(), json!(refs.iter().take(9).map(|i| image_ref(i)).collect::<Vec<_>>()));
                    }
                }
            }
        }
    }
    Ok(b)
}

#[derive(Deserialize)]
struct Generation {
    id: String,
    state: String,
    #[serde(default)]
    output: Vec<Output>,
    failure_reason: Option<String>,
    failure_code: Option<String>,
}

#[derive(Deserialize)]
struct Output {
    #[serde(rename = "type")]
    kind: Option<String>,
    url: String,
}

async fn run_one(cx: &Ctx, body: &Map<String, Value>, kind: OutputKind, label: &str) -> GenResult<Vec<OutputItem>> {
    let key = cx.key()?;
    let mut g: Generation = util::send_json(cx, cx.http.post(cx.url("/generations")).bearer_auth(key).json(body)).await?;
    let (every, expected) = match kind {
        OutputKind::Video => (Duration::from_secs(5), Duration::from_secs(90)),
        _ => (Duration::from_secs(2), Duration::from_secs(15)),
    };
    let started = Instant::now();
    let id = g.id.clone();
    if !matches!(g.state.as_str(), "completed" | "failed") {
        g = util::poll(every, Duration::from_secs(60 * 20), || async {
            let g: Generation = util::send_json(cx, cx.http.get(cx.url(&format!("/generations/{id}"))).bearer_auth(key)).await?;
            match g.state.as_str() {
                "completed" | "failed" => Ok(Some(g)),
                "queued" => {
                    cx.report(Progress::message("In queue"));
                    Ok(None)
                }
                _ => {
                    util::estimate(cx, started, expected, label);
                    Ok(None)
                }
            }
        })
        .await?;
    }
    if g.state == "failed" {
        let reason = g.failure_reason.unwrap_or_else(|| "generation failed".into());
        return Err(match g.failure_code.as_deref() {
            Some("content_moderated") => GenError::Moderated(reason),
            Some(code) => GenError::Provider(format!("Luma: {reason} ({code})")),
            None => GenError::Provider(format!("Luma: {reason}")),
        });
    }
    let items: Vec<OutputItem> = g
        .output
        .into_iter()
        .map(|o| {
            let k = match o.kind.as_deref() {
                Some("video") => OutputKind::Video,
                Some("image") => OutputKind::Image,
                _ => kind,
            };
            OutputItem::url(k, o.url)
        })
        .collect();
    if items.is_empty() {
        return Err(util::decode_err(cx, "generation completed without output"));
    }
    Ok(items)
}

#[async_trait]
impl Provider for Luma {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Luma AI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Ray video and Uni images".into(),
            website: "https://lumalabs.ai/api".into(),
            needs_key: true,
            key_env: vec!["LUMAAI_API_KEY".into(), "LUMA_API_KEY".into()],
            key_url: Some("https://platform.lumalabs.ai".into()),
            key_hint: Some("luma-api-…".into()),
            default_base_url: "https://agents.lumalabs.ai/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
            group: ProviderGroup::Media,
            quick_start: false,
            base_url_presets: vec![],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(models())
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        // There is no balance endpoint; listing uploaded files is free and authenticated.
        let _: Value = util::send_json(cx, cx.http.get(cx.url("/files")).query(&[("limit", "1")]).bearer_auth(cx.key()?)).await?;
        Ok("Key works".into())
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        if !models().iter().any(|m| m.id == req.model) {
            return Err(GenError::Unsupported(format!("unknown Luma model `{}`", req.model)));
        }
        let body = body(req)?;
        let kind = req.task.output();
        let n = req.count.max(1);
        let mut out = GenOutput::default();
        for i in 0..n {
            let label = if n > 1 { format!("{} of {n}", i + 1) } else { "Generating".into() };
            out.items.extend(run_one(cx, &body, kind, &label).await?);
        }
        out.items.truncate(n as usize);
        Ok(out)
    }
}
