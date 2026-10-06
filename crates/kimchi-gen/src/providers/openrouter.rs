//! OpenRouter: one key for image and video models from every lab.
//!
//! * Images: `POST /images` (OpenAI-style body plus `aspect_ratio`,
//!   `resolution`, `input_references`), answered synchronously with base64.
//! * Video: `POST /videos` → `{id}`, poll `GET /videos/{id}` until
//!   `completed`, then download `GET /videos/{id}/content?index=N` with the key.
//! * Catalog: `GET /images/models` and `GET /videos/models` describe every
//!   model's supported values, which we turn into [`ModelInfo`] capabilities
//!   and also use at generation time to snap requests to valid values.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "openrouter";

pub struct OpenRouter;

/// Shown first in pickers, with a price hint where the listing has none.
const FEATURED: &[(&str, Option<&str>)] = &[
    ("google/gemini-3.1-flash-image", Some("≈ $0.07 / image")),
    ("google/gemini-3-pro-image", Some("≈ $0.13 / image")),
    ("openai/gpt-image-2.5-sunburst", Some("$30 / 1M output tokens")),
    ("black-forest-labs/flux.2-pro", Some("$0.03 / MP")),
    ("bytedance-seed/seedream-4.5", Some("$0.04 / image")),
    ("google/veo-3.1", None),
    ("google/veo-3.1-fast", None),
    ("kwaivgi/kling-v3.0-pro", None),
    ("bytedance/seedance-2.0", None),
    ("x-ai/grok-imagine-video-1.5", None),
];

/// A slice of the live listings, used when they can't be fetched.
const FALLBACK_IMAGES: &str = r#"[
{"id":"google/gemini-3.1-flash-image","name":"Google: Nano Banana 2 (Gemini 3.1 Flash Image)","supported_parameters":{"resolution":{"type":"enum","values":["512","1K","2K","4K"]},"aspect_ratio":{"type":"enum","values":["1:1","1:4","1:8","2:3","3:2","3:4","4:1","4:3","4:5","5:4","8:1","9:16","16:9","21:9"]},"n":{"type":"range","min":1,"max":1},"input_references":{"type":"range","min":0,"max":14}}},
{"id":"google/gemini-3-pro-image","name":"Google: Nano Banana Pro (Gemini 3 Pro Image)","supported_parameters":{"resolution":{"type":"enum","values":["1K","2K","4K"]},"aspect_ratio":{"type":"enum","values":["1:1","2:3","3:2","3:4","4:3","4:5","5:4","9:16","16:9","21:9"]},"n":{"type":"range","min":1,"max":1},"input_references":{"type":"range","min":0,"max":14}}},
{"id":"openai/gpt-image-2.5-sunburst","name":"OpenAI: GPT Image 2.5 Sunburst","supported_parameters":{"aspect_ratio":{"type":"enum","values":["1:1","3:2","2:3","4:3","3:4","16:9","9:16","21:9","auto"]},"quality":{"type":"enum","values":["auto","low","medium","high","xhigh","max"]},"background":{"type":"enum","values":["auto","transparent","opaque"]},"n":{"type":"range","min":1,"max":10},"input_references":{"type":"range","min":0,"max":16}}},
{"id":"black-forest-labs/flux.2-pro","name":"Black Forest Labs: FLUX.2 Pro","supported_parameters":{"aspect_ratio":{"type":"enum","values":["1:1","4:3","3:4","3:2","2:3","16:9","9:16","21:9","auto"]},"n":{"type":"range","min":1,"max":1},"input_references":{"type":"range","min":0,"max":8},"seed":{"type":"boolean"}}},
{"id":"bytedance-seed/seedream-4.5","name":"ByteDance Seed: Seedream 4.5","supported_parameters":{"resolution":{"type":"enum","values":["1K","2K","4K"]},"aspect_ratio":{"type":"enum","values":["1:1","2:3","3:2","3:4","4:3","4:5","5:4","9:16","16:9","9:21","21:9","auto"]},"n":{"type":"range","min":1,"max":10},"input_references":{"type":"range","min":0,"max":14},"seed":{"type":"boolean"}}}
]"#;
const FALLBACK_VIDEOS: &str = r#"[
{"id":"google/veo-3.1","name":"Google: Veo 3.1","supported_durations":[4,6,8],"supported_resolutions":["720p","1080p","4K"],"supported_aspect_ratios":["16:9","9:16"],"supported_frame_images":["first_frame","last_frame"],"generate_audio":true,"seed":true,"pricing_skus":{"duration_seconds_with_audio":"0.40","duration_seconds_with_audio_4k":"0.60","duration_seconds_without_audio":"0.20"}},
{"id":"google/veo-3.1-fast","name":"Google: Veo 3.1 Fast","supported_durations":[4,6,8],"supported_resolutions":["720p","1080p","4K"],"supported_aspect_ratios":["16:9","9:16"],"supported_frame_images":["first_frame","last_frame"],"generate_audio":true,"seed":true,"pricing_skus":{"duration_seconds_with_audio":"0.12","duration_seconds_with_audio_4k":"0.30","duration_seconds_without_audio_720p":"0.08"}},
{"id":"kwaivgi/kling-v3.0-pro","name":"Kling: Video v3.0 Pro","supported_durations":[3,4,5,6,7,8,9,10,11,12,13,14,15],"supported_resolutions":["720p"],"supported_aspect_ratios":["16:9","9:16","1:1"],"supported_frame_images":["first_frame","last_frame"],"generate_audio":true,"seed":false,"pricing_skus":{"duration_seconds":"0.112","duration_seconds_with_audio":"0.168"}},
{"id":"bytedance/seedance-2.0","name":"ByteDance: Seedance 2.0","supported_durations":[4,5,6,7,8,9,10,11,12,13,14,15],"supported_resolutions":["480p","720p","1080p","4K"],"supported_aspect_ratios":["1:1","3:4","9:16","4:3","16:9","21:9","9:21"],"supported_frame_images":["first_frame","last_frame"],"generate_audio":true,"seed":true,"pricing_skus":{}},
{"id":"x-ai/grok-imagine-video-1.5","name":"xAI: Grok Imagine Video 1.5","supported_durations":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15],"supported_resolutions":["480p","720p","1080p"],"supported_aspect_ratios":["16:9","9:16","1:1","4:3","3:4","3:2","2:3"],"supported_frame_images":["first_frame"],"generate_audio":null,"seed":null,"pricing_skus":{"cents_per_video_output_second_480p":"8","cents_per_video_output_second_1080p":"25"}}
]"#;

/// Aspect ratios `/images` accepts when we know nothing about the model.
const ANY_RATIO: &[&str] = &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "4:5", "5:4", "21:9", "9:21"];

// ---- listing → ModelInfo -------------------------------------------------

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .collect()
}

/// `supported_parameters.<key>` of an image model.
fn sp<'a>(m: &'a Value, key: &str) -> Option<&'a Value> {
    m.get("supported_parameters").and_then(|p| p.get(key))
}

fn enum_values(m: &Value, key: &str) -> Vec<String> {
    strs(sp(m, key).and_then(|p| p.get("values")))
}

fn range(m: &Value, key: &str) -> Option<(u64, u64)> {
    let p = sp(m, key)?;
    Some((p.get("min").and_then(Value::as_u64)?, p.get("max").and_then(Value::as_u64)?))
}

fn display_name(m: &Value, id: &str) -> String {
    let name = m.get("name").and_then(Value::as_str).unwrap_or(id);
    // "Google: Veo 3.1" → "Veo 3.1"; the provider is obvious from context.
    name.split_once(": ").map_or(name, |(_, n)| n).to_string()
}

fn featured(id: &str) -> Option<Option<&'static str>> {
    FEATURED.iter().find(|(f, _)| *f == id).map(|(_, p)| *p)
}

fn describe(m: &Value) -> Option<String> {
    m.get("description").and_then(Value::as_str).filter(|d| !d.is_empty()).map(|d| util::truncate(d, 240))
}

fn select_param(key: &str, label: &str, values: Vec<String>) -> Option<ParamSpec> {
    (!values.is_empty()).then(|| ParamSpec {
        key: key.into(),
        label: label.into(),
        default: json!(if values.iter().any(|v| v == "auto") { "auto" } else { values[0].as_str() }),
        kind: ParamKind::Select {
            options: values.iter().map(|v| SelectOption::new(v.as_str(), capitalize(v))).collect(),
        },
        help: None,
    })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn image_info(m: &Value) -> Option<ModelInfo> {
    let id = m.get("id").and_then(Value::as_str)?;
    let refs = range(m, "input_references");
    let mut tasks = vec![];
    if refs.is_none_or(|(min, _)| min == 0) {
        tasks.push(Task::TextToImage);
    }
    if refs.is_some_and(|(_, max)| max > 0) {
        tasks.push(Task::ImageToImage);
    }
    // Vector-only (SVG) models can't be placed on a video timeline.
    if tasks.is_empty() || enum_values(m, "output_format") == ["svg"] {
        return None;
    }
    let feat = featured(id);
    Some(ModelInfo {
        description: describe(m),
        aspect_ratios: enum_values(m, "aspect_ratio").into_iter().filter(|r| r != "auto").collect(),
        resolutions: enum_values(m, "resolution"),
        max_outputs: range(m, "n").map_or(1, |(_, max)| max.clamp(1, 10) as u32),
        seed: sp(m, "seed").is_some(),
        max_images: refs.map_or(0, |(_, max)| max as u32),
        params: [
            select_param("quality", "Quality", enum_values(m, "quality")),
            select_param("background", "Background", enum_values(m, "background")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        price: feat.flatten().map(str::to_string),
        featured: feat.is_some(),
        ..ModelInfo::new(ID, id, display_name(m, id), &tasks)
    })
}

/// "$0.10–0.40 / s" from per-second SKUs (dollars, or cents when the key says so).
fn video_price(skus: Option<&Value>) -> Option<String> {
    let skus = skus?.as_object()?;
    let per_second: Vec<f64> = skus
        .iter()
        .filter(|(k, _)| {
            k.contains("second") && !["reference", "continuation", "image_input"].iter().any(|x| k.contains(x))
        })
        .filter_map(|(k, v)| {
            let n = v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64())?;
            Some(if k.starts_with("cents") { n / 100.0 } else { n })
        })
        .collect();
    let lo = per_second.iter().copied().reduce(f64::min)?;
    let hi = per_second.iter().copied().reduce(f64::max)?;
    Some(if (hi - lo).abs() < 1e-9 { format!("${lo:.2} / s") } else { format!("${lo:.2}–{hi:.2} / s") })
}

fn video_info(m: &Value) -> Option<ModelInfo> {
    let id = m.get("id").and_then(Value::as_str)?;
    let durations: Vec<f64> =
        m.get("supported_durations").and_then(Value::as_array)?.iter().filter_map(Value::as_f64).collect();
    // No durations means an edit/upscale/avatar model that needs a video or audio input.
    if durations.is_empty() {
        return None;
    }
    let frames = strs(m.get("supported_frame_images"));
    let mut tasks = vec![Task::TextToVideo];
    if frames.iter().any(|f| f == "first_frame") {
        tasks.push(Task::ImageToVideo);
    }
    let mut durations = durations;
    durations.sort_by(f64::total_cmp);
    Some(ModelInfo {
        description: describe(m),
        aspect_ratios: strs(m.get("supported_aspect_ratios")),
        durations,
        resolutions: strs(m.get("supported_resolutions")),
        seed: m.get("seed").and_then(Value::as_bool).unwrap_or(false),
        end_frame: frames.iter().any(|f| f == "last_frame"),
        audio: m.get("generate_audio").and_then(Value::as_bool).unwrap_or(false),
        price: video_price(m.get("pricing_skus")),
        featured: featured(id).is_some(),
        ..ModelInfo::new(ID, id, display_name(m, id), &tasks)
    })
}

fn parse_list(s: &str) -> Vec<Value> {
    serde_json::from_str(s).unwrap_or_default()
}

// ---- requests --------------------------------------------------------------

/// Sends a request and decodes JSON. Unlike [`util::send_json`], a 403 is
/// inspected first: OpenRouter uses it for moderation blocks as well as bad keys.
async fn call(cx: &Ctx, req: reqwest::RequestBuilder) -> GenResult<Value> {
    let resp = req.send().await.map_err(util::net_err(cx))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(util::net_err(cx))?;
    if (200..300).contains(&status) {
        return serde_json::from_str(&text)
            .map_err(|e| util::decode_err(cx, format!("{e}: {}", util::truncate(&text, 300))));
    }
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let message = util::error_message(&text);
    let lower = text.to_ascii_lowercase();
    let flagged = body.pointer("/error/metadata/reasons").is_some()
        || ["moderation", "flagged", "content policy", "safety"].iter().any(|k| lower.contains(k));
    match status {
        400 | 403 | 422 if flagged => {
            let reasons = strs(body.pointer("/error/metadata/reasons"));
            Err(GenError::Moderated(if reasons.is_empty() {
                message
            } else {
                format!("{message} ({})", reasons.join(", "))
            }))
        }
        401 | 403 => Err(GenError::Unauthorized { provider: cx.provider.clone(), status, message }),
        _ => Err(GenError::Http { provider: cx.provider.clone(), status, message }),
    }
}

fn authed(cx: &Ctx, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match cx.api_key.as_deref() {
        Some(k) => req.bearer_auth(k),
        None => req,
    }
}

/// The listing entries at `path` (`/images/models` or `/videos/models`).
async fn listing(cx: &Ctx, path: &str) -> GenResult<Vec<Value>> {
    let v = call(cx, authed(cx, cx.http.get(cx.url(path)))).await?;
    Ok(v.get("data").and_then(Value::as_array).cloned().unwrap_or_default())
}

/// The listing entry for one model, if we can get it. Generation still works
/// without it; values are then passed through unchecked.
async fn spec(cx: &Ctx, path: &str, model: &str) -> Option<Value> {
    listing(cx, path).await.ok()?.into_iter().find(|m| m.get("id").and_then(Value::as_str) == Some(model))
}

fn image_ref(img: &InputImage) -> Value {
    json!({ "type": "image_url", "image_url": { "url": img.data_url() } })
}

/// Picks the allowed resolution matching `wanted` (case-insensitive), else
/// the closest one, comparing approximate long edges so `"1080p"`, `"2K"` and
/// `"512"` styles mix (`2160p` ≈ `4K`, `1080p` ≈ `2K`, `720p` ≈ `1K`).
fn closest_resolution(wanted: &str, allowed: &[String]) -> Option<String> {
    if let Some(r) = allowed.iter().find(|r| r.eq_ignore_ascii_case(wanted)) {
        return Some(r.clone());
    }
    let num = |s: &str| -> Option<f64> {
        let lower = s.to_ascii_lowercase();
        let digits: String = lower.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let n: f64 = digits.parse().ok()?;
        Some(match lower.chars().last() {
            Some('k') => n * 1000.0,
            Some('p') => n * 16.0 / 9.0,
            _ => n,
        })
    };
    let w = num(wanted)?;
    allowed
        .iter()
        .filter_map(|r| Some((r, (num(r)? - w).abs())))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, _)| r.clone())
}

impl OpenRouter {
    async fn image(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        cx.report(Progress::message("Generating"));
        let m = spec(cx, "/images/models", &req.model).await;
        let has = |k: &str| m.as_ref().is_none_or(|m| sp(m, k).is_some());

        let ratios = m.as_ref().map(|m| enum_values(m, "aspect_ratio")).unwrap_or_default();
        let ratios: Vec<&str> = ratios.iter().map(String::as_str).filter(|r| *r != "auto").collect();
        let mut body = json!({ "model": req.model, "prompt": req.prompt });
        if has("aspect_ratio") {
            let allowed = if ratios.is_empty() { ANY_RATIO } else { &ratios };
            body["aspect_ratio"] = json!(util::closest_ratio(&req.aspect(), allowed));
        }
        if let Some(r) = req.resolution.as_deref() {
            match &m {
                Some(m) => {
                    if let Some(r) = closest_resolution(r, &enum_values(m, "resolution")) {
                        body["resolution"] = json!(r);
                    }
                }
                None => body["resolution"] = json!(r),
            }
        }
        if let (Some(seed), true) = (req.seed, has("seed")) {
            body["seed"] = json!(seed);
        }
        for k in ["quality", "background"] {
            if let (Some(v), true) = (req.param(k).and_then(Value::as_str), has(k)) {
                body[k] = json!(v);
            }
        }
        let max_refs = m.as_ref().and_then(|m| range(m, "input_references")).map_or(16, |(_, max)| max as usize);
        let refs: Vec<Value> = req.references().take(max_refs).map(image_ref).collect();
        if !refs.is_empty() {
            body["input_references"] = json!(refs);
        }

        // Ask for as many images per call as the model allows; run calls in parallel.
        let per_call = m.as_ref().and_then(|m| range(m, "n")).map_or(1, |(_, max)| max.max(1) as u32);
        let mut batches = vec![];
        let mut left = req.count.max(1);
        while left > 0 {
            let n = left.min(per_call);
            batches.push(n);
            left -= n;
        }
        let calls = batches.into_iter().map(|n| {
            let mut body = body.clone();
            body["n"] = json!(n);
            async move { call(cx, cx.http.post(cx.url("/images")).bearer_auth(key).json(&body)).await }
        });
        let mut out = GenOutput::default();
        for v in futures::future::try_join_all(calls).await? {
            for d in v.get("data").and_then(Value::as_array).into_iter().flatten() {
                let Some(b) = d.get("b64_json").and_then(Value::as_str) else { continue };
                let bytes = util::b64_decode(b).map_err(|e| util::decode_err(cx, format!("bad base64: {e}")))?;
                let mime = d
                    .get("media_type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| util::sniff_mime(&bytes).map(str::to_string))
                    .unwrap_or_else(|| "image/png".into());
                out.items.push(OutputItem::bytes(OutputKind::Image, bytes, mime));
            }
            if let Some(c) = v.pointer("/usage/cost").and_then(Value::as_f64) {
                out.cost_usd = Some(out.cost_usd.unwrap_or(0.0) + c);
            }
        }
        Ok(out)
    }

    async fn video(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        cx.report(Progress::message("Submitting"));
        let m = spec(cx, "/videos/models", &req.model).await;
        let list = |k: &str| m.as_ref().map(|m| strs(m.get(k))).unwrap_or_default();

        let mut body = json!({ "model": req.model, "prompt": req.prompt });
        let durations: Vec<f64> = list("supported_durations").iter().filter_map(|d| d.parse().ok()).collect();
        if let Some(d) = req.duration {
            let d = if durations.is_empty() { d.round() } else { util::closest_duration(Some(d), &durations, d) };
            body["duration"] = json!(d as u32);
        }
        let ratios = list("supported_aspect_ratios");
        let ratios: Vec<&str> = ratios.iter().map(String::as_str).collect();
        if !(req.task == Task::ImageToVideo && req.aspect_ratio.is_none()) {
            body["aspect_ratio"] = json!(if ratios.is_empty() {
                req.aspect()
            } else {
                util::closest_ratio(&req.aspect(), &ratios).to_string()
            });
        }
        if let Some(r) = req.resolution.as_deref() {
            let allowed = list("supported_resolutions");
            let r = if allowed.is_empty() { Some(r.to_string()) } else { closest_resolution(r, &allowed) };
            if let Some(r) = r {
                body["resolution"] = json!(r);
            }
        }
        let flag = |k: &str| m.as_ref().is_none_or(|m| m.get(k).and_then(Value::as_bool) != Some(false));
        if let (Some(seed), true) = (req.seed, flag("seed")) {
            body["seed"] = json!(seed);
        }
        if let (Some(audio), true) = (req.audio, flag("generate_audio")) {
            body["generate_audio"] = json!(audio);
        }
        let frames = list("supported_frame_images");
        let frame_ok = |f: &str| m.is_none() || frames.iter().any(|x| x == f);
        let start = (req.task == Task::ImageToVideo).then(|| req.start_frame()).flatten();
        if let Some(start) = start {
            let mut frame_images = vec![];
            if frame_ok("first_frame") {
                frame_images.push(json!({ "type": "image_url", "image_url": { "url": start.data_url() }, "frame_type": "first_frame" }));
            }
            if let Some(end) = req.image(ImageRole::EndFrame).filter(|_| frame_ok("last_frame")) {
                frame_images.push(
                    json!({ "type": "image_url", "image_url": { "url": end.data_url() }, "frame_type": "last_frame" }),
                );
            }
            if frame_images.is_empty() {
                return Err(GenError::Unsupported(format!("{} can't start from an image", req.model)));
            }
            body["frame_images"] = json!(frame_images);
        } else {
            let refs: Vec<Value> = req.references().map(image_ref).collect();
            if !refs.is_empty() {
                body["input_references"] = json!(refs);
            }
        }

        // One clip per job; run `count` jobs side by side.
        let started = Instant::now();
        let jobs = (0..req.count.max(1)).map(|_| self.video_job(cx, key, &body, started));
        let mut out = GenOutput::default();
        for (items, cost) in futures::future::try_join_all(jobs).await? {
            out.items.extend(items);
            if let Some(c) = cost {
                out.cost_usd = Some(out.cost_usd.unwrap_or(0.0) + c);
            }
        }
        Ok(out)
    }

    async fn video_job(
        &self,
        cx: &Ctx,
        key: &str,
        body: &Value,
        started: Instant,
    ) -> GenResult<(Vec<OutputItem>, Option<f64>)> {
        let created = call(cx, cx.http.post(cx.url("/videos")).bearer_auth(key).json(body)).await?;
        let id = created
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| util::decode_err(cx, "video job has no id"))?
            .to_string();
        let status_url = cx.url(&format!("/videos/{id}"));
        let done = util::poll(Duration::from_secs(5), Duration::from_secs(60 * 30), || async {
            let v = call(cx, cx.http.get(&status_url).bearer_auth(key)).await?;
            match v.get("status").and_then(Value::as_str).unwrap_or("pending") {
                "completed" => Ok(Some(v)),
                "failed" => {
                    let msg = v
                        .get("error")
                        .map(|e| {
                            e.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| util::error_message(&json!({ "error": e }).to_string()))
                        })
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| "The video generation failed.".into());
                    let lower = msg.to_ascii_lowercase();
                    Err(if ["moderation", "safety", "policy", "flagged"].iter().any(|k| lower.contains(k)) {
                        GenError::Moderated(msg)
                    } else {
                        GenError::Provider(msg)
                    })
                }
                s @ ("cancelled" | "expired") => Err(GenError::Provider(format!("The video job was {s}."))),
                s => {
                    util::estimate(
                        cx,
                        started,
                        Duration::from_secs(150),
                        if s == "pending" { "In queue" } else { "Rendering" },
                    );
                    Ok(None)
                }
            }
        })
        .await?;
        // `unsigned_urls` point at the content endpoint, one per output.
        let n = done.get("unsigned_urls").and_then(Value::as_array).map_or(1, |u| u.len().max(1));
        let items = (0..n)
            .map(|i| OutputItem {
                kind: OutputKind::Video,
                source: OutputSource::Url {
                    url: cx.url(&format!("/videos/{id}/content?index={i}")),
                    headers: vec![("Authorization".into(), format!("Bearer {key}"))],
                },
            })
            .collect();
        Ok((items, done.pointer("/usage/cost").and_then(Value::as_f64)))
    }
}

#[async_trait]
impl Provider for OpenRouter {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "OpenRouter".into(),
            kind: ProviderKind::Cloud,
            tagline: "One key for image and video models from every lab".into(),
            website: "https://openrouter.ai".into(),
            needs_key: true,
            key_env: vec!["OPENROUTER_API_KEY".into()],
            key_url: Some("https://openrouter.ai/settings/keys".into()),
            key_hint: Some("sk-or-v1-…".into()),
            default_base_url: "https://openrouter.ai/api/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
            group: ProviderGroup::Gateway,
            quick_start: true,
            base_url_presets: vec![],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let (images, videos) = futures::join!(listing(cx, "/images/models"), listing(cx, "/videos/models"));
        if let (Err(e), Err(_)) = (&images, &videos) {
            tracing::debug!("openrouter listings unavailable, using built-in models: {e}");
        }
        let images = images.unwrap_or_else(|_| parse_list(FALLBACK_IMAGES));
        let videos = videos.unwrap_or_else(|_| parse_list(FALLBACK_VIDEOS));
        let mut models: Vec<ModelInfo> =
            images.iter().filter_map(image_info).chain(videos.iter().filter_map(video_info)).collect();
        // Featured first, in our order; the rest keep the listing's order (newest first).
        let rank = |m: &ModelInfo| FEATURED.iter().position(|(id, _)| *id == m.id).unwrap_or(usize::MAX);
        models.sort_by_key(rank);
        Ok(models)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v = call(cx, cx.http.get(cx.url("/key")).bearer_auth(cx.key()?)).await?;
        let d = v.get("data").unwrap_or(&Value::Null);
        let label = d.get("label").and_then(Value::as_str).filter(|l| !l.is_empty());
        let mut s = match label {
            Some(l) => format!("Key “{l}” works"),
            None => "Key works".into(),
        };
        if let Some(left) = d.get("limit_remaining").and_then(Value::as_f64) {
            s.push_str(&format!(" · ${left:.2} left"));
        } else if let Some(used) = d.get("usage").and_then(Value::as_f64) {
            s.push_str(&format!(" · ${used:.2} used"));
        }
        Ok(s)
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        match req.task {
            Task::TextToImage | Task::ImageToImage => self.image(cx, req).await,
            Task::TextToVideo | Task::ImageToVideo => self.video(cx, req).await,
        }
    }
}
