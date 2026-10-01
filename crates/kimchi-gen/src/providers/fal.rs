//! fal: Hundreds of image and video models, fast queues.
//!
//! Every endpoint goes through the queue: `POST {base}/{endpoint}` →
//! `{request_id, status_url, response_url}`, poll `status_url?logs=1` until
//! `COMPLETED`, then `GET response_url`. Inputs differ per model, so a curated
//! table maps our request onto each endpoint's schema (verified against fal's
//! per-endpoint OpenAPI, Oct 2026). Extra endpoint ids listed in the `models`
//! provider option go through a generic best-effort mapping.
//!
//! Small input images are inlined as data URIs; larger ones are uploaded to
//! fal's CDN first (`rest.fal.ai/storage/upload/initiate`). The platform and
//! storage APIs live on other hosts than the queue; when the base URL is
//! overridden (tests, proxies) they are derived from it instead.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "fal";

pub struct Fal;

// ---- model table ------------------------------------------------------------

/// How an endpoint takes its output shape.
#[derive(Clone, Copy)]
enum Size {
    /// Shape follows the input image.
    None,
    /// `image_size: {width, height}` around this many megapixels.
    Pixels(f64),
    /// `image_size` preset name (`landscape_16_9`, …).
    Preset,
    /// `aspect_ratio` from this list.
    Ratio(&'static [&'static str]),
}

/// How an endpoint takes input images.
#[derive(Clone, Copy)]
enum Images {
    None,
    One(&'static str),
    /// Array field with a max count.
    Many(&'static str, u32),
}

#[derive(Clone, Copy)]
enum DurFmt {
    /// `"5"`
    Str,
    /// `"8s"`
    Secs,
    /// `5`
    Int,
}

#[derive(Clone, Copy)]
enum Durs {
    None,
    List(DurFmt, &'static [u32]),
    Range(DurFmt, u32, u32),
}

impl Durs {
    fn values(self) -> Vec<f64> {
        match self {
            Durs::None => vec![],
            Durs::List(_, v) => v.iter().map(|d| *d as f64).collect(),
            Durs::Range(_, lo, hi) => (lo..=hi).map(f64::from).collect(),
        }
    }

    fn encode(self, wanted: f64) -> Option<Value> {
        let (fmt, values) = match self {
            Durs::None => return None,
            Durs::List(f, _) | Durs::Range(f, ..) => (f, self.values()),
        };
        let d = util::closest_duration(Some(wanted), &values, wanted) as i64;
        Some(match fmt {
            DurFmt::Str => json!(d.to_string()),
            DurFmt::Secs => json!(format!("{d}s")),
            DurFmt::Int => json!(d),
        })
    }
}

/// One endpoint and how it takes its inputs.
#[derive(Clone, Copy)]
struct Endpoint {
    id: &'static str,
    size: Size,
    images: Images,
    /// Field for the last frame (image-to-video).
    end: Option<&'static str>,
}

const fn ep(id: &'static str, size: Size, images: Images, end: Option<&'static str>) -> Endpoint {
    Endpoint { id, size, images, end }
}

struct Spec {
    /// Model id shown to the user (the text endpoint, else the image one).
    id: &'static str,
    name: &'static str,
    description: &'static str,
    kind: OutputKind,
    /// Text-to-image / text-to-video endpoint.
    text: Option<Endpoint>,
    /// Image-to-image / image-to-video endpoint.
    image: Option<Endpoint>,
    /// Used instead of `image` when an end frame is given.
    first_last: Option<Endpoint>,
    durs: Durs,
    resolutions: &'static [&'static str],
    /// Field name for "generate sound".
    audio: Option<&'static str>,
    negative: bool,
    seed: bool,
    /// `num_images` max (1 = field not sent; repeat calls instead).
    batch: u32,
    /// Always-sent fields the schema requires.
    fixed: &'static [(&'static str, &'static str)],
    price: &'static str,
    featured: bool,
    /// Typical run time in seconds, for estimated progress.
    secs: u64,
}

const fn image(id: &'static str, name: &'static str, description: &'static str) -> Spec {
    Spec {
        id,
        name,
        description,
        kind: OutputKind::Image,
        text: None,
        image: None,
        first_last: None,
        durs: Durs::None,
        resolutions: &[],
        audio: None,
        negative: false,
        seed: true,
        batch: 1,
        fixed: &[],
        price: "",
        featured: false,
        secs: 15,
    }
}

const fn video(id: &'static str, name: &'static str, description: &'static str) -> Spec {
    Spec { kind: OutputKind::Video, secs: 120, ..image(id, name, description) }
}

const R_IMG: &[&str] = &["21:9", "16:9", "3:2", "4:3", "1:1", "3:4", "2:3", "9:16", "9:21"];
const R_GEMINI: &[&str] = &["21:9", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16"];
const R_WIDE_TALL: &[&str] = &["16:9", "9:16"];
const R_SQUARE3: &[&str] = &["16:9", "9:16", "1:1"];
const R_SEEDANCE: &[&str] = &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"];
const R_WAN: &[&str] = &["16:9", "4:3", "1:1", "3:4", "9:16"];
const R_PIXVERSE: &[&str] = &["21:9", "16:9", "3:2", "4:3", "1:1", "3:4", "2:3", "9:16"];
const R_FLUX3: &[&str] = &["21:9", "2:1", "16:9", "4:3", "1:1", "3:4", "9:16"];

const NONE: Size = Size::None;
const NO_IMG: Images = Images::None;

const SPECS: &[Spec] = &[
    // ---- images ----
    Spec {
        text: Some(ep("fal-ai/nano-banana-pro", Size::Ratio(R_GEMINI), NO_IMG, None)),
        image: Some(ep("fal-ai/nano-banana-pro/edit", Size::Ratio(R_GEMINI), Images::Many("image_urls", 14), None)),
        resolutions: &["1K", "2K", "4K"],
        batch: 4,
        price: "$0.15 / image (4K ×2)",
        featured: true,
        secs: 25,
        ..image("fal-ai/nano-banana-pro", "Nano Banana Pro", "Google Gemini 3 Pro Image: top prompt adherence, text and multi-image edits.")
    },
    Spec {
        text: Some(ep("fal-ai/nano-banana-2", Size::Ratio(R_GEMINI), NO_IMG, None)),
        image: Some(ep("fal-ai/nano-banana-2/edit", Size::Ratio(R_GEMINI), Images::Many("image_urls", 14), None)),
        resolutions: &["0.5K", "1K", "2K", "4K"],
        batch: 4,
        price: "$0.08 / image",
        ..image("fal-ai/nano-banana-2", "Nano Banana 2", "Gemini 3.1 Flash Image: fast generation and editing.")
    },
    Spec {
        text: Some(ep("fal-ai/flux-2-pro", Size::Pixels(1.0), NO_IMG, None)),
        image: Some(ep("fal-ai/flux-2-pro/edit", Size::Pixels(1.0), Images::Many("image_urls", 8), None)),
        price: "$0.03 first MP + $0.015 / MP",
        featured: true,
        ..image("fal-ai/flux-2-pro", "FLUX.2 [pro]", "Black Forest Labs' fast, high-quality model with multi-reference editing.")
    },
    Spec {
        text: Some(ep("fal-ai/flux-2-max", Size::Pixels(1.0), NO_IMG, None)),
        image: Some(ep("fal-ai/flux-2-max/edit", Size::Pixels(1.0), Images::Many("image_urls", 8), None)),
        price: "$0.07 first MP + $0.03 / MP",
        ..image("fal-ai/flux-2-max", "FLUX.2 [max]", "Black Forest Labs' highest-quality model.")
    },
    Spec {
        text: Some(ep("fal-ai/flux-2", Size::Pixels(1.0), NO_IMG, None)),
        image: Some(ep("fal-ai/flux-2/edit", Size::Pixels(1.0), Images::Many("image_urls", 4), None)),
        batch: 4,
        secs: 10,
        ..image("fal-ai/flux-2", "FLUX.2 [dev]", "Open-weights FLUX.2: cheap and fast.")
    },
    Spec {
        text: Some(ep("bytedance/seedream/v5/pro/text-to-image", Size::Pixels(2.0), NO_IMG, None)),
        image: Some(ep("bytedance/seedream/v5/pro/edit", Size::Pixels(2.0), Images::Many("image_urls", 10), None)),
        batch: 6,
        seed: false,
        price: "$0.0675 / image (≤ 2.4 MP)",
        secs: 25,
        ..image("bytedance/seedream/v5/pro/text-to-image", "Seedream 5.0 Pro", "ByteDance's 2K model; strong at typography and multi-image edits.")
    },
    Spec {
        text: Some(ep("openai/gpt-image-2", Size::Pixels(1.6), NO_IMG, None)),
        image: Some(ep("openai/gpt-image-2/edit", Size::Pixels(1.6), Images::Many("image_urls", 16), None)),
        batch: 4,
        seed: false,
        price: "token-based; quality sets cost",
        secs: 45,
        ..image("openai/gpt-image-2", "GPT Image 2", "OpenAI's image model: precise instructions, text, transparent backgrounds.")
    },
    Spec {
        text: Some(ep("alibaba/qwen-image-3/text-to-image", Size::Pixels(1.0), NO_IMG, None)),
        image: Some(ep("alibaba/qwen-image-3/edit", Size::Pixels(1.0), Images::Many("image_urls", 3), None)),
        batch: 6,
        negative: true,
        price: "$0.04 / image (1K)",
        ..image("alibaba/qwen-image-3/text-to-image", "Qwen Image 3", "Alibaba's model; excellent Chinese and English text rendering.")
    },
    Spec {
        image: Some(ep("fal-ai/flux-pro/kontext", Size::Ratio(R_IMG), Images::One("image_url"), None)),
        batch: 4,
        ..image("fal-ai/flux-pro/kontext", "FLUX.1 Kontext [pro]", "Fast, faithful single-image edits.")
    },
    Spec {
        text: Some(ep("ideogram/v4.5", Size::Preset, NO_IMG, None)),
        batch: 8,
        price: "$0.03–0.22 / image (by quality)",
        ..image("ideogram/v4.5", "Ideogram 4.5", "Graphic design and typography.")
    },
    Spec {
        text: Some(ep("fal-ai/recraft/v4.1/pro/text-to-image", Size::Preset, NO_IMG, None)),
        seed: false,
        ..image("fal-ai/recraft/v4.1/pro/text-to-image", "Recraft V4.1 Pro", "Design-grade images with brand colours.")
    },
    // ---- video ----
    Spec {
        text: Some(ep("fal-ai/veo3.1", Size::Ratio(R_WIDE_TALL), NO_IMG, None)),
        image: Some(ep("fal-ai/veo3.1/image-to-video", Size::Ratio(R_WIDE_TALL), Images::One("image_url"), None)),
        first_last: Some(ep(
            "fal-ai/veo3.1/first-last-frame-to-video",
            Size::Ratio(R_WIDE_TALL),
            Images::One("first_frame_url"),
            Some("last_frame_url"),
        )),
        durs: Durs::List(DurFmt::Secs, &[4, 6, 8]),
        resolutions: &["720p", "1080p", "4k"],
        audio: Some("generate_audio"),
        negative: true,
        price: "$0.20–0.60 / s",
        featured: true,
        ..video("fal-ai/veo3.1", "Veo 3.1", "Google's flagship video model with native sound.")
    },
    Spec {
        text: Some(ep("fal-ai/veo3.1/fast", Size::Ratio(R_WIDE_TALL), NO_IMG, None)),
        image: Some(ep("fal-ai/veo3.1/fast/image-to-video", Size::Ratio(R_WIDE_TALL), Images::One("image_url"), None)),
        first_last: Some(ep(
            "fal-ai/veo3.1/fast/first-last-frame-to-video",
            Size::Ratio(R_WIDE_TALL),
            Images::One("first_frame_url"),
            Some("last_frame_url"),
        )),
        durs: Durs::List(DurFmt::Secs, &[4, 6, 8]),
        resolutions: &["720p", "1080p", "4k"],
        audio: Some("generate_audio"),
        negative: true,
        price: "$0.10–0.35 / s",
        secs: 60,
        ..video("fal-ai/veo3.1/fast", "Veo 3.1 Fast", "Cheaper, faster Veo 3.1 with sound.")
    },
    Spec {
        text: Some(ep("fal-ai/kling-video/v3/pro/text-to-video", Size::Ratio(R_SQUARE3), NO_IMG, None)),
        image: Some(ep(
            "fal-ai/kling-video/v3/pro/image-to-video",
            NONE,
            Images::One("start_image_url"),
            Some("end_image_url"),
        )),
        durs: Durs::Range(DurFmt::Str, 3, 15),
        audio: Some("generate_audio"),
        negative: true,
        seed: false,
        price: "$0.11–0.17 / s",
        featured: true,
        ..video("fal-ai/kling-video/v3/pro/text-to-video", "Kling 3.0 Pro", "Cinematic motion, start/end frames, native audio.")
    },
    Spec {
        text: Some(ep("bytedance/seedance-2.5/text-to-video", Size::Ratio(R_SEEDANCE), NO_IMG, None)),
        image: Some(ep("bytedance/seedance-2.5/image-to-video", NONE, Images::One("image_url"), Some("end_image_url"))),
        durs: Durs::Range(DurFmt::Str, 4, 30),
        resolutions: &["480p", "720p", "1080p"],
        audio: Some("generate_audio"),
        seed: false,
        price: "$0.22–1.16 / s",
        featured: true,
        ..video("bytedance/seedance-2.5/text-to-video", "Seedance 2.5", "ByteDance's newest: up to 30 s with sound.")
    },
    Spec {
        text: Some(ep("bytedance/seedance-2.0/text-to-video", Size::Ratio(R_SEEDANCE), NO_IMG, None)),
        image: Some(ep(
            "bytedance/seedance-2.0/image-to-video",
            Size::Ratio(R_SEEDANCE),
            Images::One("image_url"),
            Some("end_image_url"),
        )),
        durs: Durs::Range(DurFmt::Str, 4, 15),
        resolutions: &["480p", "720p", "1080p", "4k"],
        audio: Some("generate_audio"),
        seed: false,
        price: "$0.30–0.68 / s",
        ..video("bytedance/seedance-2.0/text-to-video", "Seedance 2.0", "Multi-shot storytelling with sound, up to 4K.")
    },
    Spec {
        text: Some(ep("minimax/h3-max/text-to-video", Size::Ratio(R_SEEDANCE), NO_IMG, None)),
        image: Some(ep("minimax/h3-max/image-to-video", NONE, Images::One("image_url"), Some("end_image_url"))),
        durs: Durs::Range(DurFmt::Int, 5, 15),
        resolutions: &["480P", "768P", "1080P"],
        fixed: &[("prompt_expansion_mode", "balanced")],
        price: "$0.05–0.16 / s",
        ..video("minimax/h3-max/text-to-video", "MiniMax H3 Max", "Hailuo's successor: physics-heavy action, start/end frames.")
    },
    Spec {
        text: Some(ep("alibaba/wan-3.0/text-to-video", Size::Ratio(R_WAN), NO_IMG, None)),
        image: Some(ep("alibaba/wan-3.0/image-to-video", NONE, Images::One("start_image_url"), Some("end_image_url"))),
        durs: Durs::Range(DurFmt::Int, 2, 30),
        resolutions: &["480p", "720p", "1080p"],
        audio: Some("audio"),
        price: "$0.05–0.20 / s",
        ..video("alibaba/wan-3.0/text-to-video", "Wan 3.0", "Alibaba's model: long clips with sound at a low price.")
    },
    Spec {
        text: Some(ep("lightricks/ltx-2.5/text-to-video/pro", Size::Ratio(R_WIDE_TALL), NO_IMG, None)),
        image: Some(ep("lightricks/ltx-2.5/image-to-video/pro", NONE, Images::One("image_url"), Some("end_image_url"))),
        durs: Durs::List(DurFmt::Int, &[6, 8, 10]),
        resolutions: &["720p", "1080p"],
        audio: Some("generate_audio"),
        seed: false,
        price: "$0.12–0.17 / s",
        secs: 60,
        ..video("lightricks/ltx-2.5/text-to-video/pro", "LTX-2.5 Pro", "Fast, with camera-motion control and sound.")
    },
    Spec {
        text: Some(ep("blackforestlabs/flux-3/text-to-video", Size::Ratio(R_FLUX3), NO_IMG, None)),
        image: Some(ep("blackforestlabs/flux-3/image-to-video", Size::Ratio(R_FLUX3), Images::One("image_url"), None)),
        first_last: Some(ep(
            "blackforestlabs/flux-3/first-last-frame-to-video",
            Size::Ratio(R_FLUX3),
            Images::One("start_image_url"),
            Some("end_image_url"),
        )),
        durs: Durs::Range(DurFmt::Int, 5, 20),
        resolutions: &["720p", "1080p"],
        audio: Some("generate_audio"),
        seed: false,
        price: "$0.17–0.29 / s",
        ..video("blackforestlabs/flux-3/text-to-video", "FLUX.3 Video", "Black Forest Labs' video model with sound.")
    },
    Spec {
        text: Some(ep("google/gemini-omni-flash/v1.1/text-to-video", Size::Ratio(R_WIDE_TALL), NO_IMG, None)),
        image: Some(ep(
            "google/gemini-omni-flash/v1.1/image-to-video",
            Size::Ratio(R_WIDE_TALL),
            Images::One("image_url"),
            Some("end_image_url"),
        )),
        durs: Durs::Range(DurFmt::Int, 3, 10),
        resolutions: &["360p", "720p", "1080p", "4k"],
        seed: false,
        price: "$0.03–0.30 / s",
        secs: 60,
        ..video("google/gemini-omni-flash/v1.1/text-to-video", "Gemini Omni Flash", "Cheap, quick drafts from Google.")
    },
    Spec {
        text: Some(ep("fal-ai/pixverse/v6/text-to-video", Size::Ratio(R_PIXVERSE), NO_IMG, None)),
        image: Some(ep("fal-ai/pixverse/v6/image-to-video", NONE, Images::One("image_url"), None)),
        durs: Durs::Range(DurFmt::Int, 1, 15),
        resolutions: &["360p", "540p", "720p", "1080p"],
        audio: Some("generate_audio_switch"),
        negative: true,
        price: "$0.025–0.115 / s",
        secs: 60,
        ..video("fal-ai/pixverse/v6/text-to-video", "PixVerse v6", "Stylised effects and fast turnaround.")
    },
];

fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.id == id)
}

fn ratios(e: &Endpoint) -> Option<Vec<String>> {
    match e.size {
        Size::Ratio(r) => Some(r.iter().map(|x| x.to_string()).collect()),
        Size::Pixels(_) => Some(R_IMG.iter().map(|x| x.to_string()).collect()),
        Size::Preset => Some(["16:9", "4:3", "1:1", "3:4", "9:16"].map(String::from).to_vec()),
        Size::None => None,
    }
}

fn param(key: &str, label: &str, kind: ParamKind, default: Value) -> ParamSpec {
    ParamSpec { key: key.into(), label: label.into(), kind, default, help: None }
}

fn select(values: &[&str]) -> ParamKind {
    ParamKind::Select { options: values.iter().map(|v| SelectOption::new(*v, v.replace('_', " "))).collect() }
}

/// The few model-specific knobs worth surfacing; anything else in
/// `req.params` is passed through verbatim.
fn params_for(id: &str) -> Vec<ParamSpec> {
    let safety = |max: &str, def: &str| {
        let opts: Vec<String> = (1..=max.parse::<u32>().unwrap_or(6)).map(|n| n.to_string()).collect();
        let refs: Vec<&str> = opts.iter().map(String::as_str).collect();
        param("safety_tolerance", "Safety tolerance", select(&refs), json!(def))
    };
    match id {
        "fal-ai/nano-banana-pro" | "fal-ai/nano-banana-2" => vec![safety("6", "4")],
        "fal-ai/flux-2-pro" | "fal-ai/flux-2-max" => vec![safety("5", "2")],
        "fal-ai/flux-pro/kontext" => vec![safety("6", "2")],
        "openai/gpt-image-2" => vec![
            param("quality", "Quality", select(&["auto", "low", "medium", "high"]), json!("high")),
            param("background", "Background", select(&["auto", "transparent", "opaque"]), json!("auto")),
        ],
        "ideogram/v4.5" => vec![param("quality", "Quality", select(&["low", "medium", "high"]), json!("medium"))],
        "fal-ai/veo3.1" | "fal-ai/veo3.1/fast" => vec![safety("6", "4")],
        "fal-ai/kling-video/v3/pro/text-to-video" => {
            vec![param("cfg_scale", "Prompt adherence", ParamKind::Float { min: 0.0, max: 1.0, step: 0.05 }, json!(0.5))]
        }
        "minimax/h3-max/text-to-video" => {
            vec![param("prompt_expansion_mode", "Prompt expansion", select(&["disabled", "balanced", "quality"]), json!("balanced"))]
        }
        "lightricks/ltx-2.5/text-to-video/pro" => vec![param(
            "camera_motion",
            "Camera motion",
            select(&["static", "dolly_in", "dolly_out", "dolly_left", "dolly_right", "jib_up", "jib_down", "focus_shift"]),
            json!("static"),
        )],
        _ => vec![],
    }
}

fn tasks_of(s: &Spec) -> Vec<Task> {
    let (t, i) = match s.kind {
        OutputKind::Video => (Task::TextToVideo, Task::ImageToVideo),
        _ => (Task::TextToImage, Task::ImageToImage),
    };
    let mut v = vec![];
    if s.text.is_some() {
        v.push(t);
    }
    if s.image.is_some() {
        v.push(i);
    }
    v
}

fn model_info(s: &Spec) -> ModelInfo {
    let max_images = match s.image.map(|e| e.images) {
        Some(Images::One(_)) => 1,
        Some(Images::Many(_, n)) => n,
        _ => 0,
    };
    ModelInfo {
        description: Some(s.description.into()),
        aspect_ratios: s.text.as_ref().or(s.image.as_ref()).and_then(ratios).unwrap_or_default(),
        durations: s.durs.values(),
        resolutions: s.resolutions.iter().map(|r| r.to_string()).collect(),
        max_outputs: if s.kind == OutputKind::Image { 4 } else { 1 },
        negative_prompt: s.negative,
        seed: s.seed,
        end_frame: s.first_last.is_some() || s.image.is_some_and(|e| e.end.is_some()),
        max_images,
        audio: s.audio.is_some(),
        params: params_for(s.id),
        price: (!s.price.is_empty()).then(|| s.price.into()),
        featured: s.featured,
        ..ModelInfo::new(ID, s.id, s.name, &tasks_of(s))
    }
}

/// A model added by id through the `models` option, mapped generically.
fn custom_info(id: &str) -> ModelInfo {
    let video = ["video", "i2v", "t2v"].iter().any(|k| id.contains(k));
    let image_in = ["edit", "image-to", "i2v", "kontext", "img2img", "redux"].iter().any(|k| id.contains(k));
    let task = match (video, image_in) {
        (true, true) => Task::ImageToVideo,
        (true, false) => Task::TextToVideo,
        (false, true) => Task::ImageToImage,
        (false, false) => Task::TextToImage,
    };
    ModelInfo {
        description: Some("Custom fal endpoint (generic input mapping).".into()),
        aspect_ratios: R_IMG.iter().map(|x| x.to_string()).collect(),
        seed: true,
        negative_prompt: true,
        max_outputs: if video { 1 } else { 4 },
        ..ModelInfo::new(ID, id, id, &[task])
    }
}

fn custom_ids(cx: &Ctx) -> Vec<String> {
    match cx.options.get("models") {
        Some(Value::String(s)) => s.split([',', '\n', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(String::from).collect(),
        _ => vec![],
    }
}

// ---- input mapping ----------------------------------------------------------

/// Input images resolved to URLs (data URIs or CDN uploads), keeping roles.
struct Refs {
    start: Option<String>,
    end: Option<String>,
    refs: Vec<String>,
}

/// Inline small images; upload bigger ones so requests stay small.
const INLINE_LIMIT: usize = 1_500_000;

async fn resolve_images(cx: &Ctx, req: &GenRequest) -> GenResult<Refs> {
    let mut urls = Vec::with_capacity(req.images.len());
    for img in &req.images {
        urls.push(if img.data.len() <= INLINE_LIMIT { img.data_url() } else { upload(cx, img).await? });
    }
    let find = |role| req.images.iter().position(|i| i.role == role).map(|i| urls[i].clone());
    Ok(Refs {
        start: find(ImageRole::StartFrame).or_else(|| urls.first().cloned()),
        end: find(ImageRole::EndFrame),
        refs: req.images.iter().zip(&urls).filter(|(i, _)| i.role != ImageRole::EndFrame).map(|(_, u)| u.clone()).collect(),
    })
}

fn rest_base(cx: &Ctx) -> String {
    if cx.base_url.contains("queue.fal.run") { "https://rest.fal.ai".into() } else { cx.base_url.clone() }
}

fn api_base(cx: &Ctx) -> String {
    if cx.base_url.contains("queue.fal.run") { "https://api.fal.ai".into() } else { cx.base_url.clone() }
}

async fn upload(cx: &Ctx, img: &InputImage) -> GenResult<String> {
    #[derive(Deserialize)]
    struct Initiated {
        upload_url: String,
        file_url: String,
    }
    let url = format!("{}/storage/upload/initiate?storage_type=fal-cdn-v3", rest_base(cx));
    let init: Initiated = util::send_json(
        cx,
        cx.http.post(url).header("Authorization", auth(cx)?).json(&json!({"content_type": img.mime, "file_name": img.file_name()})),
    )
    .await?;
    util::send(cx, cx.http.put(&init.upload_url).header("Content-Type", &img.mime).body(img.data.clone())).await?;
    Ok(init.file_url)
}

fn input_for(s: &Spec, e: &Endpoint, req: &GenRequest, imgs: &Refs) -> Map<String, Value> {
    let mut b = Map::new();
    b.insert("prompt".into(), json!(req.prompt));
    match e.size {
        Size::None => {}
        Size::Pixels(mp) => {
            let ratio = match (req.width, req.height, &req.aspect_ratio) {
                (Some(w), Some(h), None) if w > 0 && h > 0 => format!("{w}:{h}"),
                _ => req.aspect(),
            };
            let (w, h) = util::size_for_ratio(&ratio, mp, 16);
            b.insert("image_size".into(), json!({"width": w, "height": h}));
        }
        Size::Preset => {
            let preset = match util::closest_ratio(&req.aspect(), &["16:9", "4:3", "1:1", "3:4", "9:16"]) {
                "16:9" => "landscape_16_9",
                "4:3" => "landscape_4_3",
                "3:4" => "portrait_4_3",
                "9:16" => "portrait_16_9",
                _ => "square_hd",
            };
            b.insert("image_size".into(), json!(preset));
        }
        Size::Ratio(list) => {
            b.insert("aspect_ratio".into(), json!(util::closest_ratio(&req.aspect(), list)));
        }
    }
    let inputs: Vec<&String> = if s.kind == OutputKind::Video { imgs.start.iter().collect() } else { imgs.refs.iter().collect() };
    match e.images {
        Images::None => {}
        Images::One(field) => {
            if let Some(u) = inputs.first() {
                b.insert(field.into(), json!(u));
            }
        }
        Images::Many(field, n) => {
            b.insert(field.into(), json!(inputs.iter().take(n as usize).collect::<Vec<_>>()));
        }
    }
    if let (Some(field), Some(u)) = (e.end, &imgs.end) {
        b.insert(field.into(), json!(u));
    }
    if let Some(v) = req.duration.and_then(|d| s.durs.encode(d)) {
        b.insert("duration".into(), v);
    }
    if let Some(r) = req.resolution.as_deref().and_then(|r| pick_resolution(s.resolutions, r)) {
        b.insert("resolution".into(), json!(r));
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
    if s.batch > 1 {
        b.insert("num_images".into(), json!(req.count.clamp(1, s.batch)));
    }
    for (k, v) in s.fixed {
        b.insert((*k).into(), json!(v));
    }
    for (k, v) in &req.params {
        b.insert(k.clone(), v.clone());
    }
    b
}

/// The allowed resolution matching `wanted` exactly (case-insensitive), else
/// the one with the closest pixel height.
fn pick_resolution(allowed: &[&'static str], wanted: &str) -> Option<&'static str> {
    if let Some(r) = allowed.iter().find(|r| r.eq_ignore_ascii_case(wanted)) {
        return Some(r);
    }
    let height = |r: &str| {
        let r = r.to_ascii_lowercase();
        if let Some(k) = r.strip_suffix('k') {
            k.parse::<f64>().ok().map(|n| n * 1080.0)
        } else {
            r.trim_end_matches('p').parse::<f64>().ok()
        }
    };
    let target = height(wanted)?;
    allowed.iter().copied().min_by(|a, b| {
        let d = |r: &str| height(r).map(|v| (v - target).abs()).unwrap_or(f64::MAX);
        d(a).total_cmp(&d(b))
    })
}

fn generic_input(req: &GenRequest, imgs: &Refs) -> Map<String, Value> {
    let mut b = Map::new();
    b.insert("prompt".into(), json!(req.prompt));
    b.insert("aspect_ratio".into(), json!(req.aspect()));
    if let Some(first) = imgs.refs.first() {
        b.insert("image_url".into(), json!(first));
        b.insert("image_urls".into(), json!(imgs.refs));
    }
    if let Some(end) = &imgs.end {
        b.insert("end_image_url".into(), json!(end));
    }
    if let Some(d) = req.duration {
        b.insert("duration".into(), json!(d.round() as i64));
    }
    if let Some(r) = &req.resolution {
        b.insert("resolution".into(), json!(r));
    }
    if let Some(a) = req.audio {
        b.insert("generate_audio".into(), json!(a));
    }
    if let Some(n) = req.negative_prompt.as_ref().filter(|n| !n.trim().is_empty()) {
        b.insert("negative_prompt".into(), json!(n));
    }
    if let Some(s) = req.seed {
        b.insert("seed".into(), json!(s));
    }
    if req.task.output() == OutputKind::Image && req.count > 1 {
        b.insert("num_images".into(), json!(req.count));
    }
    for (k, v) in &req.params {
        b.insert(k.clone(), v.clone());
    }
    b
}

// ---- queue ------------------------------------------------------------------

#[derive(Deserialize)]
struct Submitted {
    request_id: String,
    status_url: Option<String>,
    response_url: Option<String>,
}

#[derive(Deserialize)]
struct Status {
    status: String,
    queue_position: Option<u64>,
    #[serde(default)]
    logs: Option<Vec<Log>>,
    error: Option<String>,
    error_type: Option<String>,
    response_url: Option<String>,
}

#[derive(Deserialize)]
struct Log {
    #[serde(default)]
    message: String,
}

fn auth(cx: &Ctx) -> GenResult<String> {
    Ok(format!("Key {}", cx.key()?))
}

/// Status and result URLs drop the endpoint's subpath: `owner/app/sub` →
/// `owner/app` (`workflows/` and `comfy/` ids keep a third segment).
fn app_id(endpoint: &str) -> String {
    let n = if endpoint.starts_with("workflows/") || endpoint.starts_with("comfy/") { 3 } else { 2 };
    endpoint.split('/').take(n).collect::<Vec<_>>().join("/")
}

/// The latest `NN%` or `step a/b` found in the logs.
fn log_fraction(logs: &[Log]) -> Option<f64> {
    logs.iter().rev().find_map(|l| {
        let m = l.message.as_str();
        if let Some(i) = m.find('%') {
            let start = m[..i].rfind(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(0, |p| p + 1);
            if let Ok(p) = m[start..i].parse::<f64>()
                && p <= 100.0
            {
                return Some(p / 100.0);
            }
        }
        m.split_whitespace().find_map(|w| {
            let (a, b) = w.split_once('/')?;
            let (a, b) = (a.parse::<f64>().ok()?, b.trim_end_matches(|c: char| !c.is_ascii_digit()).parse::<f64>().ok()?);
            (b > 0.0 && a <= b).then_some(a / b)
        })
    })
}

fn is_policy(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["content_policy", "content policy", "nsfw", "moderation", "flagged", "safety filter", "safety checker"].iter().any(|k| m.contains(k))
}

fn moderated(e: GenError) -> GenError {
    match e {
        GenError::Http { message, .. } | GenError::Provider(message) if is_policy(&message) => GenError::Moderated(message),
        e => e,
    }
}

/// Like [`util::send_json`], but recognises fal's `{"detail": [{type: "content_policy_violation"}]}`.
async fn fal_json<T: serde::de::DeserializeOwned>(cx: &Ctx, req: reqwest::RequestBuilder) -> GenResult<T> {
    let resp = req.send().await.map_err(util::net_err(cx))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(util::net_err(cx))?;
    if (200..300).contains(&status) {
        return serde_json::from_str(&text).map_err(|e| util::decode_err(cx, format!("{e}: {}", util::truncate(&text, 300))));
    }
    if status == 401 || status == 403 {
        return Err(GenError::Unauthorized { provider: cx.provider.clone(), status });
    }
    let message = util::error_message(&text);
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let policy = v.pointer("/detail/0/type").and_then(Value::as_str).is_some_and(|t| t.contains("content_policy"));
    if policy || is_policy(&message) {
        return Err(GenError::Moderated(message));
    }
    Err(GenError::Http { provider: cx.provider.clone(), status, message })
}

async fn run(cx: &Ctx, endpoint: &str, input: &Map<String, Value>, expected: Duration, label: &str) -> GenResult<Value> {
    let auth = auth(cx)?;
    let sub: Submitted = fal_json(cx, cx.http.post(cx.url(&format!("/{endpoint}"))).header("Authorization", &auth).json(input)).await?;
    let base = format!("{}/{}/requests/{}", cx.base_url, app_id(endpoint), sub.request_id);
    let status_url = sub.status_url.unwrap_or_else(|| format!("{base}/status"));
    let response_url = sub.response_url.unwrap_or(base);
    let started = Instant::now();
    let every = if expected > Duration::from_secs(30) { Duration::from_secs(3) } else { Duration::from_millis(1500) };
    let response_url = util::poll(every, Duration::from_secs(60 * 30), || async {
        let st: Status = fal_json(cx, cx.http.get(&status_url).query(&[("logs", "1")]).header("Authorization", &auth)).await?;
        match st.status.as_str() {
            "IN_QUEUE" => {
                cx.report(Progress::message(match st.queue_position {
                    Some(n) if n > 0 => format!("In queue ({n} ahead)"),
                    _ => "In queue".to_string(),
                }));
                Ok(None)
            }
            "COMPLETED" => match st.error.filter(|e| !e.is_empty()) {
                Some(e) => Err(moderated(GenError::Provider(match st.error_type {
                    Some(t) => format!("fal: {e} ({t})"),
                    None => format!("fal: {e}"),
                }))),
                None => Ok(Some(st.response_url.unwrap_or_else(|| response_url.clone()))),
            },
            _ => {
                match st.logs.as_deref().and_then(log_fraction) {
                    Some(f) => cx.report(Progress::fraction(0.05 + f * 0.9, label)),
                    None => util::estimate(cx, started, expected, label),
                }
                Ok(None)
            }
        }
    })
    .await?;
    fal_json(cx, cx.http.get(&response_url).header("Authorization", &auth)).await
}

/// Collects output files from the common fal result shapes, dropping images
/// the safety checker blacked out.
fn outputs(cx: &Ctx, v: &Value, kind: OutputKind) -> GenResult<GenOutput> {
    let flagged: Vec<bool> = v
        .get("has_nsfw_concepts")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|b| b.as_bool().unwrap_or(false)).collect())
        .unwrap_or_default();
    let mut items = vec![];
    let mut push = |file: &Value, k: OutputKind| {
        if let Some(url) = file.get("url").and_then(Value::as_str).or_else(|| file.as_str()) {
            let k = file.get("content_type").and_then(Value::as_str).and_then(util::kind_for_mime).unwrap_or(k);
            items.push(OutputItem::url(k, url));
        }
    };
    if let Some(list) = v.get("images").and_then(Value::as_array) {
        for (i, img) in list.iter().enumerate() {
            if !flagged.get(i).copied().unwrap_or(false) {
                push(img, OutputKind::Image);
            }
        }
    }
    for (key, k) in [("image", OutputKind::Image), ("video", OutputKind::Video)] {
        if let Some(f) = v.get(key).filter(|f| !f.is_null()) {
            push(f, k);
        }
    }
    if let Some(list) = v.get("videos").and_then(Value::as_array) {
        list.iter().for_each(|f| push(f, OutputKind::Video));
    }
    if items.is_empty() {
        if flagged.iter().any(|f| *f) {
            return Err(GenError::Moderated("fal's safety checker flagged the output".into()));
        }
        let what = if kind == OutputKind::Video { "video" } else { "images" };
        return Err(util::decode_err(cx, format!("no {what} in result: {}", util::truncate(&v.to_string(), 200))));
    }
    Ok(GenOutput { items, seed: v.get("seed").and_then(Value::as_i64), cost_usd: None })
}

#[async_trait]
impl Provider for Fal {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "fal".into(),
            kind: ProviderKind::Cloud,
            tagline: "Hundreds of image and video models, fast queues".into(),
            website: "https://fal.ai".into(),
            needs_key: true,
            key_env: vec!["FAL_KEY".into(), "FAL_API_KEY".into()],
            key_url: Some("https://fal.ai/dashboard/keys".into()),
            key_hint: Some("key-id:key-secret".into()),
            default_base_url: "https://queue.fal.run".into(),
            base_url_editable: false,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let mut list: Vec<ModelInfo> = SPECS.iter().map(model_info).collect();
        for id in custom_ids(cx) {
            if !list.iter().any(|m| m.id == id) {
                list.push(custom_info(&id));
            }
        }
        Ok(list)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        // The pricing endpoint requires a valid key (listing models doesn't).
        let url = format!("{}/v1/models/pricing", api_base(cx));
        let _: Value =
            util::send_json(cx, cx.http.get(url).query(&[("endpoint_id", "fal-ai/flux-2-pro")]).header("Authorization", auth(cx)?)).await?;
        Ok("Key works".into())
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let kind = req.task.output();
        let imgs = resolve_images(cx, req).await?;
        let n = req.count.max(1);
        let Some(s) = spec(&req.model) else {
            let secs = if kind == OutputKind::Video { 120 } else { 15 };
            let v = run(cx, &req.model, &generic_input(req, &imgs), Duration::from_secs(secs), "Generating").await?;
            let mut out = outputs(cx, &v, kind)?;
            out.items.truncate(n as usize);
            return Ok(out);
        };
        let e = match (req.task.needs_image(), imgs.end.is_some()) {
            (false, _) => s.text,
            (true, true) => s.first_last.or(s.image),
            (true, false) => s.image,
        }
        .ok_or_else(|| GenError::Unsupported(format!("{} doesn't do {}", s.name, req.task.as_str().replace('_', " "))))?;
        if req.task.needs_image() && imgs.refs.is_empty() && imgs.start.is_none() {
            return Err(GenError::Provider("This task needs an input image.".into()));
        }
        let mut input = input_for(s, &e, req, &imgs);
        let expected = Duration::from_secs(s.secs);
        // Batch through `num_images` when the endpoint has it, else repeat.
        let calls = if s.batch > 1 { n.div_ceil(s.batch) } else { n };
        let mut out = GenOutput::default();
        for i in 0..calls {
            if i > 0
                && s.seed
                && let Some(seed) = req.seed
            {
                input.insert("seed".into(), json!(seed + i as i64));
            }
            let label = if calls > 1 { format!("{} of {calls}", i + 1) } else { "Generating".into() };
            let v = run(cx, e.id, &input, expected, &label).await?;
            let o = outputs(cx, &v, kind)?;
            out.seed = out.seed.or(o.seed);
            out.items.extend(o.items);
        }
        out.items.truncate(n as usize);
        Ok(out)
    }
}
