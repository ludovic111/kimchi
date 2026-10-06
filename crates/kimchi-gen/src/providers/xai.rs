//! xAI: Grok Imagine images and video.
//!
//! * Images: `POST /images/generations`, or `POST /images/edits` (JSON, data
//!   URLs) when reference images are given. Synchronous.
//! * Video: `POST /videos/generations` → `{request_id}`, poll
//!   `GET /videos/{request_id}` (real 0–100 progress) until `done`, then the
//!   result carries a short-lived public URL.
//!
//! Costs come back as `usage.cost_in_usd_ticks` (1 USD = 10^10 ticks).

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::providers::openai::{moderation, parse_images};
use crate::types::*;
use crate::util;

pub const ID: &str = "xai";

pub struct Xai;

const IMAGE_RATIOS: &[&str] =
    &["1:1", "4:3", "3:4", "3:2", "2:3", "16:9", "9:16", "2:1", "1:2", "21:9", "19.5:9", "9:19.5", "20:9", "9:20"];
const VIDEO_RATIOS: &[&str] = &["16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3"];
/// The API takes any whole number of seconds from 1 to 15; these are the
/// lengths offered in the picker.
const VIDEO_SECONDS: &[f64] = &[4.0, 6.0, 8.0, 10.0, 12.0, 15.0];
const MAX_EDIT_IMAGES: usize = 5;
const TICKS_PER_USD: f64 = 1e10;

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn image_model(id: &str, name: &str, price: &str, featured: bool, description: &str) -> ModelInfo {
    let mut params = vec![];
    if id == "grok-imagine-image-2.0" {
        params.push(ParamSpec {
            key: "quality".into(),
            label: "Quality".into(),
            kind: ParamKind::Select {
                options: vec![
                    SelectOption::new("auto", "Auto"),
                    SelectOption::new("low", "Low"),
                    SelectOption::new("medium", "Medium"),
                ],
            },
            default: json!("auto"),
            help: Some("Medium costs more but renders finer detail.".into()),
        });
    }
    ModelInfo {
        aspect_ratios: strings(IMAGE_RATIOS),
        resolutions: strings(&["1K", "2K"]),
        max_outputs: 10,
        max_images: MAX_EDIT_IMAGES as u32,
        params,
        price: Some(price.into()),
        featured,
        description: Some(description.into()),
        ..ModelInfo::new(ID, id, name, &[Task::TextToImage, Task::ImageToImage])
    }
}

fn video_model(id: &str, name: &str, pro: bool, price: &str, description: &str) -> ModelInfo {
    ModelInfo {
        aspect_ratios: strings(VIDEO_RATIOS),
        durations: VIDEO_SECONDS.to_vec(),
        resolutions: strings(if pro { &["480p", "720p", "1080p"] } else { &["480p", "720p"] }),
        audio: true,
        end_frame: pro,
        price: Some(price.into()),
        featured: pro,
        description: Some(description.into()),
        ..ModelInfo::new(ID, id, name, &[Task::TextToVideo, Task::ImageToVideo])
    }
}

fn catalog() -> Vec<ModelInfo> {
    vec![
        image_model(
            "grok-imagine-image-2.0",
            "Grok Imagine Image 2.0",
            "$0.04 / image",
            true,
            "xAI's latest image model, with multi-image edits.",
        ),
        image_model("grok-imagine-image", "Grok Imagine Image", "$0.02 / image", false, "Fast, cheap Grok images."),
        video_model(
            "grok-imagine-video-1.5",
            "Grok Imagine Video 1.5",
            true,
            "$0.08 / s",
            "Up to 15 s at 1080p with sound; start and end frames.",
        ),
        video_model("grok-imagine-video", "Grok Imagine Video", false, "$0.05 / s", "Up to 15 s at 720p with sound."),
    ]
}

fn cost(v: &Value) -> Option<f64> {
    v.pointer("/usage/cost_in_usd_ticks").and_then(Value::as_f64).map(|t| t / TICKS_PER_USD)
}

fn image_ref(img: &InputImage) -> Value {
    json!({ "type": "image_url", "url": img.data_url() })
}

impl Xai {
    async fn image(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        let mut body = json!({
            "model": req.model,
            "prompt": req.prompt,
            "n": req.count.clamp(1, 10),
            "response_format": "b64_json",
            "aspect_ratio": util::closest_ratio(&req.aspect(), IMAGE_RATIOS),
        });
        if let Some(r) = req.resolution.as_deref() {
            body["resolution"] = json!(if r.to_ascii_lowercase().starts_with('2') { "2k" } else { "1k" });
        }
        if let Some(q) = req.param("quality").and_then(Value::as_str).filter(|q| *q != "auto") {
            body["quality"] = json!(q);
        }
        let refs: Vec<&InputImage> = req.references().take(MAX_EDIT_IMAGES).collect();
        let path = match refs.as_slice() {
            [] => "/images/generations",
            [one] => {
                body["image"] = image_ref(one);
                "/images/edits"
            }
            many => {
                body["images"] = many.iter().map(|i| image_ref(i)).collect();
                "/images/edits"
            }
        };
        cx.report(Progress::message("Generating"));
        let resp: Value =
            util::send_json(cx, cx.http.post(cx.url(path)).bearer_auth(key).json(&body)).await.map_err(moderation)?;
        let items = parse_images(cx, &resp, "image/jpeg")?;
        if items.is_empty() {
            return Err(GenError::Moderated("Grok returned no image for this prompt.".into()));
        }
        Ok(GenOutput { items, cost_usd: cost(&resp), ..Default::default() })
    }

    async fn video(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let key = cx.key()?;
        let pro = req.model.contains("1.5");
        let resolution = match req.resolution.as_deref() {
            Some(r) if r.contains("1080") && pro => "1080p",
            Some(r) if r.contains("480") => "480p",
            _ => "720p",
        };
        let mut body = json!({
            "model": req.model,
            "prompt": req.prompt,
            "duration": req.duration.map_or(8, |d| d.round().clamp(1.0, 15.0) as u32),
            "resolution": resolution,
        });
        if req.task == Task::ImageToVideo
            && let Some(img) = req.start_frame()
        {
            body["image"] = json!({ "url": img.data_url() });
            // Without an explicit ratio the video follows the image; setting one stretches it.
            if req.aspect_ratio.is_some() {
                body["aspect_ratio"] = json!(util::closest_ratio(&req.aspect(), VIDEO_RATIOS));
            }
            if pro && let Some(end) = req.image(ImageRole::EndFrame) {
                body["last_frame"] = json!({ "url": end.data_url() });
            }
        } else {
            body["aspect_ratio"] = json!(util::closest_ratio(&req.aspect(), VIDEO_RATIOS));
        }
        if req.audio == Some(false) {
            body["generate_audio"] = json!(false);
        }

        let mut out = GenOutput::default();
        let count = req.count.max(1);
        for i in 0..count {
            let created: Value =
                util::send_json(cx, cx.http.post(cx.url("/videos/generations")).bearer_auth(key).json(&body))
                    .await
                    .map_err(moderation)?;
            let id = created
                .get("request_id")
                .and_then(Value::as_str)
                .ok_or_else(|| util::decode_err(cx, "video request has no request_id"))?;
            let status_url = cx.url(&format!("/videos/{id}"));
            let done: Value = util::poll(Duration::from_secs(4), Duration::from_secs(60 * 20), || async {
                let v: Value = util::send_json(cx, cx.http.get(&status_url).bearer_auth(key)).await?;
                match v.get("status").and_then(Value::as_str).unwrap_or("pending") {
                    "done" => Ok(Some(v)),
                    "failed" => Err(failure(&v)),
                    "expired" => Err(GenError::Provider("The video request expired before it finished.".into())),
                    _ => {
                        let pct = v.get("progress").and_then(Value::as_f64).unwrap_or(0.0) / 100.0;
                        let msg = if count > 1 { format!("Video {}/{count}", i + 1) } else { "Rendering".into() };
                        cx.report(Progress::fraction((i as f64 + pct) / count as f64 * 0.97, msg));
                        Ok(None)
                    }
                }
            })
            .await?;
            if done.pointer("/video/respect_moderation") == Some(&Value::Bool(false)) {
                return Err(GenError::Moderated("xAI filtered the generated video.".into()));
            }
            let url = done
                .pointer("/video/url")
                .and_then(Value::as_str)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| util::decode_err(cx, "finished video has no url"))?;
            out.items.push(OutputItem::url(OutputKind::Video, url));
            if let Some(c) = cost(&done) {
                out.cost_usd = Some(out.cost_usd.unwrap_or(0.0) + c);
            }
        }
        Ok(out)
    }
}

fn failure(v: &Value) -> GenError {
    let msg = v
        .pointer("/error/message")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .unwrap_or("xAI couldn't generate this video.")
        .to_string();
    let lower = msg.to_ascii_lowercase();
    if ["moderat", "policy", "safety", "inappropriate"].iter().any(|k| lower.contains(k)) {
        GenError::Moderated(msg)
    } else {
        GenError::Provider(msg)
    }
}

#[async_trait]
impl Provider for Xai {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "xAI".into(),
            kind: ProviderKind::Cloud,
            tagline: "Grok Imagine images and video".into(),
            website: "https://x.ai/api".into(),
            needs_key: true,
            key_env: vec!["XAI_API_KEY".into()],
            key_url: Some("https://console.x.ai".into()),
            key_hint: Some("xai-…".into()),
            default_base_url: "https://api.x.ai/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
            group: ProviderGroup::Media,
            quick_start: false,
            base_url_presets: vec![],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(catalog())
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(cx, cx.http.get(cx.url("/api-key")).bearer_auth(cx.key()?)).await?;
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        if flag("api_key_blocked") || flag("api_key_disabled") || flag("team_blocked") {
            return Err(GenError::Provider("This xAI key is blocked or disabled.".into()));
        }
        Ok(match v.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
            Some(name) => format!("Key “{name}” works"),
            None => "Key works".into(),
        })
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        match req.task {
            Task::TextToImage | Task::ImageToImage => self.image(cx, req).await,
            Task::TextToVideo | Task::ImageToVideo => self.video(cx, req).await,
        }
    }
}
