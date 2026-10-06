//! Runway: Gen-4 video and image references.
//!
//! `POST /v1/{text_to_image,text_to_video,image_to_video}` → `{id}`, then poll
//! `GET /v1/tasks/{id}` (Runway asks for ≥ 5 s between polls) until
//! `SUCCEEDED` with `output: [url]`. Every request needs `X-Runway-Version`.
//! Ratios are pixel strings that differ per model and endpoint, so the table
//! below lists them verbatim from the API spec (version 2024-11-06).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "runway";

const API_VERSION: &str = "2024-11-06";

pub struct Runway;

struct Model {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    kind: OutputKind,
    /// Ratios for the text endpoint (`None`: no text-only task).
    text: Option<&'static [&'static str]>,
    /// Ratios for the image endpoint (`None`: no image task).
    image: Option<&'static [&'static str]>,
    /// Image models that need at least one reference.
    refs_required: bool,
    durations: &'static [u32],
    resolutions: &'static [&'static str],
    /// Short side of the default output, picks among same-ratio sizes.
    default_short: u32,
    end_frame: bool,
    audio: bool,
    negative: bool,
    seed: bool,
    /// Accepts `contentModeration.publicFigureThreshold`.
    moderation: bool,
    price: &'static str,
    featured: bool,
    secs: u64,
}

const GEN4_VIDEO: &[&str] = &["1280:720", "720:1280", "1104:832", "960:960", "832:1104", "1584:672"];
const WIDE_TALL: &[&str] = &["1280:720", "720:1280"];
const VEO: &[&str] = &["1280:720", "720:1280", "1920:1080", "1080:1920"];
const GEN4_IMAGE: &[&str] = &[
    "1024:1024", "1080:1080", "1168:880", "1360:768", "1440:1080", "1080:1440", "1808:768", "1920:1080", "1080:1920", "2112:912",
    "1280:720", "720:1280", "720:720", "960:720", "720:960", "1680:720",
];
const GEMINI_IMAGE: &[&str] = &["1344:768", "768:1344", "1024:1024", "1184:864", "864:1184", "1536:672", "832:1248", "1248:832", "896:1152", "1152:896"];

const fn video(id: &'static str, name: &'static str, description: &'static str) -> Model {
    Model {
        id,
        name,
        description,
        kind: OutputKind::Video,
        text: None,
        image: None,
        refs_required: false,
        durations: &[],
        resolutions: &[],
        default_short: 720,
        end_frame: false,
        audio: false,
        negative: false,
        seed: true,
        moderation: false,
        price: "",
        featured: false,
        secs: 90,
    }
}

const fn image(id: &'static str, name: &'static str, description: &'static str) -> Model {
    Model { kind: OutputKind::Image, default_short: 1080, secs: 20, ..video(id, name, description) }
}

const D2_10: &[u32] = &[2, 3, 4, 5, 6, 7, 8, 9, 10];

const MODELS: &[Model] = &[
    Model {
        text: Some(WIDE_TALL),
        image: Some(GEN4_VIDEO),
        durations: D2_10,
        moderation: true,
        price: "$0.12 / s",
        featured: true,
        ..video("gen4.5", "Gen-4.5", "Runway's best video model: realistic motion and strong prompt adherence.")
    },
    Model {
        image: Some(GEN4_VIDEO),
        durations: D2_10,
        moderation: true,
        price: "$0.05 / s",
        secs: 45,
        ..video("gen4_turbo", "Gen-4 Turbo", "Fast, cheap image-to-video.")
    },
    Model {
        text: Some(VEO),
        image: Some(VEO),
        durations: &[4, 6, 8],
        resolutions: &["720p", "1080p"],
        end_frame: true,
        audio: true,
        negative: true,
        price: "$0.20–0.40 / s",
        ..video("veo3.1", "Veo 3.1", "Google Veo 3.1 with native sound and first/last frames.")
    },
    Model {
        text: Some(VEO),
        image: Some(VEO),
        durations: &[4, 6, 8],
        resolutions: &["720p", "1080p"],
        end_frame: true,
        audio: true,
        negative: true,
        price: "$0.10–0.15 / s",
        secs: 60,
        ..video("veo3.1_fast", "Veo 3.1 Fast", "Cheaper, faster Veo 3.1 with sound.")
    },
    Model {
        text: Some(WIDE_TALL),
        image: Some(WIDE_TALL),
        durations: &[3, 4, 5, 6, 7, 8, 9, 10],
        seed: false,
        price: "$0.10 / s",
        secs: 60,
        ..video("gemini_omni_flash", "Gemini Omni Flash", "Quick, inexpensive drafts from Google.")
    },
    Model {
        text: Some(GEN4_IMAGE),
        image: Some(GEN4_IMAGE),
        resolutions: &["720p", "1080p"],
        moderation: true,
        price: "$0.05 (720p) – $0.08 (1080p) / image",
        featured: true,
        ..image("gen4_image", "Gen-4 Image", "Consistent characters and places from up to 3 references.")
    },
    Model {
        image: Some(GEN4_IMAGE),
        refs_required: true,
        resolutions: &["720p", "1080p"],
        moderation: true,
        price: "$0.02 / image",
        secs: 10,
        ..image("gen4_image_turbo", "Gen-4 Image Turbo", "Fast reference-driven images (needs a reference).")
    },
    Model {
        text: Some(GEMINI_IMAGE),
        image: Some(GEMINI_IMAGE),
        seed: false,
        price: "$0.05 / image",
        ..image("gemini_2.5_flash", "Gemini 2.5 Flash Image", "Google's Nano Banana via Runway.")
    },
];

fn find(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

const LABELS: &[&str] = &["21:9", "2:1", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16"];

/// Friendly ratio labels for pixel ratios (`1104:832` → `4:3`).
fn labels(pixels: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for p in pixels {
        let l = util::closest_ratio(p, LABELS).to_string();
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

fn short_side(res: &str) -> Option<u32> {
    let r = res.to_ascii_lowercase();
    if r == "4k" {
        return Some(2160);
    }
    r.trim_end_matches('p').parse().ok()
}

/// Closest pixel ratio by shape, then by size among equally good shapes.
fn pick_ratio(list: &[&'static str], aspect: &str, short: u32) -> &'static str {
    let Some((a, b)) = util::parse_ratio(aspect) else { return list[0] };
    let target = (a / b).ln();
    let target_area = (short as f64).powi(2) * (a / b).max(b / a);
    let score = |r: &str| {
        let (w, h) = util::parse_ratio(r).unwrap_or((1.0, 1.0));
        let shape = ((w / h).ln() - target).abs();
        let size = ((w * h) / target_area).ln().abs();
        // Shape dominates; size only breaks near-ties.
        (shape * 20.0).round() * 10.0 + size
    };
    list.iter().copied().min_by(|x, y| score(x).total_cmp(&score(y))).unwrap_or(list[0])
}

fn model_info(m: &Model) -> ModelInfo {
    let (t, i) = match m.kind {
        OutputKind::Video => (Task::TextToVideo, Task::ImageToVideo),
        _ => (Task::TextToImage, Task::ImageToImage),
    };
    let mut tasks = vec![];
    if m.text.is_some() {
        tasks.push(t);
    }
    if m.image.is_some() {
        tasks.push(i);
    }
    let mut params = vec![];
    if m.moderation {
        params.push(ParamSpec {
            key: "public_figures".into(),
            label: "Public figures".into(),
            kind: ParamKind::Select { options: vec![SelectOption::new("auto", "Standard"), SelectOption::new("low", "Less strict")] },
            default: json!("auto"),
            help: Some("How strictly recognisable people are blocked.".into()),
        });
    }
    ModelInfo {
        description: Some(m.description.into()),
        aspect_ratios: labels(m.text.or(m.image).unwrap_or(&[])),
        durations: m.durations.iter().map(|d| *d as f64).collect(),
        resolutions: m.resolutions.iter().map(|r| r.to_string()).collect(),
        max_outputs: if m.kind == OutputKind::Image { 4 } else { 1 },
        negative_prompt: m.negative,
        seed: m.seed,
        end_frame: m.end_frame,
        max_images: if m.image.is_none() {
            0
        } else if m.kind == OutputKind::Image {
            3
        } else {
            1
        },
        audio: m.audio,
        params,
        price: (!m.price.is_empty()).then(|| m.price.into()),
        featured: m.featured,
        ..ModelInfo::new(ID, m.id, m.name, &tasks)
    }
}

fn headers(cx: &Ctx, req: reqwest::RequestBuilder) -> GenResult<reqwest::RequestBuilder> {
    Ok(req.bearer_auth(cx.key()?).header("X-Runway-Version", API_VERSION))
}

/// Data URIs are capped at 5 MB encoded; bigger images go through an ephemeral upload.
const INLINE_LIMIT: usize = 3_500_000;

async fn image_uri(cx: &Ctx, img: &InputImage) -> GenResult<String> {
    if img.data.len() <= INLINE_LIMIT {
        return Ok(img.data_url());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Upload {
        upload_url: String,
        fields: Map<String, Value>,
        runway_uri: String,
    }
    let up: Upload = util::send_json(
        cx,
        headers(cx, cx.http.post(cx.url("/uploads")))?.json(&json!({"filename": img.file_name(), "type": "ephemeral"})),
    )
    .await?;
    let mut form = Form::new();
    for (k, v) in up.fields {
        form = form.text(k, v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
    }
    let part = Part::bytes(img.data.to_vec()).file_name(img.file_name()).mime_str(&img.mime).map_err(|e| GenError::Provider(e.to_string()))?;
    util::send(cx, cx.http.post(&up.upload_url).multipart(form.part("file", part))).await?;
    Ok(up.runway_uri)
}

async fn body(cx: &Ctx, m: &Model, req: &GenRequest) -> GenResult<(&'static str, Map<String, Value>)> {
    let with_image = req.task.needs_image();
    let ratios = if with_image { m.image } else { m.text }
        .ok_or_else(|| GenError::Unsupported(format!("{} doesn't do {}", m.name, req.task.as_str().replace('_', " "))))?;
    let short = req.resolution.as_deref().and_then(short_side).unwrap_or(m.default_short);
    let mut b = Map::new();
    if !req.prompt.trim().is_empty() {
        b.insert("promptText".into(), json!(req.prompt));
    }
    b.insert("ratio".into(), json!(pick_ratio(ratios, &req.aspect(), short)));
    let path = match m.kind {
        OutputKind::Image => {
            let refs: Vec<&InputImage> = req.references().take(3).collect();
            if m.refs_required && refs.is_empty() {
                return Err(GenError::Provider(format!("{} needs at least one reference image.", m.name)));
            }
            let mut list = vec![];
            for (i, img) in refs.into_iter().enumerate() {
                // Tags let prompts say "@image1".
                list.push(json!({"uri": image_uri(cx, img).await?, "tag": format!("image{}", i + 1)}));
            }
            if !list.is_empty() {
                b.insert("referenceImages".into(), json!(list));
            }
            "/text_to_image"
        }
        _ => {
            // Always sent: gen4.5 requires it. Default 5 s, else the longest.
            if let Some(&last) = m.durations.last() {
                let default = if m.durations.contains(&5) { 5.0 } else { last as f64 };
                let allowed: Vec<f64> = m.durations.iter().map(|d| *d as f64).collect();
                b.insert("duration".into(), json!(util::closest_duration(req.duration, &allowed, default) as u32));
            }
            if m.audio
                && let Some(a) = req.audio
            {
                b.insert("audio".into(), json!(a));
            }
            if with_image {
                let start = req.start_frame().ok_or_else(|| GenError::Provider("This task needs an input image.".into()))?;
                let start_uri = image_uri(cx, start).await?;
                match req.image(ImageRole::EndFrame).filter(|_| m.end_frame) {
                    Some(end) => {
                        let end_uri = image_uri(cx, end).await?;
                        b.insert(
                            "promptImage".into(),
                            json!([{"uri": start_uri, "position": "first"}, {"uri": end_uri, "position": "last"}]),
                        );
                    }
                    None => {
                        b.insert("promptImage".into(), json!(start_uri));
                    }
                }
                "/image_to_video"
            } else {
                "/text_to_video"
            }
        }
    };
    b.insert("model".into(), json!(m.id));
    if m.negative
        && let Some(n) = req.negative_prompt.as_ref().filter(|n| !n.trim().is_empty())
    {
        b.insert("negativePrompt".into(), json!(n));
    }
    if m.seed
        && let Some(s) = req.seed
    {
        b.insert("seed".into(), json!(s.rem_euclid(4_294_967_296)));
    }
    if m.moderation
        && let Some(t) = req.param("public_figures").and_then(Value::as_str)
    {
        b.insert("contentModeration".into(), json!({"publicFigureThreshold": t}));
    }
    Ok((path, b))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskState {
    status: String,
    #[serde(default)]
    progress: Option<f64>,
    #[serde(default)]
    output: Option<Vec<String>>,
    #[serde(default)]
    failure: Option<String>,
    #[serde(default)]
    failure_code: Option<String>,
    #[serde(default)]
    cost: Option<Value>,
}

async fn run_task(cx: &Ctx, path: &str, body: &Map<String, Value>, expected: Duration, label: &str) -> GenResult<TaskState> {
    let created: Value = util::send_json(cx, headers(cx, cx.http.post(cx.url(path)))?.json(body)).await?;
    let id = created.get("id").and_then(Value::as_str).ok_or_else(|| util::decode_err(cx, "no task id"))?.to_string();
    let started = Instant::now();
    util::poll(Duration::from_secs(5), Duration::from_secs(60 * 30), || async {
        let t: TaskState = util::send_json(cx, headers(cx, cx.http.get(cx.url(&format!("/tasks/{id}"))))?).await?;
        match t.status.as_str() {
            "SUCCEEDED" => Ok(Some(t)),
            "FAILED" => {
                let code = t.failure_code.clone().unwrap_or_default();
                let msg = t.failure.clone().unwrap_or_else(|| "generation failed".into());
                if code.contains("SAFETY") {
                    Err(GenError::Moderated(msg))
                } else if code.is_empty() {
                    Err(GenError::Provider(format!("Runway: {msg}")))
                } else {
                    Err(GenError::Provider(format!("Runway: {msg} ({code})")))
                }
            }
            "CANCELLED" => Err(GenError::Provider("Runway cancelled the task.".into())),
            "THROTTLED" => {
                cx.report(Progress::message("Waiting for a free slot"));
                Ok(None)
            }
            "PENDING" => {
                cx.report(Progress::message("In queue"));
                Ok(None)
            }
            _ => {
                match t.progress.filter(|p| *p > 0.0) {
                    Some(p) => cx.report(Progress::fraction(p * 0.95, label)),
                    None => util::estimate(cx, started, expected, label),
                }
                Ok(None)
            }
        }
    })
    .await
}

#[async_trait]
impl Provider for Runway {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Runway".into(),
            kind: ProviderKind::Cloud,
            tagline: "Gen-4 video and image references".into(),
            website: "https://runwayml.com".into(),
            needs_key: true,
            key_env: vec!["RUNWAYML_API_SECRET".into(), "RUNWAY_API_KEY".into()],
            key_url: Some("https://dev.runwayml.com".into()),
            key_hint: Some("key_…".into()),
            default_base_url: "https://api.dev.runwayml.com/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
            group: ProviderGroup::Media,
            quick_start: false,
            base_url_presets: vec![],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(MODELS.iter().map(model_info).collect())
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(cx, headers(cx, cx.http.get(cx.url("/organization")))?).await?;
        Ok(match v.get("creditBalance").and_then(Value::as_f64) {
            Some(c) => format!("{c:.0} credits left"),
            None => "Key works".into(),
        })
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let m = find(&req.model).ok_or_else(|| GenError::Unsupported(format!("unknown Runway model `{}`", req.model)))?;
        if req.task.output() != m.kind {
            return Err(GenError::Unsupported(format!("{} doesn't do {}", m.name, req.task.as_str().replace('_', " "))));
        }
        let (path, mut body) = body(cx, m, req).await?;
        let n = req.count.max(1);
        let mut out = GenOutput { seed: req.seed, ..Default::default() };
        let mut credits = None::<f64>;
        // One output per task.
        for i in 0..n {
            if m.seed
                && i > 0
                && let Some(s) = req.seed
            {
                body.insert("seed".into(), json!((s + i as i64).rem_euclid(4_294_967_296)));
            }
            let label = if n > 1 { format!("{} of {n}", i + 1) } else { "Generating".into() };
            let t = run_task(cx, path, &body, Duration::from_secs(m.secs), &label).await?;
            let urls = t.output.unwrap_or_default();
            if urls.is_empty() {
                return Err(util::decode_err(cx, "task succeeded without output"));
            }
            out.items.extend(urls.into_iter().map(|u| OutputItem::url(m.kind, u)));
            if let Some(c) = t.cost.as_ref().and_then(|c| c.get("credits")).and_then(Value::as_f64) {
                credits = Some(credits.unwrap_or(0.0) + c);
            }
        }
        out.cost_usd = credits.map(|c| c / 100.0);
        Ok(out)
    }
}
