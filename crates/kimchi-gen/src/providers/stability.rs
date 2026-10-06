//! Stability AI: Stable Diffusion 3.5 and Stable Image.
//!
//! The v2beta Stable Image endpoints are synchronous multipart calls that
//! return one image each. We ask for JSON so the seed and finish reason come
//! back alongside the base64 image.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "stability";

pub struct Stability;

const RATIOS: &[&str] = &["21:9", "16:9", "3:2", "5:4", "1:1", "4:5", "2:3", "9:16", "9:21"];

/// Which v2beta endpoint a model lives on.
#[derive(Clone, Copy, PartialEq)]
enum Endpoint {
    Ultra,
    Core,
    /// `/generate/sd3` with a `model` field.
    Sd3,
}

struct Model {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    endpoint: Endpoint,
    img2img: bool,
    negative: bool,
    /// Credits per image (1 credit = $0.01).
    credits: f64,
    featured: bool,
}

const MODELS: &[Model] = &[
    Model {
        id: "ultra",
        name: "Stable Image Ultra",
        description: "Stability's highest-quality photoreal model (SD 3.5 Large based).",
        endpoint: Endpoint::Ultra,
        img2img: true,
        negative: true,
        credits: 8.0,
        featured: true,
    },
    Model {
        id: "core",
        name: "Stable Image Core",
        description: "Fast and cheap 1.5 MP images.",
        endpoint: Endpoint::Core,
        img2img: false,
        negative: true,
        credits: 3.0,
        featured: false,
    },
    Model {
        id: "sd3.5-large",
        name: "Stable Diffusion 3.5 Large",
        description: "8B-parameter base model; strong prompt adherence.",
        endpoint: Endpoint::Sd3,
        img2img: true,
        negative: true,
        credits: 6.5,
        featured: false,
    },
    Model {
        id: "sd3.5-large-turbo",
        name: "Stable Diffusion 3.5 Large Turbo",
        description: "Distilled SD 3.5 Large, 4 steps.",
        endpoint: Endpoint::Sd3,
        img2img: true,
        negative: false,
        credits: 4.0,
        featured: false,
    },
    Model {
        id: "sd3.5-medium",
        name: "Stable Diffusion 3.5 Medium",
        description: "2.5B-parameter model, balanced speed and quality.",
        endpoint: Endpoint::Sd3,
        img2img: true,
        negative: true,
        credits: 3.5,
        featured: false,
    },
];

const STYLE_PRESETS: &[&str] = &[
    "3d-model",
    "analog-film",
    "anime",
    "cinematic",
    "comic-book",
    "digital-art",
    "enhance",
    "fantasy-art",
    "isometric",
    "line-art",
    "low-poly",
    "modeling-compound",
    "neon-punk",
    "origami",
    "photographic",
    "pixel-art",
    "tile-texture",
];

fn find(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

fn model_info(m: &Model) -> ModelInfo {
    let tasks: &[Task] = if m.img2img { &[Task::TextToImage, Task::ImageToImage] } else { &[Task::TextToImage] };
    let mut params = vec![ParamSpec {
        key: "output_format".into(),
        label: "Format".into(),
        kind: ParamKind::Select {
            options: vec![
                SelectOption::new("png", "PNG"),
                SelectOption::new("jpeg", "JPEG"),
                SelectOption::new("webp", "WebP"),
            ],
        },
        default: json!("png"),
        help: None,
    }];
    if m.img2img {
        params.push(ParamSpec {
            key: "strength".into(),
            label: "Strength".into(),
            kind: ParamKind::Float { min: 0.0, max: 1.0, step: 0.05 },
            default: json!(0.6),
            help: Some("Image-to-image: 0 keeps the input, 1 ignores it.".into()),
        });
    }
    if m.endpoint == Endpoint::Sd3 {
        params.push(ParamSpec {
            key: "cfg_scale".into(),
            label: "Guidance".into(),
            kind: ParamKind::Float { min: 1.0, max: 10.0, step: 0.5 },
            default: json!(if m.id == "sd3.5-large-turbo" { 1.0 } else { 4.0 }),
            help: None,
        });
    }
    let mut styles = vec![SelectOption::new("", "None")];
    styles.extend(STYLE_PRESETS.iter().map(|s| SelectOption::new(*s, s.replace('-', " "))));
    params.push(ParamSpec {
        key: "style_preset".into(),
        label: "Style".into(),
        kind: ParamKind::Select { options: styles },
        default: json!(""),
        help: None,
    });
    ModelInfo {
        description: Some(m.description.into()),
        aspect_ratios: RATIOS.iter().map(|s| s.to_string()).collect(),
        max_outputs: 4,
        negative_prompt: m.negative,
        seed: true,
        params,
        price: Some(format!("≈ ${} / image", format!("{:.3}", m.credits / 100.0).trim_end_matches('0'))),
        featured: m.featured,
        ..ModelInfo::new(ID, m.id, m.name, tasks)
    }
}

#[derive(Deserialize)]
struct Answer {
    image: Option<String>,
    finish_reason: Option<String>,
    seed: Option<Value>,
}

fn form(m: &Model, req: &GenRequest, seed: Option<i64>) -> GenResult<(Form, String)> {
    let format = req.param("output_format").and_then(Value::as_str).unwrap_or("png").to_string();
    let mut f = Form::new().text("prompt", req.prompt.clone()).text("output_format", format.clone());
    if m.endpoint == Endpoint::Sd3 {
        f = f.text("model", m.id);
    }
    let edit = req.task == Task::ImageToImage;
    if edit {
        if !m.img2img {
            return Err(GenError::Unsupported(format!("{} doesn't take an input image", m.name)));
        }
        let img = req.references().next().ok_or_else(|| GenError::Provider("This task needs an input image.".into()))?;
        let part = Part::bytes(img.data.to_vec()).file_name(img.file_name()).mime_str(&img.mime).map_err(|e| GenError::Provider(e.to_string()))?;
        let strength = req.param("strength").and_then(Value::as_f64).unwrap_or(0.6);
        f = f.part("image", part).text("strength", strength.to_string());
        if m.endpoint == Endpoint::Sd3 {
            f = f.text("mode", "image-to-image");
        }
    } else {
        // Ratio is ignored (and rejected on sd3) when an input image sets the size.
        f = f.text("aspect_ratio", util::closest_ratio(&req.aspect(), RATIOS).to_string());
    }
    if m.negative
        && let Some(n) = req.negative_prompt.as_ref().filter(|n| !n.trim().is_empty())
    {
        f = f.text("negative_prompt", n.clone());
    }
    if let Some(s) = seed {
        f = f.text("seed", s.rem_euclid(4_294_967_295).to_string());
    }
    if let Some(c) = req.param("cfg_scale").and_then(Value::as_f64).filter(|_| m.endpoint == Endpoint::Sd3) {
        f = f.text("cfg_scale", c.to_string());
    }
    if let Some(s) = req.param("style_preset").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        f = f.text("style_preset", s.to_string());
    }
    Ok((f, format))
}

fn path(m: &Model) -> &'static str {
    match m.endpoint {
        Endpoint::Ultra => "/v2beta/stable-image/generate/ultra",
        Endpoint::Core => "/v2beta/stable-image/generate/core",
        Endpoint::Sd3 => "/v2beta/stable-image/generate/sd3",
    }
}

/// Like [`util::send`], but Stability answers 403 for moderation too, which
/// must not look like a bad key.
async fn send(cx: &Ctx, req: reqwest::RequestBuilder) -> GenResult<String> {
    let resp = req.send().await.map_err(util::net_err(cx))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(util::net_err(cx))?;
    if (200..300).contains(&status) {
        return Ok(text);
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("content_moderation") || lower.contains("flagged") {
        return Err(GenError::Moderated(stability_message(&text)));
    }
    if status == 401 || status == 403 {
        return Err(GenError::Unauthorized { provider: cx.provider.clone(), status, message: stability_message(&text) });
    }
    Err(GenError::Http { provider: cx.provider.clone(), status, message: stability_message(&text) })
}

/// Stability errors look like `{"name": "...", "errors": ["..."]}`.
fn stability_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let errors = v.get("errors").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "));
    match errors.filter(|s| !s.is_empty()) {
        Some(e) => util::truncate(&e, 400),
        None => util::error_message(body),
    }
}

#[async_trait]
impl Provider for Stability {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Stability AI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Stable Diffusion 3.5 and Stable Image".into(),
            website: "https://platform.stability.ai".into(),
            needs_key: true,
            key_env: vec!["STABILITY_API_KEY".into()],
            key_url: Some("https://platform.stability.ai/account/keys".into()),
            key_hint: Some("sk-…".into()),
            default_base_url: "https://api.stability.ai".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToAudio],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let mut models: Vec<_> = MODELS.iter().map(model_info).collect();
        models.push(ModelInfo { durations: vec![10., 30., 60., 120., 180.], seed: true, featured: true,
            description: Some("Music and sound effects from a prompt.".into()),
            ..ModelInfo::new(ID, "stable-audio-2.5", "Stable Audio 2.5", &[Task::TextToAudio]) });
        Ok(models)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let text = send(cx, cx.http.get(cx.url("/v1/user/balance")).bearer_auth(cx.key()?)).await?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(match v.get("credits").and_then(Value::as_f64) {
            Some(c) => format!("{c:.1} credits left"),
            None => "Key works".into(),
        })
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        if req.task == Task::TextToAudio {
            if req.model != "stable-audio-2.5" { return Err(GenError::Unsupported("Choose Stable Audio for sound generation.".into())); }
            let duration = req.duration.unwrap_or(30.);
            if !duration.is_finite() || !(1.0..=190.0).contains(&duration) { return Err(GenError::Provider("Audio duration must be between 1 and 190 seconds.".into())); }
            let mut form = Form::new().text("prompt", req.prompt.clone()).text("model", "stable-audio-2.5")
                .text("duration", duration.to_string()).text("output_format", "wav");
            if let Some(seed) = req.seed { form = form.text("seed", seed.rem_euclid(4_294_967_295).to_string()); }
            cx.report(Progress::message("Generating audio…"));
            let response = util::send(cx, cx.http.post(cx.url("/v2beta/audio/stable-audio-2/text-to-audio"))
                .bearer_auth(cx.key()?).header("accept", "audio/*").multipart(form)).await?;
            let data = response.bytes().await.map_err(util::net_err(cx))?;
            if data.is_empty() { return Err(util::decode_err(cx, "Empty audio response")); }
            return Ok(GenOutput { items: vec![OutputItem::bytes(OutputKind::Audio, data, "audio/wav")], seed: req.seed, cost_usd: None });
        }

        let m = find(&req.model).ok_or_else(|| GenError::Unsupported(format!("unknown Stability model `{}`", req.model)))?;
        if !matches!(req.task, Task::TextToImage | Task::ImageToImage) {
            return Err(GenError::Unsupported(format!("{} only makes images", m.name)));
        }
        let key = cx.key()?;
        let mut out = GenOutput::default();
        let n = req.count.max(1);
        for i in 0..n {
            let label = if n > 1 { format!("Image {} of {n}", i + 1) } else { "Generating".into() };
            let (form, format) = form(m, req, req.seed.map(|s| s + i as i64))?;
            let call = send(
                cx,
                cx.http.post(cx.url(path(m))).bearer_auth(key).header("Accept", "application/json").multipart(form),
            );
            // Synchronous call: tick an estimate while we wait.
            let started = Instant::now();
            tokio::pin!(call);
            let text = loop {
                tokio::select! {
                    r = &mut call => break r?,
                    _ = tokio::time::sleep(Duration::from_millis(1000)) => util::estimate(cx, started, Duration::from_secs(8), &label),
                }
            };
            let a: Answer = serde_json::from_str(&text).map_err(|e| util::decode_err(cx, e.to_string()))?;
            if a.finish_reason.as_deref() == Some("CONTENT_FILTERED") {
                return Err(GenError::Moderated("Stability filtered the output image".into()));
            }
            let data = a.image.as_deref().and_then(|s| util::b64_decode(s).ok()).ok_or_else(|| util::decode_err(cx, "no image in response"))?;
            let mime = util::sniff_mime(&data).unwrap_or(match format.as_str() {
                "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                _ => "image/png",
            });
            out.items.push(OutputItem::bytes(OutputKind::Image, data, mime));
            let seed = a.seed.as_ref().and_then(|s| s.as_i64().or_else(|| s.as_str()?.parse().ok()));
            out.seed = out.seed.or(seed);
        }
        out.cost_usd = Some(m.credits * n as f64 / 100.0);
        Ok(out)
    }
}
