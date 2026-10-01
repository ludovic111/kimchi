//! OpenAI: GPT Image via `/images/generations` and `/images/edits`.
//!
//! Images come back synchronously as base64. The Sora Videos API was shut
//! down on 2026-09-24 with no replacement, so this provider is image-only.

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "openai";

pub struct OpenAi;

/// GPT Image 1.x only knows three fixed sizes.
const LEGACY_RATIOS: &[&str] = &["1:1", "3:2", "2:3"];
/// GPT Image 2+ takes any `WxH` (multiples of 16, ratio within 1:3–3:1).
const FREE_RATIOS: &[&str] = &["1:1", "4:3", "3:4", "3:2", "2:3", "16:9", "9:16", "21:9", "9:21"];
const MAX_REFERENCES: usize = 16;

fn select(key: &str, label: &str, options: &[(&str, &str)], default: &str, help: &str) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: label.into(),
        kind: ParamKind::Select { options: options.iter().map(|(v, l)| SelectOption::new(*v, *l)).collect() },
        default: json!(default),
        help: Some(help.into()),
    }
}

fn params(extra_quality: bool) -> Vec<ParamSpec> {
    let mut quality = vec![("auto", "Auto"), ("low", "Low"), ("medium", "Medium"), ("high", "High")];
    if extra_quality {
        quality.extend([("xhigh", "Extra high"), ("max", "Max")]);
    }
    vec![
        select("quality", "Quality", &quality, "auto", "Higher quality costs more and takes longer."),
        select(
            "background",
            "Background",
            &[("auto", "Auto"), ("opaque", "Opaque"), ("transparent", "Transparent")],
            "auto",
            "Transparent backgrounds are saved as PNG.",
        ),
        select(
            "moderation",
            "Moderation",
            &[("auto", "Standard"), ("low", "Less strict")],
            "auto",
            "How strictly OpenAI filters prompts and results.",
        ),
    ]
}

/// Whether the model takes arbitrary sizes (GPT Image 2 and later).
fn free_size(model: &str) -> bool {
    !model.starts_with("gpt-image-1") && !model.starts_with("chatgpt-image")
}

fn image_model(id: &str, name: &str, price: &str, featured: bool, description: &str) -> ModelInfo {
    let free = free_size(id);
    let ratios = if free { FREE_RATIOS } else { LEGACY_RATIOS };
    ModelInfo {
        aspect_ratios: ratios.iter().map(|s| s.to_string()).collect(),
        resolutions: if free { vec!["1K".into(), "2K".into(), "4K".into()] } else { vec![] },
        max_outputs: 10,
        max_images: MAX_REFERENCES as u32,
        params: params(id.starts_with("gpt-image-2.5")),
        price: Some(price.into()),
        featured,
        description: Some(description.into()),
        ..ModelInfo::new(ID, id, name, &[Task::TextToImage, Task::ImageToImage])
    }
}

fn catalog() -> Vec<ModelInfo> {
    vec![
        image_model(
            "gpt-image-2.5-sunburst",
            "GPT Image 2.5 Sunburst",
            "$30 / 1M output tokens",
            true,
            "OpenAI's most capable image model: precise edits, legible text, up to 4K.",
        ),
        image_model(
            "gpt-image-2.5-flare",
            "GPT Image 2.5 Flare",
            "$30 / 1M output tokens",
            true,
            "Fast everyday GPT Image 2.5.",
        ),
        image_model("gpt-image-2", "GPT Image 2", "$30 / 1M output tokens", false, "Previous generation, any size."),
        image_model(
            "gpt-image-1.5",
            "GPT Image 1.5",
            "≈ $0.01–0.20 / image",
            false,
            "Retiring 2026-12-01. Square, 3:2 and 2:3 only.",
        ),
        image_model(
            "gpt-image-1-mini",
            "GPT Image 1 Mini",
            "≈ $0.005–0.05 / image",
            false,
            "Retiring 2026-12-01. Cheapest GPT Image.",
        ),
    ]
}

/// `WxH` for the request: a fixed size on 1.x, a free size on 2+.
fn image_size(req: &GenRequest) -> String {
    if !free_size(&req.model) {
        return match util::closest_ratio(&req.aspect(), LEGACY_RATIOS) {
            "3:2" => "1536x1024",
            "2:3" => "1024x1536",
            _ => "1024x1024",
        }
        .into();
    }
    let ratio = util::closest_ratio(&req.aspect(), FREE_RATIOS);
    let mp = match req.resolution.as_deref().map(str::to_ascii_uppercase).as_deref() {
        Some("4K") => 8.2,
        Some("2K") => 3.6,
        _ => 1.05,
    };
    let (mut w, mut h) = util::size_for_ratio(ratio, mp, 16);
    // Keep inside the documented limits: edges ≤ 3840, ≤ 8,294,400 px total.
    let over = (w.max(h) as f64 / 3840.0).max((w as f64 * h as f64 / 8_294_400.0).sqrt());
    if over > 1.0 {
        w = ((w as f64 / over / 16.0).floor() as u32) * 16;
        h = ((h as f64 / over / 16.0).floor() as u32) * 16;
    }
    format!("{w}x{h}")
}

fn param_str<'a>(req: &'a GenRequest, key: &str) -> Option<&'a str> {
    req.param(key).and_then(Value::as_str).filter(|s| !s.is_empty() && *s != "auto")
}

/// Parses an OpenAI-style images response (`data[].b64_json` or `data[].url`).
/// Shared with the other providers that speak this dialect.
pub(crate) fn parse_images(cx: &Ctx, v: &Value, fallback_mime: &str) -> GenResult<Vec<OutputItem>> {
    let data = v.get("data").and_then(Value::as_array).ok_or_else(|| util::decode_err(cx, "no `data` array"))?;
    let mut items = Vec::with_capacity(data.len());
    for d in data {
        if let Some(b) = d.get("b64_json").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            let bytes = util::b64_decode(b).map_err(|e| util::decode_err(cx, format!("bad base64: {e}")))?;
            let mime = util::sniff_mime(&bytes).unwrap_or(fallback_mime).to_string();
            items.push(OutputItem::bytes(OutputKind::Image, bytes, mime));
        } else if let Some(u) = d.get("url").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            items.push(OutputItem::url(OutputKind::Image, u));
        }
    }
    Ok(items)
}

/// Turns an HTTP 400 that mentions moderation into [`GenError::Moderated`].
pub(crate) fn moderation(e: GenError) -> GenError {
    const HINTS: &[&str] =
        &["moderation", "safety system", "content policy", "content_policy", "safety_violation", "nsfw"];
    match e {
        GenError::Http { status: 400 | 422, ref message, .. }
            if HINTS.iter().any(|k| message.to_ascii_lowercase().contains(k)) =>
        {
            GenError::Moderated(message.clone())
        }
        e => e,
    }
}

fn image_part(img: &InputImage, name: String) -> GenResult<Part> {
    Part::bytes(img.data.to_vec())
        .file_name(name)
        .mime_str(&img.mime)
        .map_err(|e| GenError::Provider(format!("bad image type `{}`: {e}", img.mime)))
}

#[async_trait]
impl Provider for OpenAi {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "OpenAI".into(),
            kind: ProviderKind::Cloud,
            tagline: "GPT Image: precise edits and legible text".into(),
            website: "https://platform.openai.com".into(),
            needs_key: true,
            key_env: vec!["OPENAI_API_KEY".into()],
            key_url: Some("https://platform.openai.com/api-keys".into()),
            key_hint: Some("sk-…".into()),
            default_base_url: "https://api.openai.com/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(catalog())
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(cx, cx.http.get(cx.url("/models")).bearer_auth(cx.key()?)).await?;
        let ids: Vec<&str> =
            v.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(|m| m["id"].as_str()).collect();
        let images = ids.iter().filter(|id| id.starts_with("gpt-image")).count();
        Ok(format!("Key works · {images} GPT Image models available"))
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        if !matches!(req.task, Task::TextToImage | Task::ImageToImage) {
            return Err(GenError::Unsupported("OpenAI no longer offers video generation through its API".into()));
        }
        let key = cx.key()?;
        let n = req.count.clamp(1, 10);
        let size = image_size(req);
        let options: Vec<(&str, &str)> =
            ["quality", "background", "moderation"].into_iter().filter_map(|k| Some((k, param_str(req, k)?))).collect();
        let refs: Vec<&InputImage> = req.references().take(MAX_REFERENCES).collect();
        cx.report(Progress::message("Generating"));

        let resp: Value = if refs.is_empty() {
            let mut body = json!({ "model": req.model, "prompt": req.prompt, "n": n, "size": size });
            for (k, v) in options {
                body[k] = json!(v);
            }
            util::send_json(cx, cx.http.post(cx.url("/images/generations")).bearer_auth(key).json(&body)).await
        } else {
            let mut form = Form::new()
                .text("model", req.model.clone())
                .text("prompt", req.prompt.clone())
                .text("n", n.to_string())
                .text("size", size);
            for (k, v) in options {
                form = form.text(k, v.to_string());
            }
            for (i, img) in refs.iter().enumerate() {
                form = form.part("image[]", image_part(img, format!("image{i}.{}", util::extension_for(&img.mime)))?);
            }
            util::send_json(cx, cx.http.post(cx.url("/images/edits")).bearer_auth(key).multipart(form)).await
        }
        .map_err(moderation)?;

        let items = parse_images(cx, &resp, "image/png")?;
        Ok(GenOutput { items, ..Default::default() })
    }
}
