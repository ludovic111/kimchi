//! OpenAI-compatible server: anything speaking the OpenAI Images API (LocalAI, vLLM-Omni,
//! stable-diffusion.cpp `sd-server`, Nexa…). The base URL includes the `/v1` prefix.
//!
//! * `GET  /models` (`{data: [{id}]}`) lists models; ids that are obviously chat, embedding or
//!   speech models are hidden. Without that route, a single [`DEFAULT`] entry is offered and its
//!   requests omit `model`, letting single-model servers use what they loaded.
//! * `POST /images/generations` JSON `{model, prompt, n, size: "WxH", response_format: "b64_json"}`.
//! * `POST /images/edits` multipart: the same fields plus `image` (or repeated `image[]`).
//! * Results may come back as `b64_json` or `url` (relative URLs resolve against the server).
//!
//! `Authorization: Bearer` is sent when a key is saved (it's optional). Non-standard fields
//! (`negative_prompt`, `seed`, `num_inference_steps`, `guidance_scale`, as understood by vLLM-Omni
//! and others) are only sent when set, because strict servers reject unknown fields. The `extra`
//! param merges raw JSON into the request, e.g. `{"step": 30}` for LocalAI.
//!
//! # Options
//! * `model`: a model id to offer even if `/models` doesn't list it.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use super::a1111::{image_dims, offline, param, param_f64};
use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "openai_compat";

/// Model id meaning "don't send `model`".
pub const DEFAULT: &str = "default";

pub struct OpenAiCompat;

#[async_trait]
impl Provider for OpenAiCompat {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "OpenAI-compatible server".into(),
            kind: ProviderKind::Local,
            tagline: "Any server speaking the OpenAI Images API (LocalAI, vLLM-Omni, sd-server…)".into(),
            website: "https://platform.openai.com/docs/api-reference/images".into(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: Some("optional".into()),
            default_base_url: "http://127.0.0.1:8080/v1".into(),
            base_url_editable: true,
            tasks: vec![Task::TextToImage, Task::ImageToImage],
            group: ProviderGroup::Local,
            quick_start: false,
            base_url_presets: vec![],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let mut ids = match list_models(cx).await {
            Ok(ids) => ids,
            Err(e @ (GenError::Network { .. } | GenError::Unauthorized { .. })) => {
                return Err(offline(cx, "The server")(e));
            }
            // No /models route: fall back to the generic entry below.
            Err(_) => vec![],
        };
        if let Some(extra) = cx.option_str("model").map(str::trim).filter(|s| !s.is_empty())
            && !ids.iter().any(|i| i == extra)
        {
            ids.insert(0, extra.to_string());
        }
        if ids.is_empty() {
            return Ok(vec![model(DEFAULT, "Server default", "Whatever model the server has loaded")]);
        }
        Ok(ids.iter().map(|id| model(id, id, "")).collect())
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        match list_models(cx).await {
            Ok(ids) if ids.is_empty() => Ok("Connected".into()),
            Ok(ids) => Ok(format!("Connected · {} image model{}", ids.len(), if ids.len() == 1 { "" } else { "s" })),
            // Reachable, just without a model list.
            Err(GenError::Http { .. }) => Ok("Connected (no model list)".into()),
            Err(e) => Err(offline(cx, "The server")(e)),
        }
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let edit = req.task == Task::ImageToImage;
        let images: Vec<&InputImage> = if edit { req.references().collect() } else { vec![] };
        if edit && images.is_empty() {
            return Err(GenError::Provider("Image-to-image needs an input image.".into()));
        }
        let ratio = images
            .first()
            .and_then(|i| image_dims(&i.data))
            .map(|(w, h)| util::ratio_string(w, h))
            .unwrap_or_else(|| req.aspect());
        let (w, h) = util::size_for_ratio(&ratio, param_f64(req, "megapixels").unwrap_or(1.0), 64);

        let mut fields = Map::new();
        if req.model != DEFAULT && !req.model.is_empty() {
            fields.insert("model".into(), json!(req.model));
        }
        fields.insert("prompt".into(), json!(req.prompt));
        fields.insert("n".into(), json!(req.count.max(1)));
        fields.insert("size".into(), json!(format!("{w}x{h}")));
        fields.insert("response_format".into(), json!("b64_json"));
        if let Some(neg) = req.negative_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
            fields.insert("negative_prompt".into(), json!(neg));
        }
        if let Some(seed) = req.seed {
            fields.insert("seed".into(), json!(seed));
        }
        if let Some(steps) = param_f64(req, "steps").filter(|s| *s >= 1.0) {
            fields.insert("num_inference_steps".into(), json!(steps.round() as i64));
        }
        if let Some(cfg) = param_f64(req, "cfg").filter(|c| *c > 0.0) {
            fields.insert("guidance_scale".into(), json!(cfg));
        }
        fields.extend(extra_fields(req)?);

        let http = if edit {
            let mut form = reqwest::multipart::Form::new();
            for (k, v) in &fields {
                form = form.text(k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string));
            }
            let field = if images.len() > 1 { "image[]" } else { "image" };
            for img in &images {
                let part = reqwest::multipart::Part::stream_with_length(img.data.clone(), img.data.len() as u64)
                    .file_name(img.file_name())
                    .mime_str(&img.mime)
                    .map_err(|e| GenError::Provider(format!("bad image type {}: {e}", img.mime)))?;
                form = form.part(field, part);
            }
            cx.http.post(cx.url("/images/edits")).multipart(form)
        } else {
            cx.http.post(cx.url("/images/generations")).json(&fields)
        };

        let run = util::send_json::<Value>(cx, auth(cx, http));
        tokio::pin!(run);
        let started = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        let resp = loop {
            tokio::select! {
                r = &mut run => break r?,
                _ = tick.tick() => util::estimate(cx, started, Duration::from_secs(30), "Generating"),
            }
        };

        let mut items = vec![];
        for d in resp["data"].as_array().into_iter().flatten() {
            if let Some(b) = d["b64_json"].as_str().filter(|s| !s.is_empty()) {
                let data = util::b64_decode(b).map_err(|e| util::decode_err(cx, format!("bad base64 image: {e}")))?;
                let mime = util::sniff_mime(&data).unwrap_or("image/png");
                items.push(OutputItem::bytes(OutputKind::Image, data, mime));
            } else if let Some(u) = d["url"].as_str().filter(|s| !s.is_empty()) {
                items.push(OutputItem::url(OutputKind::Image, absolute(cx, u)));
            }
        }
        Ok(GenOutput { items, seed: req.seed, cost_usd: None })
    }
}

fn auth(cx: &Ctx, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match &cx.api_key {
        Some(k) => rb.bearer_auth(k),
        None => rb,
    }
}

/// Image-capable model ids from `/models`.
async fn list_models(cx: &Ctx) -> GenResult<Vec<String>> {
    let v: Value =
        util::send_json(cx, auth(cx, cx.http.get(cx.url("/models")).timeout(Duration::from_secs(10)))).await?;
    let list = v["data"].as_array().or(v["models"].as_array()).or(v.as_array()).cloned().unwrap_or_default();
    let ids: Vec<String> = list
        .iter()
        .filter_map(|m| m.as_str().or(m["id"].as_str()).or(m["name"].as_str()).map(str::to_string))
        .collect();
    let images: Vec<String> = ids.iter().filter(|id| !obviously_not_images(id)).cloned().collect();
    // If the filter would hide everything, it guessed wrong.
    Ok(if images.is_empty() { ids } else { images })
}

fn obviously_not_images(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| id.contains(k));
    let image_like = has(&[
        "image",
        "diffusion",
        "sd",
        "flux",
        "dall",
        "kontext",
        "pixart",
        "sana",
        "kolors",
        "playground",
        "hidream",
        "turbo",
        "xl",
        "paint",
        "draw",
        "art",
    ]);
    !image_like
        && has(&[
            "embed",
            "whisper",
            "tts",
            "rerank",
            "transcri",
            "speech",
            "instruct",
            "-chat",
            "coder",
            "gpt-4",
            "gpt-3",
            "gpt-oss",
            "llama",
            "mistral",
            "gemma",
            "phi-",
            "deepseek",
            "bert",
            "moderation",
            "qwen2",
            "qwen3",
            "granite",
            "smollm",
            "vl-",
            "-vl",
        ])
}

fn model(id: &str, name: &str, desc: &str) -> ModelInfo {
    ModelInfo {
        description: (!desc.is_empty()).then(|| desc.to_string()),
        max_outputs: 4,
        negative_prompt: true,
        seed: true,
        max_images: 4,
        params: vec![
            param(
                "megapixels",
                "Resolution (MP)",
                ParamKind::Float { min: 0.25, max: 4.0, step: 0.25 },
                json!(1.0),
                "Sent as size=WxH in the project's aspect ratio",
            ),
            param("steps", "Steps", ParamKind::Int { min: 0, max: 150, step: 1 }, json!(0), "0 = server default"),
            param(
                "cfg",
                "Guidance",
                ParamKind::Float { min: 0.0, max: 30.0, step: 0.5 },
                json!(0.0),
                "0 = server default",
            ),
            param(
                "extra",
                "Extra JSON",
                ParamKind::Text { multiline: true },
                json!(""),
                "Fields merged into the request, e.g. {\"quality\": \"high\"}",
            ),
        ],
        ..ModelInfo::new(ID, id, name, &[Task::TextToImage, Task::ImageToImage])
    }
}

fn extra_fields(req: &GenRequest) -> GenResult<Map<String, Value>> {
    match req.param("extra") {
        Some(Value::Object(o)) => Ok(o.clone()),
        Some(Value::String(s)) if !s.trim().is_empty() => match serde_json::from_str(s) {
            Ok(Value::Object(o)) => Ok(o),
            _ => Err(GenError::Provider("Extra JSON must be an object like {\"key\": \"value\"}.".into())),
        },
        _ => Ok(Map::new()),
    }
}

/// Resolves `/generated/x.png` against the server's origin.
fn absolute(cx: &Ctx, u: &str) -> String {
    if u.contains("://") || u.starts_with("data:") {
        return u.to_string();
    }
    match url::Url::parse(&cx.base_url).and_then(|base| base.join(u)) {
        Ok(full) => full.to_string(),
        Err(_) => u.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_text_models() {
        assert!(obviously_not_images("llama-3.2-3b-instruct"));
        assert!(obviously_not_images("nomic-embed-text"));
        assert!(!obviously_not_images("stablediffusion"));
        assert!(!obviously_not_images("qwen-image-edit"));
        assert!(!obviously_not_images("sd-cpp-local"));
    }
}
