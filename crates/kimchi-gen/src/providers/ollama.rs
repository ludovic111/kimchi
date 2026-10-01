//! Ollama: local image models with one command.
//!
//! Image generation in Ollama is experimental (MLX engine):
//! * `GET  /api/tags` lists installed models; image models carry `"capabilities": ["image"]`.
//! * `POST /api/generate` `{model, prompt, width, height, steps, images: [base64], options: {seed}}`
//!   streams NDJSON lines `{completed, total}` and ends with `{image: <base64 PNG>, done: true}`.
//!   One image per request, so `count` > 1 runs sequentially with consecutive seeds.
//! * `GET  /api/version` for the status line.
//!
//! State as of October 2026: image models only run on macOS with Apple Silicon, and Ollama
//! 0.32.6 "temporarily" removed image generation (still absent in 0.35): `/api/generate` answers
//! 400 "image generation models are not currently supported". 0.32.5 is the last working version.
//! Known models: `x/z-image-turbo` (text to image) and `x/flux2-klein` (text to image and editing
//! with up to a few reference images; `:4b` is Apache-2.0, `:9b` non-commercial).
//!
//! No provider options.

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};

use super::a1111::{offline, param, param_f64};
use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "ollama";

/// Last release with image generation; it was removed in the next one.
const LAST_WORKING: (u64, u64, u64) = (0, 32, 5);
const CAVEAT: &str =
    "Experimental: needs macOS on Apple Silicon and Ollama 0.32.5 (later versions turned image generation off).";

/// Well-known pullable image models: (name, display name, can edit, default steps, blurb).
const KNOWN: &[(&str, &str, bool, i64, &str)] = &[
    ("x/z-image-turbo", "Z-Image Turbo", false, 9, "Fast 6B photoreal model with good text rendering (13 GB)."),
    (
        "x/flux2-klein",
        "FLUX.2 Klein",
        true,
        4,
        "Generates and edits from reference images. 4B Apache-2.0 (5.7 GB); :9b is non-commercial.",
    ),
];

pub struct Ollama;

#[async_trait]
impl Provider for Ollama {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Ollama".into(),
            kind: ProviderKind::Local,
            tagline: "Local image models with one command".into(),
            website: "https://ollama.com".into(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: None,
            default_base_url: "http://127.0.0.1:11434".into(),
            base_url_editable: true,
            tasks: vec![Task::TextToImage, Task::ImageToImage],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let req = cx.http.get(cx.url("/api/tags")).timeout(Duration::from_secs(10));
        let tags: Value = util::send_json(cx, req).await.map_err(offline(cx, "Ollama"))?;
        let mut out = vec![];
        for m in tags["models"].as_array().into_iter().flatten() {
            let Some(name) = m["name"].as_str().or(m["model"].as_str()) else { continue };
            let is_image = match m["capabilities"].as_array() {
                Some(caps) => caps.iter().any(|c| c == "image"),
                // Older servers don't report capabilities in /api/tags.
                None => known(name).is_some(),
            };
            if is_image {
                out.push(model(name, true));
            }
        }
        for (base, ..) in KNOWN {
            if !out.iter().any(|m| m.id.split(':').next() == Some(*base)) {
                out.push(model(base, false));
            }
        }
        Ok(out)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let req = cx.http.get(cx.url("/api/version")).timeout(Duration::from_secs(10));
        let v: Value = util::send_json(cx, req).await.map_err(offline(cx, "Ollama"))?;
        let version = v["version"].as_str().unwrap_or("?");
        Ok(match parse_version(version) {
            Some(ver) if ver > LAST_WORKING => {
                format!("Ollama {version} · image generation was removed in 0.32.6; use 0.32.5 for image models")
            }
            _ => format!("Ollama {version}"),
        })
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let edit = req.task == Task::ImageToImage;
        let images: Vec<String> = if edit { req.references().map(InputImage::base64).collect() } else { vec![] };
        let mut body = json!({ "model": req.model, "prompt": req.prompt, "stream": true });
        if !images.is_empty() {
            // Edits default to the reference image's size.
            body["images"] = json!(images);
        } else {
            let mp = param_f64(req, "megapixels").unwrap_or(1.0);
            let (w, h) = util::size_for_ratio(&req.aspect(), mp, 16);
            body["width"] = json!(w.min(4096));
            body["height"] = json!(h.min(4096));
        }
        if let Some(steps) = param_f64(req, "steps") {
            body["steps"] = json!(steps.round() as i64);
        }
        let seed = req.seed.unwrap_or_else(|| (uuid::Uuid::new_v4().as_u128() as i64) & 0x7fff_ffff);
        let count = req.count.max(1);
        let mut items = vec![];
        for i in 0..count {
            body["options"] = json!({ "seed": seed + i as i64 });
            let image = run_one(cx, req, &body, i, count).await?;
            let data = util::b64_decode(&image).map_err(|e| util::decode_err(cx, format!("bad base64 image: {e}")))?;
            let mime = util::sniff_mime(&data).unwrap_or("image/png");
            items.push(OutputItem::bytes(OutputKind::Image, data, mime));
        }
        Ok(GenOutput { items, seed: Some(seed), cost_usd: None })
    }
}

/// One streamed `/api/generate` call; returns the base64 image.
async fn run_one(cx: &Ctx, req: &GenRequest, body: &Value, i: u32, count: u32) -> GenResult<String> {
    let resp =
        util::send(cx, cx.http.post(cx.url("/api/generate")).json(body)).await.map_err(|e| explain(e, &req.model))?;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = vec![];
    let mut image = None;
    let label = if count > 1 { format!("Image {}/{count} · ", i + 1) } else { String::new() };
    cx.report(Progress::message(format!("{label}Loading model")));
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk.map_err(util::net_err(cx))?);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            handle_line(cx, &line, &label, i, count, &mut image)?;
        }
    }
    handle_line(cx, &buf, &label, i, count, &mut image)?;
    image.ok_or_else(|| GenError::Provider("Ollama finished without an image. Is this an image model?".into()))
}

fn handle_line(cx: &Ctx, line: &[u8], label: &str, i: u32, count: u32, image: &mut Option<String>) -> GenResult<()> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let v: Value = serde_json::from_slice(line)
        .map_err(|e| util::decode_err(cx, format!("{e}: {}", util::truncate(&String::from_utf8_lossy(line), 200))))?;
    if let Some(err) = v["error"].as_str() {
        return Err(GenError::Provider(err.to_string()));
    }
    if let (Some(done), Some(total)) = (v["completed"].as_f64(), v["total"].as_f64().filter(|t| *t > 0.0)) {
        let f = (i as f64 + done / total) / count as f64;
        cx.report(Progress::fraction(f * 0.97, format!("{label}Step {done}/{total}")));
    }
    if let Some(img) = v["image"].as_str().filter(|s| !s.is_empty()) {
        *image = Some(img.to_string());
    }
    Ok(())
}

/// Turns Ollama's terse errors into next steps.
fn explain(e: GenError, model: &str) -> GenError {
    match e {
        GenError::Http { status: 400, message, .. } if message.contains("not currently supported") => GenError::Provider(
            "This Ollama version has image generation turned off (removed in 0.32.6). Install Ollama 0.32.5 to use image models."
                .into(),
        ),
        GenError::Http { status: 404, .. } => {
            GenError::Provider(format!("{model} isn't installed. Run `ollama pull {model}` in a terminal."))
        }
        e => e,
    }
}

fn known(name: &str) -> Option<&'static (&'static str, &'static str, bool, i64, &'static str)> {
    let base = name.split(':').next().unwrap_or(name);
    KNOWN.iter().find(|(k, ..)| *k == base || k.trim_start_matches("x/") == base)
}

fn model(name: &str, installed: bool) -> ModelInfo {
    let k = known(name);
    let edit = k.is_some_and(|k| k.2);
    let tasks: &[Task] = if edit { &[Task::TextToImage, Task::ImageToImage] } else { &[Task::TextToImage] };
    let tag = name.split_once(':').map(|(_, t)| t).filter(|t| *t != "latest");
    let display = match (k, tag) {
        (Some(k), Some(t)) => format!("{} ({t})", k.1),
        (Some(k), None) => k.1.to_string(),
        (None, _) => name.to_string(),
    };
    let mut desc = k.map(|k| format!("{} ", k.4)).unwrap_or_default();
    if !installed {
        desc.push_str(&format!("Not installed: run `ollama pull {name}`. "));
    }
    desc.push_str(CAVEAT);
    let mut params = vec![param(
        "megapixels",
        "Resolution (MP)",
        ParamKind::Float { min: 0.25, max: 4.0, step: 0.25 },
        json!(1.0),
        "Text to image only; edits keep the reference size",
    )];
    if let Some(k) = k {
        params.push(param("steps", "Steps", ParamKind::Int { min: 1, max: 50, step: 1 }, json!(k.3), ""));
    }
    ModelInfo {
        description: Some(desc),
        max_outputs: 4,
        seed: true,
        max_images: if edit { 4 } else { 0 },
        params,
        ..ModelInfo::new(ID, name, display, tasks)
    }
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim_start_matches('v').split(['.', '-', '+']).map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("0.32.5"), Some((0, 32, 5)));
        assert!(parse_version("0.35.1-rc0").unwrap() > LAST_WORKING);
        assert_eq!(parse_version("weird"), None);
    }

    #[test]
    fn known_models() {
        assert!(known("x/flux2-klein:9b").unwrap().2);
        assert_eq!(model("x/z-image-turbo:latest", true).name, "Z-Image Turbo");
        assert!(model("x/flux2-klein", false).description.unwrap().contains("ollama pull"));
    }
}
