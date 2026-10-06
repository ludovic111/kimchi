//! Together AI: fast open and partner models for images and video.
//!
//! * Images: `POST /v1/images/generations` (synchronous, base64). Most models
//!   take `width`/`height`; the FLUX Kontext family takes `aspect_ratio`.
//!   Edits pass `image_url` (Kontext) or `reference_images` (FLUX.2, Gemini).
//! * Video: `POST /v2/videos` → job, poll `GET /v2/videos/{id}` until
//!   `completed`, then fetch the short-lived `outputs.video_url`.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::providers::openai::parse_images;
use crate::types::*;
use crate::util;

pub const ID: &str = "together";

pub struct Together;

const PIXEL_RATIOS: &[&str] = &["1:1", "4:3", "3:4", "3:2", "2:3", "16:9", "9:16", "21:9", "9:21"];
const KONTEXT_RATIOS: &[&str] = &["1:1", "4:3", "3:4", "3:2", "2:3", "16:9", "9:16", "21:9", "9:21"];
const MAX_N: u32 = 4;

/// How a model takes input images.
#[derive(Clone, Copy, PartialEq)]
enum Refs {
    None,
    /// One image to edit in `image_url`.
    Single,
    /// Several in `reference_images`.
    Many(u32),
}

struct ImageSpec {
    id: &'static str,
    name: &'static str,
    /// Kontext models size by `aspect_ratio`, the rest by `width`/`height`.
    by_ratio: bool,
    refs: Refs,
    /// Offers 1K / 2K output (by megapixels).
    big: bool,
    tuning: bool,
    negative: bool,
    price: &'static str,
    featured: bool,
}

#[rustfmt::skip]
const IMAGES: &[ImageSpec] = &[
    ImageSpec { id: "black-forest-labs/FLUX.2-pro", name: "FLUX.2 Pro", by_ratio: false, refs: Refs::Many(8), big: true, tuning: false, negative: false, price: "$0.03 / MP", featured: true },
    ImageSpec { id: "black-forest-labs/FLUX.2-max", name: "FLUX.2 Max", by_ratio: false, refs: Refs::Many(8), big: true, tuning: false, negative: false, price: "$0.07 / MP", featured: true },
    ImageSpec { id: "black-forest-labs/FLUX.2-flex", name: "FLUX.2 Flex", by_ratio: false, refs: Refs::Many(8), big: true, tuning: true, negative: false, price: "$0.03 / MP", featured: false },
    ImageSpec { id: "black-forest-labs/FLUX.2-dev", name: "FLUX.2 Dev", by_ratio: false, refs: Refs::Many(8), big: true, tuning: true, negative: false, price: "$0.015 / MP", featured: false },
    ImageSpec { id: "black-forest-labs/FLUX.1-kontext-pro", name: "FLUX.1 Kontext Pro", by_ratio: true, refs: Refs::Single, big: false, tuning: false, negative: false, price: "$0.04 / MP", featured: false },
    ImageSpec { id: "black-forest-labs/FLUX.1-kontext-max", name: "FLUX.1 Kontext Max", by_ratio: true, refs: Refs::Single, big: false, tuning: false, negative: false, price: "$0.08 / MP", featured: false },
    ImageSpec { id: "google/gemini-3-pro-image", name: "Nano Banana Pro (Gemini 3 Pro Image)", by_ratio: false, refs: Refs::Many(14), big: false, tuning: false, negative: false, price: "$0.134 / image", featured: true },
    ImageSpec { id: "google/flash-image-3.1", name: "Nano Banana 2 (Gemini 3.1 Flash Image)", by_ratio: false, refs: Refs::Many(14), big: false, tuning: false, negative: false, price: "$0.05 / MP", featured: false },
    ImageSpec { id: "ByteDance/Seedream-5.0-lite", name: "Seedream 5.0 Lite", by_ratio: false, refs: Refs::None, big: true, tuning: false, negative: false, price: "$0.035 / MP", featured: false },
    ImageSpec { id: "Qwen/Qwen-Image-2.0-Pro", name: "Qwen Image 2.0 Pro", by_ratio: false, refs: Refs::None, big: false, tuning: false, negative: true, price: "$0.075 / MP", featured: false },
    ImageSpec { id: "Qwen/Qwen-Image-2.0", name: "Qwen Image 2.0", by_ratio: false, refs: Refs::None, big: false, tuning: false, negative: true, price: "$0.035 / MP", featured: false },
    ImageSpec { id: "ideogram/ideogram-4.0", name: "Ideogram 4.0", by_ratio: false, refs: Refs::None, big: false, tuning: false, negative: true, price: "$0.06 / MP", featured: false },
];

/// How a video model is sized.
#[derive(Clone, Copy)]
enum Sizing {
    /// `width`/`height`; resolutions are the short side (`"720p"`).
    Pixels { resolutions: &'static [&'static str], ratios: &'static [&'static str] },
    /// `resolution` tier plus `ratio` (when the model takes one).
    Tiers { resolutions: &'static [&'static str], ratios: &'static [&'static str] },
}

struct VideoSpec {
    id: &'static str,
    name: &'static str,
    tasks: &'static [Task],
    durations: &'static [f64],
    sizing: Sizing,
    end_frame: bool,
    audio: bool,
    price: &'static str,
    featured: bool,
}

const T2V: &[Task] = &[Task::TextToVideo];
const I2V: &[Task] = &[Task::ImageToVideo];
const BOTH: &[Task] = &[Task::TextToVideo, Task::ImageToVideo];
const WAN_RATIOS: &[&str] = &["16:9", "9:16", "1:1", "4:3", "3:4"];

#[rustfmt::skip]
const VIDEOS: &[VideoSpec] = &[
    VideoSpec { id: "google/veo-3.1", name: "Veo 3.1", tasks: BOTH, durations: &[4.0, 6.0, 8.0], sizing: Sizing::Pixels { resolutions: &["720p", "1080p"], ratios: &["16:9", "9:16"] }, end_frame: true, audio: true, price: "$0.08 / video", featured: true },
    VideoSpec { id: "google/veo-3.1-lite", name: "Veo 3.1 Lite", tasks: BOTH, durations: &[4.0, 6.0, 8.0], sizing: Sizing::Pixels { resolutions: &["720p", "1080p"], ratios: &["16:9", "9:16"] }, end_frame: true, audio: true, price: "$0.05 / video", featured: false },
    VideoSpec { id: "ByteDance/Seedance-2.5", name: "Seedance 2.5", tasks: BOTH, durations: &[4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 15.0, 20.0, 25.0, 30.0], sizing: Sizing::Tiers { resolutions: &["480p", "720p"], ratios: &[] }, end_frame: true, audio: true, price: "$0.115 / video", featured: true },
    VideoSpec { id: "Wan-AI/wan2.7-t2v", name: "Wan 2.7", tasks: T2V, durations: &[5.0, 10.0, 15.0], sizing: Sizing::Tiers { resolutions: &["720P", "1080P"], ratios: WAN_RATIOS }, end_frame: false, audio: true, price: "$0.10 / video", featured: true },
    VideoSpec { id: "Wan-AI/wan2.7-i2v", name: "Wan 2.7 Image-to-Video", tasks: I2V, durations: &[5.0, 10.0, 15.0], sizing: Sizing::Tiers { resolutions: &["720P", "1080P"], ratios: WAN_RATIOS }, end_frame: true, audio: true, price: "$0.10 / video", featured: false },
    VideoSpec { id: "minimax/hailuo-02", name: "MiniMax Hailuo 02", tasks: BOTH, durations: &[6.0, 10.0], sizing: Sizing::Pixels { resolutions: &["768p"], ratios: &["16:9"] }, end_frame: false, audio: false, price: "$0.49 / video", featured: false },
];

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn image_info(s: &ImageSpec) -> ModelInfo {
    let tasks: &[Task] =
        if s.refs == Refs::None { &[Task::TextToImage] } else { &[Task::TextToImage, Task::ImageToImage] };
    let mut params = vec![];
    if s.tuning {
        params.push(ParamSpec {
            key: "steps".into(),
            label: "Steps".into(),
            kind: ParamKind::Int { min: 1, max: 50, step: 1 },
            default: json!(28),
            help: Some("More steps can add detail; past the default they cost more.".into()),
        });
        params.push(ParamSpec {
            key: "guidance_scale".into(),
            label: "Guidance".into(),
            kind: ParamKind::Float { min: 1.0, max: 10.0, step: 0.5 },
            default: json!(3.5),
            help: Some("How closely to follow the prompt.".into()),
        });
    }
    ModelInfo {
        aspect_ratios: strings(if s.by_ratio { KONTEXT_RATIOS } else { PIXEL_RATIOS }),
        resolutions: if s.big { strings(&["1K", "2K"]) } else { vec![] },
        max_outputs: MAX_N,
        seed: true,
        negative_prompt: s.negative,
        max_images: match s.refs {
            Refs::None => 0,
            Refs::Single => 1,
            Refs::Many(n) => n,
        },
        params,
        price: Some(s.price.into()),
        featured: s.featured,
        ..ModelInfo::new(ID, s.id, s.name, tasks)
    }
}

fn video_info(s: &VideoSpec) -> ModelInfo {
    let (resolutions, ratios) = match s.sizing {
        Sizing::Pixels { resolutions, ratios } | Sizing::Tiers { resolutions, ratios } => (resolutions, ratios),
    };
    ModelInfo {
        aspect_ratios: strings(ratios),
        durations: s.durations.to_vec(),
        resolutions: strings(resolutions),
        seed: true,
        negative_prompt: true,
        end_frame: s.end_frame,
        audio: s.audio,
        price: Some(s.price.into()),
        featured: s.featured,
        ..ModelInfo::new(ID, s.id, s.name, s.tasks)
    }
}

/// A model from `GET /models` we have no curated entry for: offer the basics.
fn generic_info(m: &Value) -> Option<ModelInfo> {
    let id = m.get("id").and_then(Value::as_str)?;
    let name = m.get("display_name").and_then(Value::as_str).unwrap_or(id);
    match m.get("type").and_then(Value::as_str)? {
        "image" => Some(ModelInfo {
            aspect_ratios: strings(PIXEL_RATIOS),
            max_outputs: MAX_N,
            seed: true,
            ..ModelInfo::new(ID, id, name, &[Task::TextToImage])
        }),
        "video" => {
            let lower = id.to_ascii_lowercase();
            // Edit / reference-to-video models need inputs we can't provide.
            if ["edit", "r2v", "upscale"].iter().any(|k| lower.contains(k)) {
                return None;
            }
            let tasks: &[Task] = if lower.contains("i2v") {
                I2V
            } else if lower.contains("t2v") {
                T2V
            } else {
                BOTH
            };
            Some(ModelInfo { aspect_ratios: strings(&["16:9", "9:16", "1:1"]), ..ModelInfo::new(ID, id, name, tasks) })
        }
        _ => None,
    }
}

fn catalog() -> Vec<ModelInfo> {
    IMAGES.iter().map(image_info).chain(VIDEOS.iter().map(video_info)).collect()
}

/// Video lives under `/v2` next to the `/v1` base.
fn v2(cx: &Ctx, path: &str) -> String {
    let base = cx.base_url.strip_suffix("/v1").unwrap_or(&cx.base_url);
    format!("{base}/v2{path}")
}

/// Together answers NSFW-filtered images with a 422.
fn moderation(e: GenError) -> GenError {
    match e {
        GenError::Http { status: 422, ref message, .. }
            if ["nsfw", "safety", "unsafe", "content", "moderat"]
                .iter()
                .any(|k| message.to_ascii_lowercase().contains(k)) =>
        {
            GenError::Moderated(message.clone())
        }
        e => crate::providers::openai::moderation(e),
    }
}

/// Short side in pixels for `"720p"`, `"1080P"`, …
fn short_side(res: &str) -> Option<u32> {
    res.trim_end_matches(['p', 'P']).parse().ok()
}

/// `(width, height)` with the given short side, multiples of 16.
fn pixels(ratio: &str, short: u32) -> (u32, u32) {
    let (a, b) = util::parse_ratio(ratio).unwrap_or((16.0, 9.0));
    let long = |r: f64| ((short as f64 * r / 16.0).round() as u32) * 16;
    if a >= b { (long(a / b), short) } else { (short, long(b / a)) }
}

fn pick<'a>(wanted: Option<&str>, allowed: &'a [&'a str]) -> Option<&'a str> {
    let first = allowed.first().copied();
    let Some(w) = wanted else { return first };
    allowed
        .iter()
        .copied()
        .find(|a| a.eq_ignore_ascii_case(w))
        .or_else(|| {
            let w = short_side(w)?;
            allowed.iter().copied().min_by_key(|a| short_side(a).map_or(u32::MAX, |s| s.abs_diff(w)))
        })
        .or(first)
}

impl Together {
    async fn image(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        let spec = IMAGES.iter().find(|s| s.id == req.model);
        let mut body =
            json!({ "model": req.model, "prompt": req.prompt, "response_format": "base64", "output_format": "png" });
        if spec.is_some_and(|s| s.by_ratio) {
            body["aspect_ratio"] = json!(util::closest_ratio(&req.aspect(), KONTEXT_RATIOS));
        } else {
            let ratio = util::closest_ratio(&req.aspect(), PIXEL_RATIOS);
            let big = spec.is_some_and(|s| s.big) && req.resolution.as_deref().is_some_and(|r| r.starts_with('2'));
            let (w, h) = util::size_for_ratio(ratio, if big { 4.0 } else { 1.0 }, 16);
            body["width"] = json!(w);
            body["height"] = json!(h);
        }
        if let Some(seed) = req.seed {
            body["seed"] = json!(seed);
        }
        if let Some(neg) = req.negative_prompt.as_deref().filter(|n| !n.trim().is_empty())
            && spec.is_none_or(|s| s.negative)
        {
            body["negative_prompt"] = json!(neg);
        }
        if spec.is_some_and(|s| s.tuning) {
            for k in ["steps", "guidance_scale"] {
                if let Some(v) = req.param(k).filter(|v| v.is_number()) {
                    body[k] = v.clone();
                }
            }
        }
        match spec.map_or(Refs::Many(8), |s| s.refs) {
            Refs::None => {}
            Refs::Single => {
                if let Some(img) = req.references().next() {
                    body["image_url"] = json!(img.data_url());
                }
            }
            Refs::Many(max) => {
                let refs: Vec<String> = req.references().take(max as usize).map(InputImage::data_url).collect();
                if !refs.is_empty() {
                    body["reference_images"] = json!(refs);
                }
            }
        }

        cx.report(Progress::message("Generating"));
        // Up to four images per call; larger counts run as parallel calls.
        let mut batches = vec![];
        let mut left = req.count.max(1);
        while left > 0 {
            batches.push(left.min(MAX_N));
            left = left.saturating_sub(MAX_N);
        }
        let calls = batches.into_iter().map(|n| {
            let mut body = body.clone();
            body["n"] = json!(n);
            async move {
                let req = cx.http.post(cx.url("/images/generations")).bearer_auth(key).json(&body);
                let v: Value = util::send_json(cx, req).await.map_err(moderation)?;
                parse_images(cx, &v, "image/png")
            }
        });
        let items: Vec<OutputItem> = futures::future::try_join_all(calls).await?.into_iter().flatten().collect();
        Ok(GenOutput { items, seed: req.seed, ..Default::default() })
    }

    async fn video(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        let spec = VIDEOS.iter().find(|s| s.id == req.model);
        let mut body = json!({ "model": req.model, "prompt": req.prompt });
        if let Some(d) = match spec {
            Some(s) => Some(util::closest_duration(req.duration, s.durations, s.durations[0])),
            None => req.duration,
        } {
            body["seconds"] = json!((d.round() as u32).to_string());
        }
        let sizing = spec
            .map_or(Sizing::Pixels { resolutions: &["720p", "1080p"], ratios: &["16:9", "9:16", "1:1"] }, |s| s.sizing);
        let follow_image = req.task == Task::ImageToVideo && req.aspect_ratio.is_none();
        match sizing {
            Sizing::Pixels { resolutions, ratios } => {
                if !follow_image {
                    let short = pick(req.resolution.as_deref(), resolutions).and_then(short_side).unwrap_or(720);
                    let (w, h) = pixels(util::closest_ratio(&req.aspect(), ratios), short);
                    body["width"] = json!(w);
                    body["height"] = json!(h);
                }
            }
            Sizing::Tiers { resolutions, ratios } => {
                if let Some(r) = pick(req.resolution.as_deref(), resolutions) {
                    body["resolution"] = json!(r);
                }
                if !ratios.is_empty() && !follow_image {
                    body["ratio"] = json!(util::closest_ratio(&req.aspect(), ratios));
                }
            }
        }
        if let Some(seed) = req.seed {
            body["seed"] = json!(seed);
        }
        if let Some(neg) = req.negative_prompt.as_deref().filter(|n| !n.trim().is_empty()) {
            body["negative_prompt"] = json!(neg);
        }
        if let (Some(audio), true) = (req.audio, spec.is_some_and(|s| s.audio)) {
            body["generate_audio"] = json!(audio);
        }
        // Frames and references can't be mixed on some models; frames win.
        let start = (req.task == Task::ImageToVideo).then(|| req.start_frame()).flatten();
        if let Some(start) = start {
            let mut frames = vec![json!({ "input_image": start.base64(), "frame": "first" })];
            if let Some(end) = req.image(ImageRole::EndFrame).filter(|_| spec.is_none_or(|s| s.end_frame)) {
                frames.push(json!({ "input_image": end.base64(), "frame": "last" }));
            }
            body["media"] = json!({ "frame_images": frames });
        } else {
            let refs: Vec<String> = req.references().map(InputImage::base64).collect();
            if !refs.is_empty() {
                body["media"] = json!({ "reference_images": refs });
            }
        }

        let started = Instant::now();
        let jobs = (0..req.count.max(1)).map(|_| self.video_job(cx, key, &body, started));
        let mut out = GenOutput { seed: req.seed, ..Default::default() };
        for (item, cost) in futures::future::try_join_all(jobs).await? {
            out.items.push(item);
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
    ) -> GenResult<(OutputItem, Option<f64>)> {
        let created: Value = util::send_json(cx, cx.http.post(v2(cx, "/videos")).bearer_auth(key).json(body))
            .await
            .map_err(moderation)?;
        let id =
            created.get("id").and_then(Value::as_str).ok_or_else(|| util::decode_err(cx, "video job has no id"))?;
        let status_url = v2(cx, &format!("/videos/{id}"));
        let done: Value = util::poll(Duration::from_secs(5), Duration::from_secs(60 * 30), || async {
            let v: Value = util::send_json(cx, cx.http.get(&status_url).bearer_auth(key)).await?;
            match v.get("status").and_then(Value::as_str).unwrap_or("queued") {
                "completed" => Ok(Some(v)),
                "failed" => {
                    let code =
                        v.pointer("/error/code").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
                    let msg = v
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .filter(|m| !m.is_empty())
                        .unwrap_or("The video generation failed.")
                        .to_string();
                    let lower = format!("{code} {}", msg.to_ascii_lowercase());
                    Err(if ["nsfw", "safety", "moderat", "policy"].iter().any(|k| lower.contains(k)) {
                        GenError::Moderated(msg)
                    } else {
                        GenError::Provider(msg)
                    })
                }
                "cancelled" => Err(GenError::Provider("The video job was cancelled.".into())),
                s => {
                    util::estimate(
                        cx,
                        started,
                        Duration::from_secs(120),
                        if s == "queued" { "In queue" } else { "Rendering" },
                    );
                    Ok(None)
                }
            }
        })
        .await?;
        let url = done
            .pointer("/outputs/video_url")
            .and_then(Value::as_str)
            .ok_or_else(|| util::decode_err(cx, "finished video has no outputs.video_url"))?;
        Ok((OutputItem::url(OutputKind::Video, url), done.pointer("/outputs/cost").and_then(Value::as_f64)))
    }
}

#[async_trait]
impl Provider for Together {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Together AI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Fast open models: FLUX.2, Qwen Image, Seedance, Wan and more".into(),
            website: "https://together.ai".into(),
            needs_key: true,
            key_env: vec!["TOGETHER_API_KEY".into()],
            key_url: Some("https://api.together.ai/settings/api-keys".into()),
            key_hint: None,
            default_base_url: "https://api.together.ai/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
            group: ProviderGroup::Gateway,
            quick_start: false,
            base_url_presets: vec![],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let mut models = catalog();
        // Add whatever else the account can run; the curated list is enough offline.
        if let Some(key) = cx.api_key.as_deref()
            && let Ok(Value::Array(list)) =
                util::send_json::<Value>(cx, cx.http.get(cx.url("/models")).bearer_auth(key)).await
        {
            let extra: Vec<ModelInfo> = list
                .iter()
                .filter_map(generic_info)
                .filter(|m| !models.iter().any(|c| c.id.eq_ignore_ascii_case(&m.id)))
                .collect();
            models.extend(extra);
        }
        Ok(models)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(cx, cx.http.get(cx.url("/models")).bearer_auth(cx.key()?)).await?;
        let list = v.as_array().map(Vec::as_slice).unwrap_or_default();
        let count = |t: &str| list.iter().filter(|m| m.get("type").and_then(Value::as_str) == Some(t)).count();
        Ok(format!("Key works · {} image and {} video models", count("image"), count("video")))
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        match req.task {
            Task::TextToImage | Task::ImageToImage => self.image(cx, req).await,
            Task::TextToVideo | Task::ImageToVideo => self.video(cx, req).await,
        }
    }
}
