//! Google Gemini API: Gemini image ("Nano Banana"), Gemini Omni and Veo video.
//!
//! * Images: `POST /models/{id}:generateContent` with `responseModalities`
//!   and `imageConfig`; one image per call, so `count` runs calls in parallel.
//! * Gemini Omni: `POST /interactions` blocks until the clip is rendered and,
//!   with `delivery: "uri"`, returns a file we poll (`GET /files/{id}`) until
//!   `ACTIVE` before downloading `/files/{id}:download?alt=media`.
//! * Veo 3.1 (shuts down 2026-10-22): `POST /models/{id}:predictLongRunning`,
//!   poll the operation, download the sample URI with the key header.
//!
//! Imagen was shut down on 2026-08-17 and isn't offered.

use std::future::Future;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "google";

pub struct Google;

const KEY_HEADER: &str = "x-goog-api-key";
const STANDARD_RATIOS: &[&str] = &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"];
const FLASH_RATIOS: &[&str] =
    &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9", "1:4", "4:1", "1:8", "8:1"];
const VIDEO_RATIOS: &[&str] = &["16:9", "9:16"];
const VEO_SECONDS: &[f64] = &[4.0, 6.0, 8.0];
/// Finish / block reasons that mean the safety filters stopped the request.
const BLOCKED: &[&str] = &[
    "SAFETY",
    "PROHIBITED_CONTENT",
    "BLOCKLIST",
    "SPII",
    "RECITATION",
    "IMAGE_SAFETY",
    "IMAGE_PROHIBITED_CONTENT",
    "IMAGE_RECITATION",
];

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn image_model(id: &str, name: &str, ratios: &[&str], sizes: &[&str], price: &str, description: &str) -> ModelInfo {
    ModelInfo {
        aspect_ratios: strings(ratios),
        resolutions: strings(sizes),
        max_outputs: 4,
        max_images: 14,
        price: Some(price.into()),
        featured: id != "gemini-3.1-flash-lite-image",
        description: Some(description.into()),
        ..ModelInfo::new(ID, id, name, &[Task::TextToImage, Task::ImageToImage])
    }
}

fn veo_model(id: &str, name: &str, lite: bool, price: &str) -> ModelInfo {
    ModelInfo {
        aspect_ratios: strings(VIDEO_RATIOS),
        durations: VEO_SECONDS.to_vec(),
        resolutions: strings(if lite { &["720p", "1080p"] } else { &["720p", "1080p", "4k"] }),
        negative_prompt: true,
        seed: true,
        end_frame: true,
        max_images: if lite { 1 } else { 3 },
        audio: true,
        price: Some(price.into()),
        description: Some("Retiring 2026-10-22 (replaced by Gemini Omni). 1080p/4K and references force 8 s.".into()),
        ..ModelInfo::new(ID, id, name, &[Task::TextToVideo, Task::ImageToVideo])
    }
}

fn catalog() -> Vec<ModelInfo> {
    vec![
        image_model(
            "gemini-3.1-flash-image",
            "Nano Banana 2 (Gemini 3.1 Flash Image)",
            FLASH_RATIOS,
            &["512", "1K", "2K", "4K"],
            "$0.045–0.15 / image",
            "Fast, high-quality generation and multi-image edits up to 4K.",
        ),
        image_model(
            "gemini-3-pro-image",
            "Nano Banana Pro (Gemini 3 Pro Image)",
            STANDARD_RATIOS,
            &["1K", "2K", "4K"],
            "$0.134–0.24 / image",
            "Google's most capable image model: reasoning, text rendering, 4K.",
        ),
        image_model(
            "gemini-3.1-flash-lite-image",
            "Nano Banana 2 Lite",
            STANDARD_RATIOS,
            &["1K"],
            "$0.034 / image",
            "Cheapest Gemini image model, 1K only.",
        ),
        ModelInfo {
            aspect_ratios: strings(VIDEO_RATIOS),
            resolutions: strings(&["360p", "720p", "1080p", "4k"]),
            end_frame: true,
            max_images: 3,
            audio: true,
            price: Some("≈ $0.10 / s".into()),
            featured: true,
            description: Some(
                "Gemini's video model with native audio; start/end frames and subject references.".into(),
            ),
            ..ModelInfo::new(ID, "gemini-omni-1.1-flash", "Gemini Omni Flash", &[Task::TextToVideo, Task::ImageToVideo])
        },
        veo_model("veo-3.1-generate-preview", "Veo 3.1", false, "$0.40–0.60 / s"),
        veo_model("veo-3.1-fast-generate-preview", "Veo 3.1 Fast", false, "$0.10–0.30 / s"),
        veo_model("veo-3.1-lite-generate-preview", "Veo 3.1 Lite", true, "$0.05–0.08 / s"),
    ]
}

fn model_id(req: &GenRequest) -> &str {
    req.model.strip_prefix("models/").unwrap_or(&req.model)
}

fn inline(img: &InputImage) -> Value {
    json!({ "inlineData": { "mimeType": img.mime, "data": img.base64() } })
}

/// Runs `fut`, reporting estimated progress every couple of seconds while it
/// blocks (for calls that hold the connection open until the result is ready).
async fn ticking<T>(cx: &Ctx, expected: Duration, msg: &str, fut: impl Future<Output = T>) -> T {
    let started = Instant::now();
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            r = &mut fut => return r,
            _ = tick.tick() => util::estimate(cx, started, expected, msg),
        }
    }
}

fn refusal(reason: &str, text: Option<String>) -> GenError {
    let detail = text.filter(|t| !t.is_empty()).map(|t| format!(" ({})", util::truncate(&t, 200))).unwrap_or_default();
    GenError::Moderated(format!("Gemini blocked this request: {reason}{detail}"))
}

/// Pulls images out of a `generateContent` response, mapping safety blocks
/// to [`GenError::Moderated`].
fn parse_content(cx: &Ctx, v: &Value) -> GenResult<Vec<OutputItem>> {
    if let Some(reason) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        return Err(refusal(reason, None));
    }
    let mut items = vec![];
    let mut text = String::new();
    let mut finish = None;
    for cand in v.get("candidates").and_then(Value::as_array).into_iter().flatten() {
        finish = finish.or(cand.get("finishReason").and_then(Value::as_str));
        for part in cand.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
            // Gemini 3 models stream interim "thought" images; skip them.
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if let Some(blob) = part.get("inlineData").or_else(|| part.get("inline_data")) {
                let data = blob.get("data").and_then(Value::as_str).unwrap_or_default();
                let bytes = util::b64_decode(data).map_err(|e| util::decode_err(cx, format!("bad image data: {e}")))?;
                let mime = blob
                    .get("mimeType")
                    .or_else(|| blob.get("mime_type"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| util::sniff_mime(&bytes).map(str::to_string))
                    .unwrap_or_else(|| "image/png".into());
                items.push(OutputItem::bytes(OutputKind::Image, bytes, mime));
            } else if let Some(t) = part.get("text").and_then(Value::as_str) {
                text.push_str(t);
            }
        }
    }
    if items.is_empty() {
        if let Some(reason) = finish.filter(|r| BLOCKED.contains(r)) {
            return Err(refusal(reason, Some(text)));
        }
        let msg = if text.trim().is_empty() {
            format!("Gemini returned no image (finish reason: {}).", finish.unwrap_or("unknown"))
        } else {
            format!("Gemini answered with text instead of an image: {}", util::truncate(text.trim(), 300))
        };
        return Err(GenError::Provider(msg));
    }
    Ok(items)
}

/// File id from `files/abc`, `…/v1beta/files/abc:download?alt=media`, etc.
fn file_id(uri: &str) -> Option<&str> {
    let rest = &uri[uri.find("files/")? + "files/".len()..];
    let end = rest.find([':', '?', '/']).unwrap_or(rest.len());
    Some(&rest[..end]).filter(|s| !s.is_empty())
}

fn keyed(url: String, key: &str) -> OutputItem {
    OutputItem {
        kind: OutputKind::Video,
        source: OutputSource::Url { url, headers: vec![(KEY_HEADER.into(), key.into())] },
    }
}

impl Google {
    async fn image(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        let id = model_id(req);
        let ratios = if id.contains("flash-image") && !id.contains("lite") { FLASH_RATIOS } else { STANDARD_RATIOS };
        let mut parts: Vec<Value> = req.references().take(14).map(inline).collect();
        parts.push(json!({ "text": req.prompt }));
        let mut image_config = json!({ "aspectRatio": util::closest_ratio(&req.aspect(), ratios) });
        if let Some(size) = req.resolution.as_deref().map(str::to_ascii_uppercase) {
            // The API wants an uppercase K; Lite only does 1K.
            if !id.contains("lite") && ["512", "1K", "2K", "4K"].contains(&size.as_str()) {
                image_config["imageSize"] = json!(size);
            }
        }
        let body = json!({
            "contents": [{ "role": "user", "parts": parts }],
            "generationConfig": { "responseModalities": ["TEXT", "IMAGE"], "imageConfig": image_config },
        });
        let url = cx.url(&format!("/models/{id}:generateContent"));
        cx.report(Progress::message("Generating"));

        // One image per call: fire `count` calls at once.
        let calls = (0..req.count.clamp(1, 4)).map(|_| async {
            let v: Value = util::send_json(cx, cx.http.post(&url).header(KEY_HEADER, key).json(&body)).await?;
            parse_content(cx, &v)
        });
        let items = futures::future::try_join_all(calls).await?.into_iter().flatten().collect();
        Ok(GenOutput { items, ..Default::default() })
    }

    async fn omni(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        let mut input: Vec<Value> = vec![];
        let image = |img: &InputImage| json!({ "type": "image", "mime_type": img.mime, "data": img.base64() });
        let start = (req.task == Task::ImageToVideo).then(|| req.start_frame()).flatten();
        if let Some(start) = start {
            // Leading images are read as first (and last) frame; references follow.
            input.push(image(start));
            input.extend(req.image(ImageRole::EndFrame).map(image));
        }
        let refs =
            req.images.iter().filter(|i| i.role == ImageRole::Reference && !start.is_some_and(|s| std::ptr::eq(s, *i)));
        input.extend(refs.take(3).map(image));
        input.push(json!({ "type": "text", "text": req.prompt }));
        let resolution = match req.resolution.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some(r) if r.contains("360") => "360p",
            Some(r) if r.contains("1080") => "1080p",
            Some(r) if r.contains("4k") || r.contains("2160") => "4k",
            _ => "720p",
        };
        let body = json!({
            "model": model_id(req),
            "input": input,
            "response_format": {
                "type": "video",
                "aspect_ratio": util::closest_ratio(&req.aspect(), VIDEO_RATIOS),
                "resolution": resolution,
                "delivery": "uri",
            },
        });

        let mut out = GenOutput::default();
        for i in 0..req.count.max(1) {
            let msg = if req.count > 1 { format!("Video {}/{}", i + 1, req.count) } else { "Rendering".into() };
            let call = cx.http.post(cx.url("/interactions")).header(KEY_HEADER, key).timeout(Duration::from_secs(900));
            let v: Value = ticking(cx, Duration::from_secs(120), &msg, util::send_json(cx, call.json(&body))).await?;
            out.items.push(self.omni_video(cx, key, &v).await?);
        }
        Ok(out)
    }

    /// Finds the video in an interaction and waits for its file to be ready.
    async fn omni_video(&self, cx: &Ctx, key: &str, v: &Value) -> GenResult<OutputItem> {
        if let Some(status) = v.get("status").and_then(Value::as_str).filter(|s| *s == "failed" || *s == "cancelled") {
            let msg = util::error_message(&v.to_string());
            return Err(GenError::Provider(format!("Gemini Omni {status}: {msg}")));
        }
        let mut videos = v.get("output_video").into_iter().cloned().collect::<Vec<_>>();
        let mut text = String::new();
        for step in v.get("steps").and_then(Value::as_array).into_iter().flatten() {
            if step.get("type").and_then(Value::as_str) != Some("model_output") {
                continue;
            }
            for c in step.get("content").and_then(Value::as_array).into_iter().flatten() {
                match c.get("type").and_then(Value::as_str) {
                    Some("video") => videos.push(c.clone()),
                    Some("text") => text.push_str(c.get("text").and_then(Value::as_str).unwrap_or_default()),
                    _ => {}
                }
            }
        }
        let Some(video) = videos.into_iter().next() else {
            let lower = text.to_ascii_lowercase();
            if ["safety", "policy", "can't", "cannot", "unable"].iter().any(|k| lower.contains(k)) {
                return Err(refusal("the model declined", Some(text)));
            }
            return Err(GenError::Provider(if text.trim().is_empty() {
                "Gemini Omni returned no video.".into()
            } else {
                format!("Gemini Omni answered with text instead of a video: {}", util::truncate(text.trim(), 300))
            }));
        };
        if let Some(data) = video.get("data").and_then(Value::as_str) {
            let bytes = util::b64_decode(data).map_err(|e| util::decode_err(cx, format!("bad video data: {e}")))?;
            let mime = video.get("mime_type").and_then(Value::as_str).unwrap_or("video/mp4").to_string();
            return Ok(OutputItem::bytes(OutputKind::Video, bytes, mime));
        }
        let uri = video.get("uri").and_then(Value::as_str).unwrap_or_default();
        let id = file_id(uri).ok_or_else(|| util::decode_err(cx, format!("video has no usable uri: `{uri}`")))?;
        let file_url = cx.url(&format!("/files/{id}"));
        util::poll(Duration::from_secs(3), Duration::from_secs(60 * 10), || async {
            let f: Value = util::send_json(cx, cx.http.get(&file_url).header(KEY_HEADER, key)).await?;
            match f.get("state").and_then(Value::as_str).unwrap_or("PROCESSING") {
                "ACTIVE" => Ok(Some(())),
                "FAILED" => Err(GenError::Provider("Gemini Omni failed to finish the video file.".into())),
                _ => {
                    cx.report(Progress::fraction(0.96, "Finishing"));
                    Ok(None)
                }
            }
        })
        .await?;
        Ok(keyed(cx.url(&format!("/files/{id}:download?alt=media")), key))
    }

    async fn veo(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        let id = model_id(req);
        let lite = id.contains("lite");
        let mut instance = json!({ "prompt": req.prompt });
        if req.task == Task::ImageToVideo
            && let Some(start) = req.start_frame()
        {
            instance["image"] = inline(start);
            if let Some(end) = req.image(ImageRole::EndFrame) {
                instance["lastFrame"] = inline(end);
            }
        }
        let refs: Vec<Value> = req
            .images
            .iter()
            .filter(|i| i.role == ImageRole::Reference && !lite)
            .take(3)
            .map(|i| json!({ "image": inline(i), "referenceType": "asset" }))
            .collect();
        let resolution = match req.resolution.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some(r) if (r.contains("4k") || r.contains("2160")) && !lite => "4k",
            Some(r) if r.contains("1080") => "1080p",
            _ => "720p",
        };
        // 1080p, 4K and reference images only render 8 s clips.
        let duration = if resolution != "720p" || !refs.is_empty() {
            8
        } else {
            util::closest_duration(req.duration, VEO_SECONDS, 8.0) as u32
        };
        if !refs.is_empty() {
            instance["referenceImages"] = json!(refs);
        }
        let mut parameters = json!({
            "aspectRatio": util::closest_ratio(&req.aspect(), VIDEO_RATIOS),
            "durationSeconds": duration,
            "resolution": resolution,
        });
        if let Some(neg) = req.negative_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
            parameters["negativePrompt"] = json!(neg);
        }
        if let Some(seed) = req.seed {
            parameters["seed"] = json!(seed);
        }
        let body = json!({ "instances": [instance], "parameters": parameters });
        let url = cx.url(&format!("/models/{id}:predictLongRunning"));

        // Veo renders one clip per operation; start them all, then wait.
        let started = Instant::now();
        let jobs = (0..req.count.clamp(1, 4)).map(|_| async {
            let op: Value = util::send_json(cx, cx.http.post(&url).header(KEY_HEADER, key).json(&body)).await?;
            let name = op
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| util::decode_err(cx, "operation has no name"))?
                .to_string();
            let op_url = cx.url(&format!("/{name}"));
            util::poll(Duration::from_secs(5), Duration::from_secs(60 * 15), || async {
                let v: Value = util::send_json(cx, cx.http.get(&op_url).header(KEY_HEADER, key)).await?;
                if v.get("done").and_then(Value::as_bool) != Some(true) {
                    util::estimate(cx, started, Duration::from_secs(90), "Rendering");
                    return Ok(None);
                }
                veo_result(cx, key, &v).map(Some)
            })
            .await
        });
        let items = futures::future::try_join_all(jobs).await?.into_iter().flatten().collect();
        Ok(GenOutput { items, ..Default::default() })
    }
}

fn veo_result(cx: &Ctx, key: &str, v: &Value) -> GenResult<Vec<OutputItem>> {
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(Value::as_str).unwrap_or("Veo failed").to_string();
        let lower = msg.to_ascii_lowercase();
        return Err(if ["safety", "policy", "blocked", "responsible ai"].iter().any(|k| lower.contains(k)) {
            GenError::Moderated(msg)
        } else {
            GenError::Provider(msg)
        });
    }
    let resp = v.pointer("/response/generateVideoResponse").unwrap_or(&Value::Null);
    let items: Vec<OutputItem> = resp
        .get("generatedSamples")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.pointer("/video/uri").and_then(Value::as_str))
        .map(|uri| keyed(uri.to_string(), key))
        .collect();
    if items.is_empty() {
        let reasons: Vec<&str> = resp
            .get("raiMediaFilteredReasons")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !reasons.is_empty() || resp.get("raiMediaFilteredCount").and_then(Value::as_u64).unwrap_or(0) > 0 {
            return Err(GenError::Moderated(if reasons.is_empty() {
                "Veo filtered the generated video.".into()
            } else {
                reasons.join(" ")
            }));
        }
        return Err(util::decode_err(cx, "finished operation has no video"));
    }
    Ok(items)
}

#[async_trait]
impl Provider for Google {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Google Gemini".into(),
            kind: ProviderKind::Cloud,
            tagline: "Nano Banana images, Gemini Omni and Veo video".into(),
            website: "https://ai.google.dev".into(),
            needs_key: true,
            key_env: vec!["GEMINI_API_KEY".into(), "GOOGLE_API_KEY".into()],
            key_url: Some("https://aistudio.google.com/apikey".into()),
            key_hint: Some("AIza…".into()),
            default_base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(catalog())
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(
            cx,
            cx.http.get(cx.url("/models")).query(&[("pageSize", "1000")]).header(KEY_HEADER, cx.key()?),
        )
        .await?;
        let names: Vec<&str> = v
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("name").and_then(Value::as_str))
            .collect();
        let ours = catalog().into_iter().filter(|m| names.iter().any(|n| n.ends_with(&m.id))).count();
        Ok(format!("Key works · {ours} image/video models available"))
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        match req.task {
            Task::TextToImage | Task::ImageToImage => self.image(cx, req).await,
            Task::TextToVideo | Task::ImageToVideo if model_id(req).starts_with("veo") => self.veo(cx, req).await,
            Task::TextToVideo | Task::ImageToVideo => self.omni(cx, req).await,
        }
    }
}
