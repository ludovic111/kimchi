//! Replicate: Run open and proprietary models by the second.
//!
//! Official models: `POST /models/{owner}/{name}/predictions {input}`; other
//! models: `POST /predictions {version, input}`. Images ask the server to hold
//! the request (`Prefer: wait`) so fast models finish in one round trip; then
//! we poll `urls.get`. Files are sent as data URIs.
//!
//! A curated table maps requests onto the best official models' inputs
//! (verified against each model's OpenAPI schema, Oct 2026). Models from
//! Replicate's collections, or added by id in the `models` option, are mapped
//! by reading their input schema at run time.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "replicate";

pub struct Replicate;

// ---- curated models -----------------------------------------------------------

#[derive(Clone, Copy)]
enum Images {
    None,
    One(&'static str),
    Many(&'static str, u32),
}

#[derive(Clone, Copy)]
enum Ratio {
    None,
    /// `aspect_ratio` from this list.
    List(&'static [&'static str]),
    /// Sora: `portrait` / `landscape`.
    Orientation,
}

struct Spec {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    kind: OutputKind,
    /// Model run for text-only requests.
    text: Option<&'static str>,
    /// Model run when images are given.
    image: Option<&'static str>,
    images: Images,
    /// Field for the last frame.
    end: Option<&'static str>,
    ratio: Ratio,
    /// (`duration` or `seconds`, allowed integer values).
    durs: Option<(&'static str, &'static [u32])>,
    /// (field, [(label, value)]).
    res: Option<(&'static str, &'static [(&'static str, &'static str)])>,
    audio: Option<&'static str>,
    negative: bool,
    seed: bool,
    /// (field, max) for several outputs per prediction.
    count: Option<(&'static str, u32)>,
    fixed: &'static [(&'static str, &'static str)],
    featured: bool,
    secs: u64,
}

const fn img(id: &'static str, name: &'static str, description: &'static str) -> Spec {
    Spec {
        id,
        name,
        description,
        kind: OutputKind::Image,
        text: Some(id),
        image: None,
        images: Images::None,
        end: None,
        ratio: Ratio::None,
        durs: None,
        res: None,
        audio: None,
        negative: false,
        seed: false,
        count: None,
        fixed: &[],
        featured: false,
        secs: 15,
    }
}

const fn vid(id: &'static str, name: &'static str, description: &'static str) -> Spec {
    Spec { kind: OutputKind::Video, secs: 120, ..img(id, name, description) }
}

const R_GEMINI: &[&str] = &["21:9", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16"];
const R_FLUX2: &[&str] = &["16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16"];
const R_KONTEXT: &[&str] = &["21:9", "2:1", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16", "1:2", "9:21"];
const R_SEEDREAM: &[&str] = &["21:9", "16:9", "3:2", "4:3", "1:1", "3:4", "2:3", "9:16"];
const R_GPT: &[&str] = &["16:9", "3:2", "4:3", "1:1", "3:4", "2:3", "9:16"];
const R_IMAGEN: &[&str] = &["16:9", "4:3", "1:1", "3:4", "9:16"];
const R_SCHNELL: &[&str] = &["21:9", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16", "9:21"];
const R_IDEOGRAM: &[&str] = &["3:1", "2:1", "16:9", "16:10", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "10:16", "9:16", "1:2", "1:3"];
const R_WIDE_TALL: &[&str] = &["16:9", "9:16"];
const R_SQUARE3: &[&str] = &["16:9", "9:16", "1:1"];
const R_SEEDANCE: &[&str] = &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16", "9:21"];
const R_WAN: &[&str] = &["16:9", "4:3", "1:1", "3:4", "9:16"];
const R_RAY: &[&str] = &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"];
const R_GEN45: &[&str] = &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"];

const RES_K: &[(&str, &str)] = &[("1K", "1K"), ("2K", "2K"), ("4K", "4K")];
const RES_720_1080: &[(&str, &str)] = &[("720p", "720p"), ("1080p", "1080p")];

const SPECS: &[Spec] = &[
    // ---- images ----
    Spec {
        image: Some("google/nano-banana-pro"),
        images: Images::Many("image_input", 14),
        ratio: Ratio::List(R_GEMINI),
        res: Some(("resolution", RES_K)),
        featured: true,
        secs: 30,
        ..img("google/nano-banana-pro", "Nano Banana Pro", "Gemini 3 Pro Image: best prompt adherence, text and multi-image edits.")
    },
    Spec {
        image: Some("google/nano-banana-2"),
        images: Images::Many("image_input", 14),
        ratio: Ratio::List(R_GEMINI),
        res: Some(("resolution", RES_K)),
        ..img("google/nano-banana-2", "Nano Banana 2", "Gemini 3.1 Flash Image: fast generation and editing.")
    },
    Spec {
        image: Some("black-forest-labs/flux-2-pro"),
        images: Images::Many("input_images", 8),
        ratio: Ratio::List(R_FLUX2),
        seed: true,
        fixed: &[("output_format", "png")],
        featured: true,
        ..img("black-forest-labs/flux-2-pro", "FLUX.2 [pro]", "Fast, high-quality generation with up to 8 references.")
    },
    Spec {
        image: Some("black-forest-labs/flux-2-max"),
        images: Images::Many("input_images", 8),
        ratio: Ratio::List(R_FLUX2),
        seed: true,
        fixed: &[("output_format", "png")],
        ..img("black-forest-labs/flux-2-max", "FLUX.2 [max]", "Black Forest Labs' highest quality.")
    },
    Spec {
        text: None,
        image: Some("black-forest-labs/flux-kontext-pro"),
        images: Images::One("input_image"),
        ratio: Ratio::List(R_KONTEXT),
        seed: true,
        fixed: &[("output_format", "png")],
        ..img("black-forest-labs/flux-kontext-pro", "FLUX.1 Kontext [pro]", "Fast, faithful single-image edits.")
    },
    Spec {
        image: Some("bytedance/seedream-5-pro"),
        images: Images::Many("image_input", 10),
        ratio: Ratio::List(R_SEEDREAM),
        res: Some(("size", &[("1K", "1K"), ("1.5K", "1.5K"), ("2K", "2K")])),
        fixed: &[("output_format", "png")],
        secs: 30,
        ..img("bytedance/seedream-5-pro", "Seedream 5.0 Pro", "ByteDance's newest: typography and multi-image edits.")
    },
    Spec {
        image: Some("openai/gpt-image-2"),
        images: Images::Many("input_images", 16),
        ratio: Ratio::List(R_GPT),
        count: Some(("number_of_images", 10)),
        secs: 45,
        ..img("openai/gpt-image-2", "GPT Image 2", "OpenAI's image model: precise instructions and text.")
    },
    Spec {
        ratio: Ratio::List(R_IMAGEN),
        res: Some(("image_size", &[("1K", "1K"), ("2K", "2K")])),
        fixed: &[("output_format", "png")],
        ..img("google/imagen-4-ultra", "Imagen 4 Ultra", "Google's photoreal text-to-image.")
    },
    Spec {
        ratio: Ratio::List(R_IDEOGRAM),
        seed: true,
        ..img("ideogram-ai/ideogram-v3-turbo", "Ideogram 3.0 Turbo", "Graphic design and typography, fast.")
    },
    Spec {
        ratio: Ratio::List(R_SCHNELL),
        seed: true,
        count: Some(("num_outputs", 4)),
        fixed: &[("output_format", "png")],
        secs: 5,
        ..img("black-forest-labs/flux-schnell", "FLUX.1 [schnell]", "Very fast, very cheap drafts.")
    },
    // ---- video ----
    Spec {
        image: Some("google/veo-3.1"),
        images: Images::One("image"),
        end: Some("last_frame"),
        ratio: Ratio::List(R_WIDE_TALL),
        durs: Some(("duration", &[4, 6, 8])),
        res: Some(("resolution", RES_720_1080)),
        audio: Some("generate_audio"),
        negative: true,
        seed: true,
        featured: true,
        ..vid("google/veo-3.1", "Veo 3.1", "Google's flagship video model with native sound.")
    },
    Spec {
        image: Some("google/veo-3.1-fast"),
        images: Images::One("image"),
        end: Some("last_frame"),
        ratio: Ratio::List(R_WIDE_TALL),
        durs: Some(("duration", &[4, 6, 8])),
        res: Some(("resolution", RES_720_1080)),
        audio: Some("generate_audio"),
        negative: true,
        seed: true,
        secs: 60,
        ..vid("google/veo-3.1-fast", "Veo 3.1 Fast", "Cheaper, faster Veo 3.1 with sound.")
    },
    Spec {
        image: Some("kwaivgi/kling-v3-video"),
        images: Images::One("start_image"),
        end: Some("end_image"),
        ratio: Ratio::List(R_SQUARE3),
        durs: Some(("duration", &[3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])),
        res: Some(("mode", &[("720p", "standard"), ("1080p", "pro"), ("4K", "4k")])),
        audio: Some("generate_audio"),
        negative: true,
        featured: true,
        ..vid("kwaivgi/kling-v3-video", "Kling 3.0", "Cinematic motion, start/end frames, multi-shot.")
    },
    Spec {
        image: Some("bytedance/seedance-2.0"),
        images: Images::One("image"),
        end: Some("last_frame_image"),
        ratio: Ratio::List(R_SEEDANCE),
        durs: Some(("duration", &[4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])),
        res: Some(("resolution", &[("480p", "480p"), ("720p", "720p"), ("1080p", "1080p"), ("4K", "4k")])),
        audio: Some("generate_audio"),
        seed: true,
        featured: true,
        ..vid("bytedance/seedance-2.0", "Seedance 2.0", "Multi-shot storytelling with sound, up to 4K.")
    },
    Spec {
        image: Some("minimax/hailuo-2.3"),
        images: Images::One("first_frame_image"),
        durs: Some(("duration", &[6, 10])),
        res: Some(("resolution", &[("768p", "768p"), ("1080p", "1080p")])),
        ..vid("minimax/hailuo-2.3", "Hailuo 2.3", "MiniMax: dynamic, physics-heavy motion.")
    },
    Spec {
        text: Some("wan-video/wan-2.7-t2v"),
        image: Some("wan-video/wan-2.7-i2v"),
        images: Images::One("first_frame"),
        end: Some("last_frame"),
        ratio: Ratio::List(R_WAN),
        durs: Some(("duration", &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])),
        res: Some(("resolution", RES_720_1080)),
        negative: true,
        seed: true,
        ..vid("wan-video/wan-2.7-t2v", "Wan 2.7", "Alibaba's open model: long clips at a low price.")
    },
    Spec {
        image: Some("openai/sora-2"),
        images: Images::One("input_reference"),
        ratio: Ratio::Orientation,
        durs: Some(("seconds", &[4, 8, 12])),
        ..vid("openai/sora-2", "Sora 2", "OpenAI's video model with synced sound.")
    },
    Spec {
        image: Some("luma/ray-3.2"),
        images: Images::One("start_image"),
        end: Some("end_image"),
        ratio: Ratio::List(R_RAY),
        durs: Some(("duration", &[5, 10])),
        res: Some(("resolution", &[("540p", "540p"), ("720p", "720p"), ("1080p", "1080p")])),
        ..vid("luma/ray-3.2", "Luma Ray 3.2", "Physically realistic motion, HDR.")
    },
    Spec {
        image: Some("runwayml/gen-4.5"),
        images: Images::One("image"),
        ratio: Ratio::List(R_GEN45),
        durs: Some(("duration", &[5, 10])),
        seed: true,
        ..vid("runwayml/gen-4.5", "Runway Gen-4.5", "Realistic motion and strong prompt adherence.")
    },
    Spec {
        image: Some("pixverse/pixverse-v6"),
        images: Images::One("image"),
        end: Some("last_frame_image"),
        ratio: Ratio::List(R_SQUARE3),
        durs: Some(("duration", &[5, 8, 10, 15])),
        res: Some(("quality", &[("360p", "360p"), ("540p", "540p"), ("720p", "720p"), ("1080p", "1080p")])),
        audio: Some("generate_audio_switch"),
        negative: true,
        seed: true,
        secs: 60,
        ..vid("pixverse/pixverse-v6", "PixVerse v6", "Stylised effects and fast turnaround.")
    },
];

fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.id == id)
}

fn tasks_of(kind: OutputKind, text: bool, image: bool) -> Vec<Task> {
    let (t, i) = if kind == OutputKind::Video { (Task::TextToVideo, Task::ImageToVideo) } else { (Task::TextToImage, Task::ImageToImage) };
    [(text, t), (image, i)].into_iter().filter(|(on, _)| *on).map(|(_, t)| t).collect()
}

fn model_info(s: &Spec) -> ModelInfo {
    ModelInfo {
        description: Some(s.description.into()),
        aspect_ratios: match s.ratio {
            Ratio::List(r) => r.iter().map(|x| x.to_string()).collect(),
            Ratio::Orientation => vec!["16:9".into(), "9:16".into()],
            Ratio::None => vec![],
        },
        durations: s.durs.map(|(_, v)| v.iter().map(|d| *d as f64).collect()).unwrap_or_default(),
        resolutions: s.res.map(|(_, v)| v.iter().map(|(l, _)| l.to_string()).collect()).unwrap_or_default(),
        max_outputs: if s.kind == OutputKind::Image { 4 } else { 1 },
        negative_prompt: s.negative,
        seed: s.seed,
        end_frame: s.end.is_some(),
        max_images: match s.images {
            Images::None => 0,
            Images::One(_) => 1,
            Images::Many(_, n) => n,
        },
        audio: s.audio.is_some(),
        featured: s.featured,
        ..ModelInfo::new(ID, s.id, s.name, &tasks_of(s.kind, s.text.is_some(), s.image.is_some()))
    }
}

/// Picks the allowed (label, value) matching `wanted` by label, else by pixel height.
fn pick_res(allowed: &[(&str, &'static str)], wanted: &str) -> Option<&'static str> {
    if let Some((_, v)) = allowed.iter().find(|(l, _)| l.eq_ignore_ascii_case(wanted)) {
        return Some(v);
    }
    let height = |r: &str| {
        let r = r.to_ascii_lowercase();
        match r.strip_suffix('k') {
            Some(k) => k.parse::<f64>().ok().map(|n| n * 1080.0),
            None => r.trim_end_matches('p').parse::<f64>().ok(),
        }
    };
    let target = height(wanted)?;
    allowed
        .iter()
        .min_by(|a, b| {
            let d = |l: &str| height(l).map(|h| (h - target).abs()).unwrap_or(f64::MAX);
            d(a.0).total_cmp(&d(b.0))
        })
        .map(|(_, v)| *v)
}

fn input_for(s: &Spec, req: &GenRequest) -> Map<String, Value> {
    let mut b = Map::new();
    b.insert("prompt".into(), json!(req.prompt));
    let with_images = req.task.needs_image();
    match s.ratio {
        Ratio::None => {}
        Ratio::List(list) => {
            b.insert("aspect_ratio".into(), json!(util::closest_ratio(&req.aspect(), list)));
        }
        Ratio::Orientation => {
            let wide = util::parse_ratio(&req.aspect()).is_none_or(|(w, h)| w >= h);
            b.insert("aspect_ratio".into(), json!(if wide { "landscape" } else { "portrait" }));
        }
    }
    if with_images {
        match s.images {
            Images::None => {}
            Images::One(field) => {
                let first = if s.kind == OutputKind::Video { req.start_frame() } else { req.references().next() };
                if let Some(i) = first.filter(|i| i.role != ImageRole::EndFrame) {
                    b.insert(field.into(), json!(i.data_url()));
                }
            }
            Images::Many(field, n) => {
                b.insert(field.into(), json!(req.references().take(n as usize).map(InputImage::data_url).collect::<Vec<_>>()));
            }
        }
        if let (Some(field), Some(end)) = (s.end, req.image(ImageRole::EndFrame)) {
            b.insert(field.into(), json!(end.data_url()));
        }
    }
    if let (Some((field, allowed)), Some(d)) = (s.durs, req.duration) {
        let values: Vec<f64> = allowed.iter().map(|v| *v as f64).collect();
        b.insert(field.into(), json!(util::closest_duration(Some(d), &values, d) as i64));
    }
    if let Some(v) = s.res.zip(req.resolution.as_deref()).and_then(|((_, allowed), r)| pick_res(allowed, r)) {
        b.insert(s.res.map(|(f, _)| f).unwrap_or("resolution").into(), json!(v));
    }
    if let (Some(field), Some(on)) = (s.audio, req.audio) {
        b.insert(field.into(), json!(on));
    }
    if s.negative
        && let Some(n) = req.negative_prompt.as_ref().filter(|n| !n.trim().is_empty())
    {
        b.insert("negative_prompt".into(), json!(n));
    }
    if s.seed
        && let Some(seed) = req.seed
    {
        b.insert("seed".into(), json!(seed));
    }
    if let Some((field, max)) = s.count {
        b.insert(field.into(), json!(req.count.clamp(1, max)));
    }
    for (k, v) in s.fixed {
        b.insert((*k).into(), json!(v));
    }
    for (k, v) in &req.params {
        b.insert(k.clone(), v.clone());
    }
    b
}

// ---- generic models (collections / custom ids) -------------------------------

const COLLECTIONS: &[(&str, Task)] = &[
    ("text-to-image", Task::TextToImage),
    ("image-editing", Task::ImageToImage),
    ("text-to-video", Task::TextToVideo),
    ("image-to-video", Task::ImageToVideo),
];

#[derive(Deserialize)]
struct ModelEntry {
    owner: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    is_official: bool,
    latest_version: Option<Value>,
}

async fn collection(cx: &Ctx, slug: &str) -> GenResult<Vec<ModelEntry>> {
    #[derive(Deserialize)]
    struct Coll {
        #[serde(default)]
        models: Vec<ModelEntry>,
    }
    let c: Coll = util::send_json(cx, cx.http.get(cx.url(&format!("/collections/{slug}"))).bearer_auth(cx.key()?)).await?;
    Ok(c.models)
}

fn custom_ids(cx: &Ctx) -> Vec<String> {
    match cx.options.get("models") {
        Some(Value::String(s)) => s.split([',', '\n', ' ']).map(str::trim).filter(|s| s.contains('/')).map(String::from).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(String::from).collect(),
        _ => vec![],
    }
}

fn generic_info(id: &str, tasks: &[Task], description: Option<String>) -> ModelInfo {
    let image = tasks.iter().any(|t| t.output() == OutputKind::Image);
    ModelInfo {
        description: description.map(|d| util::truncate(&d, 200)),
        seed: true,
        max_outputs: if image { 4 } else { 1 },
        ..ModelInfo::new(ID, id, id, tasks)
    }
}

/// Input property names and their schemas, from a version's OpenAPI schema.
fn input_props(version: &Value) -> Map<String, Value> {
    let schemas = version.pointer("/openapi_schema/components/schemas");
    let mut props = schemas.and_then(|s| s.pointer("/Input/properties")).and_then(Value::as_object).cloned().unwrap_or_default();
    // Enums usually sit behind `allOf: [{$ref: "#/components/schemas/aspect_ratio"}]`.
    for p in props.values_mut() {
        let r = p.pointer("/allOf/0/$ref").and_then(Value::as_str).and_then(|r| r.rsplit('/').next()).map(str::to_string);
        if let (Some(name), Some(s)) = (r, schemas)
            && let Some(target) = s.get(&name)
            && let (Some(obj), Some(t)) = (p.as_object_mut(), target.as_object())
        {
            for (k, v) in t {
                obj.entry(k.clone()).or_insert(v.clone());
            }
        }
    }
    props
}

fn enum_strs(p: &Value) -> Vec<String> {
    p.get("enum").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Maps a request onto an arbitrary model's input schema by field names.
fn generic_input(props: &Map<String, Value>, req: &GenRequest) -> Map<String, Value> {
    let mut b = Map::new();
    let has = |k: &str| props.contains_key(k);
    let first = |keys: &[&'static str]| keys.iter().copied().find(|k| has(k));
    b.insert("prompt".into(), json!(req.prompt));
    if let Some(p) = props.get("aspect_ratio") {
        let options = enum_strs(p);
        let refs: Vec<&str> = options.iter().map(String::as_str).filter(|o| util::parse_ratio(o).is_some()).collect();
        if refs.is_empty() {
            b.insert("aspect_ratio".into(), json!(req.aspect()));
        } else {
            b.insert("aspect_ratio".into(), json!(util::closest_ratio(&req.aspect(), &refs)));
        }
    } else if has("width") && has("height") {
        let (w, h) = match (req.width, req.height) {
            (Some(w), Some(h)) => util::size_for_ratio(&format!("{w}:{h}"), 1.0, 16),
            _ => util::size_for_ratio(&req.aspect(), 1.0, 16),
        };
        b.insert("width".into(), json!(w));
        b.insert("height".into(), json!(h));
    }
    if req.task.needs_image() {
        let refs: Vec<String> = req.references().map(InputImage::data_url).collect();
        let start = req.start_frame().filter(|i| i.role != ImageRole::EndFrame).map(InputImage::data_url);
        let list_field = first(&["image_input", "input_images", "images", "reference_images", "image_urls"])
            .filter(|k| props[*k].get("type").and_then(Value::as_str) == Some("array"));
        let one_field = first(&["image", "input_image", "start_image", "first_frame_image", "first_frame", "input_reference", "image_url"]);
        match (one_field, list_field) {
            (Some(f), _) if req.task == Task::ImageToVideo || props[f].get("type").and_then(Value::as_str) != Some("array") => {
                if let Some(s) = start.or_else(|| refs.first().cloned()) {
                    b.insert(f.into(), json!(s));
                }
            }
            (_, Some(f)) => {
                b.insert(f.into(), json!(refs));
            }
            (Some(f), None) => {
                b.insert(f.into(), json!(refs));
            }
            (None, None) => {}
        }
        if let (Some(f), Some(end)) = (first(&["last_frame", "end_image", "last_frame_image", "tail_image"]), req.image(ImageRole::EndFrame)) {
            b.insert(f.into(), json!(end.data_url()));
        }
    }
    if let (Some(f), Some(d)) = (first(&["duration", "seconds"]), req.duration) {
        let p = &props[f];
        let allowed: Vec<f64> = p.get("enum").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let d = if allowed.is_empty() { d.round() } else { util::closest_duration(Some(d), &allowed, d) };
        let d = match (p.get("minimum").and_then(Value::as_f64), p.get("maximum").and_then(Value::as_f64)) {
            (Some(lo), Some(hi)) => d.clamp(lo, hi),
            _ => d,
        };
        b.insert(f.into(), json!(d as i64));
    }
    if let (Some(f), Some(r)) = (first(&["resolution"]), &req.resolution) {
        let options = enum_strs(&props[f]);
        let pick = options.iter().find(|o| o.eq_ignore_ascii_case(r)).cloned().unwrap_or_else(|| r.clone());
        b.insert(f.into(), json!(pick));
    }
    if let (Some(f), Some(a)) = (first(&["generate_audio", "audio"]).filter(|f| props[*f].get("type").and_then(Value::as_str) == Some("boolean")), req.audio) {
        b.insert(f.into(), json!(a));
    }
    if let (true, Some(n)) = (has("negative_prompt"), req.negative_prompt.as_ref().filter(|n| !n.trim().is_empty())) {
        b.insert("negative_prompt".into(), json!(n));
    }
    if let (true, Some(s)) = (has("seed"), req.seed) {
        b.insert("seed".into(), json!(s));
    }
    if let Some(f) = first(&["num_outputs", "number_of_images", "num_images"]) {
        let max = props[f].get("maximum").and_then(Value::as_u64).unwrap_or(4) as u32;
        b.insert(f.into(), json!(req.count.clamp(1, max)));
    }
    for (k, v) in &req.params {
        b.insert(k.clone(), v.clone());
    }
    b
}

// ---- predictions --------------------------------------------------------------

#[derive(Deserialize)]
struct Prediction {
    status: String,
    output: Option<Value>,
    error: Option<Value>,
    #[serde(default)]
    logs: Option<String>,
    urls: Option<Urls>,
}

#[derive(Deserialize)]
struct Urls {
    get: Option<String>,
}

/// Where to send a prediction.
enum Target {
    Model(String),
    Version(String),
}

fn is_policy(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["e005", "flagged as sensitive", "nsfw", "safety", "moderat", "content policy"].iter().any(|k| m.contains(k))
}

fn failure(p: &Prediction) -> GenError {
    let msg = match &p.error {
        Some(Value::String(s)) => s.clone(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => format!("prediction {}", p.status),
    };
    if is_policy(&msg) { GenError::Moderated(msg) } else { GenError::Provider(format!("Replicate: {msg}")) }
}

/// The latest tqdm percentage in the logs (`" 45%|████  | 9/20"`).
fn log_fraction(logs: &str) -> Option<f64> {
    logs.lines().rev().find_map(|l| {
        let (pct, rest) = l.trim_start().split_once('%')?;
        rest.trim_start().starts_with('|').then_some(())?;
        pct.trim().parse::<f64>().ok().filter(|p| *p <= 100.0).map(|p| p / 100.0)
    })
}

fn collect_urls(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.starts_with("http") || s.starts_with("data:") => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect_urls(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_urls(x, out)),
        _ => {}
    }
}

async fn predict(cx: &Ctx, target: &Target, input: &Map<String, Value>, kind: OutputKind, expected: Duration, label: &str) -> GenResult<Vec<String>> {
    let key = cx.key()?;
    let (url, body) = match target {
        Target::Model(id) => (cx.url(&format!("/models/{id}/predictions")), json!({"input": input})),
        Target::Version(v) => (cx.url("/predictions"), json!({"version": v, "input": input})),
    };
    let mut post = cx.http.post(url).bearer_auth(key).json(&body);
    if kind == OutputKind::Image {
        post = post.header("Prefer", "wait=30");
    }
    let first: Prediction = util::send_json(cx, post).await?;
    let started = Instant::now();
    let done = match first.status.as_str() {
        "succeeded" => first,
        "failed" | "canceled" | "aborted" => return Err(failure(&first)),
        // With `Prefer: wait`, file outputs can arrive before the status flips.
        _ if first.output.as_ref().is_some_and(|o| !o.is_null()) && kind == OutputKind::Image => first,
        _ => {
            let get = first.urls.and_then(|u| u.get).ok_or_else(|| util::decode_err(cx, "prediction has no urls.get"))?;
            let every = if kind == OutputKind::Video { Duration::from_secs(3) } else { Duration::from_millis(1500) };
            util::poll(every, Duration::from_secs(60 * 30), || async {
                let p: Prediction = util::send_json(cx, cx.http.get(&get).bearer_auth(key)).await?;
                match p.status.as_str() {
                    "succeeded" => Ok(Some(p)),
                    "failed" | "canceled" | "aborted" => Err(failure(&p)),
                    "starting" => {
                        cx.report(Progress::message("Starting"));
                        Ok(None)
                    }
                    _ => {
                        match p.logs.as_deref().and_then(log_fraction) {
                            Some(f) => cx.report(Progress::fraction(0.05 + f * 0.9, label)),
                            None => util::estimate(cx, started, expected, label),
                        }
                        Ok(None)
                    }
                }
            })
            .await?
        }
    };
    let mut urls = vec![];
    if let Some(o) = &done.output {
        collect_urls(o, &mut urls);
    }
    if urls.is_empty() {
        return Err(util::decode_err(cx, "prediction finished without output files"));
    }
    Ok(urls)
}

#[async_trait]
impl Provider for Replicate {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Replicate".into(),
            kind: ProviderKind::Cloud,
            tagline: "Run open and proprietary models by the second".into(),
            website: "https://replicate.com".into(),
            needs_key: true,
            key_env: vec!["REPLICATE_API_TOKEN".into()],
            key_url: Some("https://replicate.com/account/api-tokens".into()),
            key_hint: Some("r8_…".into()),
            default_base_url: "https://api.replicate.com/v1".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let mut list: Vec<ModelInfo> = SPECS.iter().map(model_info).collect();
        let curated = |id: &str| SPECS.iter().any(|s| s.id == id || s.text == Some(id) || s.image == Some(id));
        // Merge official models from Replicate's collections; skip silently when offline.
        if cx.api_key.is_some() {
            let lists = futures::future::join_all(COLLECTIONS.iter().map(|(slug, _)| collection(cx, slug))).await;
            for ((_, task), models) in COLLECTIONS.iter().zip(lists) {
                for m in models.unwrap_or_default().into_iter().filter(|m| m.is_official) {
                    let id = format!("{}/{}", m.owner, m.name);
                    if curated(&id) {
                        continue;
                    }
                    match list.iter_mut().find(|x| x.id == id) {
                        Some(existing) if !existing.tasks.contains(task) => existing.tasks.push(*task),
                        Some(_) => {}
                        None => list.push(generic_info(&id, &[*task], m.description)),
                    }
                }
            }
        }
        for id in custom_ids(cx) {
            let base = id.split(':').next().unwrap_or(&id);
            if !curated(base) && !list.iter().any(|m| m.id == id) {
                let all = [Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo];
                list.push(generic_info(&id, &all, Some("Custom Replicate model (inputs mapped from its schema).".into())));
            }
        }
        Ok(list)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let v: Value = util::send_json(cx, cx.http.get(cx.url("/account")).bearer_auth(cx.key()?)).await?;
        Ok(match v.get("username").and_then(Value::as_str) {
            Some(u) => format!("Signed in as {u}"),
            None => "Key works".into(),
        })
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let kind = req.task.output();
        let n = req.count.max(1);
        let (target, input, batch, expected) = match spec(&req.model) {
            Some(s) => {
                if s.kind != kind {
                    return Err(GenError::Unsupported(format!("{} doesn't do {}", s.name, req.task.as_str().replace('_', " "))));
                }
                let model = if req.task.needs_image() { s.image } else { s.text }
                    .ok_or_else(|| GenError::Unsupported(format!("{} doesn't do {}", s.name, req.task.as_str().replace('_', " "))))?;
                let batch = s.count.map_or(1, |(_, max)| max);
                (Target::Model(model.into()), input_for(s, req), batch, Duration::from_secs(s.secs))
            }
            None => {
                // `owner/name` or `owner/name:version`: read the schema to map inputs.
                let (name, pinned) = match req.model.split_once(':') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (req.model.as_str(), None),
                };
                let m: ModelEntry = util::send_json(cx, cx.http.get(cx.url(&format!("/models/{name}"))).bearer_auth(cx.key()?)).await?;
                let version = match &pinned {
                    Some(v) => util::send_json(cx, cx.http.get(cx.url(&format!("/models/{name}/versions/{v}"))).bearer_auth(cx.key()?)).await?,
                    None => m.latest_version.clone().unwrap_or_default(),
                };
                let props = input_props(&version);
                let input = if props.is_empty() { generic_input(&Map::new(), req) } else { generic_input(&props, req) };
                let batch = ["num_outputs", "number_of_images", "num_images"]
                    .iter()
                    .find_map(|f| props.get(*f))
                    .map_or(1, |p| p.get("maximum").and_then(Value::as_u64).unwrap_or(4) as u32);
                let version_id = pinned.or_else(|| version.get("id").and_then(Value::as_str).map(String::from));
                let target = match version_id {
                    Some(v) if !m.is_official => Target::Version(v),
                    _ => Target::Model(name.into()),
                };
                let secs = if kind == OutputKind::Video { 120 } else { 20 };
                (target, input, batch, Duration::from_secs(secs))
            }
        };
        let calls = if batch > 1 { n.div_ceil(batch) } else { n };
        let mut input = input;
        let mut out = GenOutput { seed: req.seed, ..Default::default() };
        for i in 0..calls {
            if i > 0
                && let (Some(seed), true) = (req.seed, input.contains_key("seed"))
            {
                input.insert("seed".into(), json!(seed + i as i64));
            }
            let label = if calls > 1 { format!("{} of {calls}", i + 1) } else { "Generating".into() };
            let urls = predict(cx, &target, &input, kind, expected, &label).await?;
            out.items.extend(urls.into_iter().map(|u| OutputItem::url(kind, u)));
        }
        out.items.truncate(n as usize);
        Ok(out)
    }
}
