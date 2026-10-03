//! Black Forest Labs: FLUX, straight from the source.
//!
//! Every model is `POST /v1/{model}` → `{id, polling_url, cost}`. The result
//! must be polled from the returned `polling_url` (it may point at a regional
//! cluster) until `status == "Ready"`; `result.sample` is a signed URL that
//! expires after ~10 minutes, so the harness downloads it right away.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "bfl";

pub struct Bfl;

#[derive(Clone, Copy, PartialEq)]
enum Family {
    /// FLUX.2 [pro]/[max]: `width`/`height` (multiples of 16, ≤ 4 MP), up to
    /// 8 `input_image*`, prompt upsampling via `disable_pup`.
    Flux2,
    /// FLUX.2 [flex]: as above plus `steps`, `guidance`, `prompt_upsampling`.
    Flux2Flex,
    /// Kontext: `aspect_ratio` (3:7–7:3) and one `input_image`.
    Kontext,
    /// FLUX1.1 [pro] Ultra: `aspect_ratio` (21:9–9:21), `raw`.
    Ultra,
    /// FLUX1.1 [pro]: `width`/`height` 256–1440, multiples of 32.
    Pro11,
}

struct Model {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    family: Family,
    price: &'static str,
    featured: bool,
}

const MODELS: &[Model] = &[
    Model {
        id: "flux-2-pro",
        name: "FLUX.2 [pro]",
        description: "Fast, high-quality generation and multi-reference editing.",
        family: Family::Flux2,
        price: "from $0.03 / MP",
        featured: true,
    },
    Model {
        id: "flux-2-max",
        name: "FLUX.2 [max]",
        description: "BFL's best quality for generation and editing.",
        family: Family::Flux2,
        price: "from $0.07 / MP",
        featured: false,
    },
    Model {
        id: "flux-2-flex",
        name: "FLUX.2 [flex]",
        description: "Tunable steps and guidance; strongest typography.",
        family: Family::Flux2Flex,
        price: "from $0.05 / MP",
        featured: false,
    },
    Model {
        id: "flux-kontext-max",
        name: "FLUX.1 Kontext [max]",
        description: "Precise in-context editing with strong prompt adherence.",
        family: Family::Kontext,
        price: "≈ $0.08 / image",
        featured: false,
    },
    Model {
        id: "flux-kontext-pro",
        name: "FLUX.1 Kontext [pro]",
        description: "Fast in-context image editing and generation.",
        family: Family::Kontext,
        price: "≈ $0.04 / image",
        featured: false,
    },
    Model {
        id: "flux-pro-1.1-ultra",
        name: "FLUX1.1 [pro] Ultra",
        description: "4-megapixel images, optional raw photographic look.",
        family: Family::Ultra,
        price: "≈ $0.06 / image",
        featured: false,
    },
    Model {
        id: "flux-pro-1.1",
        name: "FLUX1.1 [pro]",
        description: "Fast, reliable text-to-image.",
        family: Family::Pro11,
        price: "≈ $0.04 / image",
        featured: false,
    },
];

const RATIOS: &[&str] = &["21:9", "16:9", "3:2", "4:3", "1:1", "3:4", "2:3", "9:16", "9:21"];

impl Family {
    fn max_refs(self) -> u32 {
        match self {
            Family::Flux2 | Family::Flux2Flex => 8,
            Family::Kontext => 1,
            Family::Ultra | Family::Pro11 => 0,
        }
    }

    fn max_tolerance(self) -> i64 {
        if matches!(self, Family::Flux2 | Family::Flux2Flex) { 5 } else { 6 }
    }

    fn formats(self) -> &'static [&'static str] {
        if matches!(self, Family::Flux2 | Family::Flux2Flex) { &["png", "jpeg", "webp"] } else { &["png", "jpeg"] }
    }
}

fn find(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

fn param(key: &str, label: &str, kind: ParamKind, default: Value, help: Option<&str>) -> ParamSpec {
    ParamSpec { key: key.into(), label: label.into(), kind, default, help: help.map(Into::into) }
}

fn model_info(m: &Model) -> ModelInfo {
    let f = m.family;
    let tasks: &[Task] = if f.max_refs() > 0 { &[Task::TextToImage, Task::ImageToImage] } else { &[Task::TextToImage] };
    let formats = f.formats().iter().map(|v| SelectOption::new(*v, v.to_uppercase())).collect();
    let mut params = vec![
        param(
            "safety_tolerance",
            "Safety tolerance",
            ParamKind::Int { min: 0, max: f.max_tolerance(), step: 1 },
            json!(2),
            Some("0 is strictest."),
        ),
        param("output_format", "Format", ParamKind::Select { options: formats }, json!("png"), None),
        param(
            "prompt_upsampling",
            "Prompt upsampling",
            ParamKind::Bool,
            json!(matches!(f, Family::Flux2 | Family::Flux2Flex)),
            Some("Let the model expand the prompt with more detail."),
        ),
    ];
    if f == Family::Ultra {
        params.push(param("raw", "Raw mode", ParamKind::Bool, json!(false), Some("Less processed, more natural-looking photos.")));
    }
    if f == Family::Flux2Flex {
        params.push(param("steps", "Steps", ParamKind::Int { min: 1, max: 50, step: 1 }, json!(50), None));
        params.push(param("guidance", "Guidance", ParamKind::Float { min: 1.5, max: 10.0, step: 0.1 }, json!(5.0), None));
    }
    ModelInfo {
        description: Some(m.description.into()),
        aspect_ratios: RATIOS.iter().map(|s| s.to_string()).collect(),
        max_outputs: 4,
        seed: true,
        max_images: f.max_refs(),
        params,
        price: Some(m.price.into()),
        featured: m.featured,
        ..ModelInfo::new(ID, m.id, m.name, tasks)
    }
}

/// Pixel size for the request: the exact project shape when only width/height
/// were given, else the aspect ratio; ~1 MP, snapped, longest side ≤ `max_side`.
fn pixels(req: &GenRequest, multiple: u32, max_side: u32) -> (u32, u32) {
    let ratio = match (req.width, req.height, &req.aspect_ratio) {
        (Some(w), Some(h), None) if w > 0 && h > 0 => format!("{w}:{h}"),
        _ => req.aspect(),
    };
    let (w, h) = util::size_for_ratio(&ratio, 1.0, multiple);
    if w.max(h) <= max_side {
        return (w, h);
    }
    let scale = max_side as f64 / w.max(h) as f64;
    let snap = |v: u32| (((v as f64 * scale) / multiple as f64).floor() as u32).max(1) * multiple;
    (snap(w), snap(h))
}

fn body(m: &Model, req: &GenRequest) -> GenResult<Map<String, Value>> {
    let f = m.family;
    let mut b = Map::new();
    b.insert("prompt".into(), json!(req.prompt));
    match f {
        Family::Flux2 | Family::Flux2Flex | Family::Pro11 => {
            let (w, h) = if f == Family::Pro11 { pixels(req, 32, 1440) } else { pixels(req, 16, 2048) };
            b.insert("width".into(), json!(w));
            b.insert("height".into(), json!(h));
        }
        Family::Kontext | Family::Ultra => {
            b.insert("aspect_ratio".into(), json!(util::closest_ratio(&req.aspect(), RATIOS)));
        }
    }
    if req.task == Task::ImageToImage {
        if f.max_refs() == 0 {
            return Err(GenError::Unsupported(format!("{} doesn't take input images", m.name)));
        }
        for (i, img) in req.references().take(f.max_refs() as usize).enumerate() {
            let key = if i == 0 { "input_image".to_string() } else { format!("input_image_{}", i + 1) };
            b.insert(key, json!(img.base64()));
        }
    }
    if let Some(seed) = req.seed {
        b.insert("seed".into(), json!(seed));
    }
    if let Some(v) = req.param("safety_tolerance").and_then(Value::as_i64) {
        b.insert("safety_tolerance".into(), json!(v.clamp(0, f.max_tolerance())));
    }
    let format = req.param("output_format").and_then(Value::as_str).filter(|v| f.formats().contains(v)).unwrap_or("png");
    b.insert("output_format".into(), json!(format));
    if let Some(up) = req.param("prompt_upsampling").and_then(Value::as_bool) {
        // FLUX.2 [pro]/[max] upsample unless told not to.
        if f == Family::Flux2 {
            b.insert("disable_pup".into(), json!(!up));
        } else {
            b.insert("prompt_upsampling".into(), json!(up));
        }
    }
    let extra: &[&str] = match f {
        Family::Ultra => &["raw"],
        Family::Flux2Flex => &["steps", "guidance"],
        _ => &[],
    };
    for key in extra {
        if let Some(v) = req.param(key) {
            b.insert((*key).into(), v.clone());
        }
    }
    Ok(b)
}

#[derive(Deserialize)]
struct Submitted {
    id: String,
    polling_url: Option<String>,
    /// Credits (1 credit = $0.01).
    cost: Option<f64>,
}

#[derive(Deserialize)]
struct Polled {
    status: String,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    progress: Option<f64>,
    #[serde(default)]
    details: Option<Value>,
}

struct One {
    url: String,
    seed: Option<i64>,
    credits: Option<f64>,
}

async fn run_one(cx: &Ctx, m: &Model, body: &Map<String, Value>, label: &str) -> GenResult<One> {
    let key = cx.key()?;
    let sub: Submitted = util::send_json(cx, cx.http.post(cx.url(&format!("/{}", m.id))).header("x-key", key).json(body)).await?;
    let poll_url = sub.polling_url.unwrap_or_else(|| cx.url(&format!("/get_result?id={}", sub.id)));
    let started = Instant::now();
    let result = util::poll(Duration::from_millis(1500), Duration::from_secs(600), || async {
        let p: Polled = util::send_json(cx, cx.http.get(&poll_url).header("x-key", key)).await?;
        match p.status.as_str() {
            "Ready" => Ok(Some(p.result.unwrap_or_default())),
            "Pending" | "Queued" | "Reasoning" | "Generating" => {
                match p.progress.filter(|v| *v > 0.0) {
                    // Documented as a number without a unit; accept 0–1 or 0–100.
                    Some(v) => cx.report(Progress::fraction(if v > 1.0 { v / 100.0 } else { v } * 0.95, label)),
                    None => util::estimate(cx, started, Duration::from_secs(15), label),
                }
                Ok(None)
            }
            "Request Moderated" | "Content Moderated" => Err(GenError::Moderated(moderation_reason(&p))),
            "Task not found" => Err(GenError::Provider("Black Forest Labs lost track of the task. Try again.".into())),
            other => Err(GenError::Provider(format!("Black Forest Labs: {other}{}", detail_suffix(&p)))),
        }
    })
    .await?;
    let url = result
        .get("sample")
        .and_then(Value::as_str)
        .ok_or_else(|| util::decode_err(cx, "result has no sample URL"))?
        .to_string();
    Ok(One { url, seed: result.get("seed").and_then(Value::as_i64), credits: sub.cost })
}

fn moderation_reason(p: &Polled) -> String {
    let what = if p.status.starts_with("Request") { "the prompt or input image" } else { "the generated image" };
    let reasons = p
        .details
        .as_ref()
        .and_then(|d| d.get("Moderation Reasons"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
        .filter(|s| !s.is_empty());
    match reasons {
        Some(r) => format!("{what} was flagged ({r})"),
        None => format!("{what} was flagged"),
    }
}

fn detail_suffix(p: &Polled) -> String {
    match &p.details {
        Some(d) if !d.is_null() => format!(" — {}", util::truncate(&d.to_string(), 200)),
        _ => String::new(),
    }
}

#[async_trait]
impl Provider for Bfl {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Black Forest Labs".into(),
            kind: ProviderKind::Cloud,
            tagline: "FLUX, straight from the source".into(),
            website: "https://bfl.ai".into(),
            needs_key: true,
            key_env: vec!["BFL_API_KEY".into()],
            key_url: Some("https://dashboard.bfl.ai".into()),
            key_hint: None,
            default_base_url: "https://api.bfl.ai/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage],
        }
    }

    async fn models(&self, _cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        Ok(MODELS.iter().map(model_info).collect())
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(cx, cx.http.get(cx.url("/credits")).header("x-key", cx.key()?)).await?;
        Ok(match v.get("credits").and_then(Value::as_f64) {
            Some(c) => format!("{c:.0} credits left"),
            None => "Key works".into(),
        })
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let m = find(&req.model).ok_or_else(|| GenError::Unsupported(format!("unknown Black Forest Labs model `{}`", req.model)))?;
        if !matches!(req.task, Task::TextToImage | Task::ImageToImage) {
            return Err(GenError::Unsupported(format!("{} only makes images", m.name)));
        }
        let mut body = body(m, req)?;
        let n = req.count.max(1);
        let mut out = GenOutput::default();
        let mut credits = None::<f64>;
        // One image per call; bump the seed so repeats differ.
        for i in 0..n {
            if let Some(s) = req.seed {
                body.insert("seed".into(), json!(s + i as i64));
            }
            let label = if n > 1 { format!("Image {} of {n}", i + 1) } else { "Generating".into() };
            let one = run_one(cx, m, &body, &label).await?;
            // Sample URLs expire after ~10 minutes, and a long batch can outlast that: fetch each
            // one now. The last goes straight to the harness, which downloads it at once.
            if i + 1 < n {
                let (data, mime) = util::download(cx, &one.url, &[]).await?;
                out.items.push(OutputItem::bytes(OutputKind::Image, data, mime));
            } else {
                out.items.push(OutputItem::url(OutputKind::Image, one.url));
            }
            out.seed = out.seed.or(one.seed);
            if let Some(c) = one.credits {
                credits = Some(credits.unwrap_or(0.0) + c);
            }
        }
        out.cost_usd = credits.map(|c| c / 100.0);
        Ok(out)
    }
}
