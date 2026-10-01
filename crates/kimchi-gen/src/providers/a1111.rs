//! Stable Diffusion WebUI: AUTOMATIC1111, Forge, SD.Next or Draw Things.
//!
//! They all speak the `/sdapi/v1` API:
//! * `GET  /sdapi/v1/sd-models` lists checkpoints; each becomes a model whose id is the checkpoint
//!   title, selected per request with `override_settings.sd_model_checkpoint`. Draw Things has no
//!   such route (nor `/progress`), so it gets a single [`CURRENT`] model: whatever is loaded in its UI.
//! * `GET  /sdapi/v1/samplers` and `/sdapi/v1/schedulers` fill the Advanced pickers.
//! * `POST /sdapi/v1/txt2img` / `img2img` take and return base64 images.
//! * `GET  /sdapi/v1/progress` is polled while a request runs; `POST /sdapi/v1/interrupt` on cancel.
//!
//! SD checkpoints fall apart at video-sized resolutions, so outputs keep the requested aspect ratio
//! (or the input image's) at the `megapixels` param, defaulted from the checkpoint name: about
//! 0.4 MP for SD 1.x, 1 MP for SDXL / Flux-class models.
//!
//! No provider options.
//!
//! The bottom of this file holds small helpers shared by the other local providers.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "a1111";

/// Model id for "whatever checkpoint the server has loaded" (Draw Things, or when listing fails).
pub const CURRENT: &str = "current";

pub struct A1111;

#[async_trait]
impl Provider for A1111 {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "Stable Diffusion WebUI".into(),
            kind: ProviderKind::Local,
            tagline: "AUTOMATIC1111, Forge, SD.Next or Draw Things".into(),
            website: "https://github.com/AUTOMATIC1111/stable-diffusion-webui".into(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: None,
            default_base_url: "http://127.0.0.1:7860".into(),
            base_url_editable: true,
            tasks: vec![Task::TextToImage, Task::ImageToImage],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let get = |path: &str| util::send_json::<Value>(cx, cx.http.get(cx.url(path)).timeout(Duration::from_secs(20)));
        let (ckpts, samplers, schedulers) =
            tokio::join!(get("/sdapi/v1/sd-models"), get("/sdapi/v1/samplers"), get("/sdapi/v1/schedulers"));
        let names = |v: GenResult<Value>| -> Vec<SelectOption> {
            let list = v.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            let mut opts = vec![SelectOption::new("", "Default")];
            opts.extend(list.iter().filter_map(|s| {
                let name = s["name"].as_str()?;
                Some(SelectOption::new(name, s["label"].as_str().unwrap_or(name)))
            }));
            opts
        };
        let (samplers, schedulers) = (names(samplers), names(schedulers));
        let model = |id: &str, name: &str, desc: String| {
            let d = checkpoint_defaults(name);
            ModelInfo {
                description: Some(desc),
                max_outputs: 4,
                negative_prompt: true,
                seed: true,
                params: sd_params(&d)
                    .into_iter()
                    .chain(select_param("sampler", "Sampler", &samplers, ""))
                    .chain(select_param("scheduler", "Scheduler", &schedulers, ""))
                    .collect(),
                ..ModelInfo::new(ID, id, name, &[Task::TextToImage, Task::ImageToImage])
            }
        };
        match ckpts {
            Ok(Value::Array(list)) => Ok(list
                .iter()
                .filter_map(|m| {
                    let title = m["title"].as_str()?;
                    let name = m["model_name"].as_str().unwrap_or(title);
                    Some(model(title, name, format!("Checkpoint {}", m["filename"].as_str().unwrap_or(title))))
                })
                .collect()),
            Err(e @ GenError::Network { .. }) => Err(offline(cx, "Stable Diffusion WebUI")(e)),
            // Draw Things (or a trimmed-down server): one entry for the loaded model.
            _ => {
                let opts = get("/sdapi/v1/options").await.map_err(offline(cx, "Stable Diffusion WebUI"))?;
                let loaded = loaded_model(&opts).unwrap_or("Current model");
                Ok(vec![model(CURRENT, loaded, "The model currently loaded in the app".into())])
            }
        }
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let opts: Value =
            util::send_json(cx, cx.http.get(cx.url("/sdapi/v1/options")).timeout(Duration::from_secs(10)))
                .await
                .map_err(offline(cx, "Stable Diffusion WebUI"))?;
        Ok(match loaded_model(&opts) {
            Some(m) => format!("Connected · {m}"),
            None => "Connected".into(),
        })
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let i2i = req.task == Task::ImageToImage;
        let current = req.model == CURRENT || req.model.is_empty();
        let d = checkpoint_defaults(&req.model);
        let init = if i2i {
            Some(req.start_frame().ok_or_else(|| GenError::Provider("Image-to-image needs an input image.".into()))?)
        } else {
            None
        };
        let dims = init.and_then(|i| image_dims(&i.data));
        let (width, height) = match dims {
            // Draw Things refuses init images whose size differs from width/height.
            Some(wh) if current => wh,
            _ => {
                let ratio = dims.map(|(w, h)| util::ratio_string(w, h)).unwrap_or_else(|| req.aspect());
                util::size_for_ratio(&ratio, param_f64(req, "megapixels").unwrap_or(d.megapixels), 64)
            }
        };
        let mut body = json!({
            "prompt": req.prompt,
            "negative_prompt": req.negative_prompt.clone().unwrap_or_default(),
            "seed": req.seed.unwrap_or(-1),
            "steps": param_f64(req, "steps").map_or(d.steps, |v| v as i64),
            "cfg_scale": param_f64(req, "cfg").unwrap_or(d.cfg),
            "width": width,
            "height": height,
            "batch_size": req.count,
            "n_iter": 1,
            "send_images": true,
            "save_images": false,
        });
        for (key, field) in [("sampler", "sampler_name"), ("scheduler", "scheduler")] {
            if let Some(v) = req.param(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                body[field] = json!(v);
            }
        }
        if !current {
            // Keep the checkpoint loaded afterwards: switching back and forth is slow.
            body["override_settings"] = json!({ "sd_model_checkpoint": req.model });
            body["override_settings_restore_afterwards"] = json!(false);
        }
        if let Some(img) = init {
            body["init_images"] = json!([img.base64()]);
            body["denoising_strength"] = json!(param_f64(req, "denoise").unwrap_or(0.6));
        }

        let path = if i2i { "/sdapi/v1/img2img" } else { "/sdapi/v1/txt2img" };
        let mut interrupt = {
            let (http, url) = (cx.http.clone(), cx.url("/sdapi/v1/interrupt"));
            OnDrop::new(move || {
                spawn_detached(async move {
                    http.post(url).send().await.ok();
                })
            })
        };
        let run = util::send_json::<Value>(cx, cx.http.post(cx.url(path)).json(&body));
        tokio::pin!(run);
        let started = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_millis(800));
        let mut has_progress = true;
        let resp = loop {
            tokio::select! {
                r = &mut run => break r,
                _ = tick.tick() => {
                    if !(has_progress && report_progress(cx).await) {
                        has_progress = false;
                        util::estimate(cx, started, Duration::from_secs(20), "Generating");
                    }
                }
            }
        };
        interrupt.disarm();
        let resp = resp?;

        // `info` is a JSON document serialised as a string.
        let info: Value = resp["info"].as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
        let images = resp["images"].as_array().cloned().unwrap_or_default();
        let items = images
            .iter()
            .filter_map(Value::as_str)
            // Forge/ControlNet append extra images (detected maps…) after the real ones.
            .take(req.count as usize)
            .map(|b| {
                let data = util::b64_decode(b).map_err(|e| util::decode_err(cx, format!("bad base64 image: {e}")))?;
                let mime = util::sniff_mime(&data).unwrap_or("image/png");
                Ok(OutputItem::bytes(OutputKind::Image, data, mime))
            })
            .collect::<GenResult<Vec<_>>>()?;
        Ok(GenOutput { items, seed: info["seed"].as_i64(), cost_usd: None })
    }
}

fn loaded_model(opts: &Value) -> Option<&str> {
    // A1111 / Forge / SD.Next report the checkpoint title; Draw Things its model file.
    opts["sd_model_checkpoint"].as_str().or_else(|| opts["model"].as_str()).filter(|s| !s.is_empty())
}

/// Reports real progress; false when the server has no progress route.
async fn report_progress(cx: &Ctx) -> bool {
    let req = cx.http.get(cx.url("/sdapi/v1/progress?skip_current_image=true")).timeout(Duration::from_secs(5));
    let Ok(p) = util::send_json::<Value>(cx, req).await else { return false };
    let fraction = p["progress"].as_f64().unwrap_or(0.0);
    let (step, steps) =
        (p["state"]["sampling_step"].as_i64().unwrap_or(0), p["state"]["sampling_steps"].as_i64().unwrap_or(0));
    let msg = if fraction <= 0.0 {
        "Loading model".to_string()
    } else {
        match p["eta_relative"].as_f64().filter(|e| *e > 0.5) {
            Some(eta) => format!("Sampling {step}/{steps} · ~{}s left", eta.round()),
            None => format!("Sampling {step}/{steps}"),
        }
    };
    cx.report(Progress::fraction(fraction * 0.97, msg));
    true
}

// ---- helpers shared by the local providers ---------------------------------

/// Turns "connection refused" into something a user can act on.
pub(crate) fn offline<'a>(cx: &'a Ctx, app: &'a str) -> impl Fn(GenError) -> GenError + 'a {
    move |e| match e {
        GenError::Network { provider, .. } => {
            GenError::Network { provider, message: format!("{app} isn't running at {}", cx.base_url) }
        }
        e => e,
    }
}

pub(crate) fn param_f64(req: &GenRequest, key: &str) -> Option<f64> {
    match req.param(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Sensible size and sampling defaults guessed from a checkpoint's file name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SdDefaults {
    pub megapixels: f64,
    pub steps: i64,
    pub cfg: f64,
}

pub(crate) fn checkpoint_defaults(name: &str) -> SdDefaults {
    let n = name.to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| n.contains(k));
    let big = has(&[
        "xl",
        "flux",
        "sd3",
        "pony",
        "illustrious",
        "noob",
        "playground",
        "hidream",
        "qwen",
        "z-image",
        "zimage",
        "kolors",
        "chroma",
        "lumina",
        "sana",
        "hunyuan",
        "auraflow",
    ]);
    let megapixels = if big { 1.0 } else { 0.4 };
    let (steps, cfg) = if has(&["schnell"]) {
        (4, 1.0)
    } else if has(&["turbo", "lightning", "hyper", "lcm", "dmd"]) {
        (8, 2.0)
    } else if has(&["flux", "chroma"]) {
        (25, 1.0)
    } else if big {
        (30, 6.0)
    } else {
        (25, 7.0)
    };
    SdDefaults { megapixels, steps, cfg }
}

pub(crate) fn param(key: &str, label: &str, kind: ParamKind, default: Value, help: &str) -> ParamSpec {
    ParamSpec { key: key.into(), label: label.into(), kind, default, help: (!help.is_empty()).then(|| help.into()) }
}

/// The knobs every Stable-Diffusion-style backend shares.
pub(crate) fn sd_params(d: &SdDefaults) -> Vec<ParamSpec> {
    vec![
        param(
            "megapixels",
            "Resolution (MP)",
            ParamKind::Float { min: 0.2, max: 4.0, step: 0.05 },
            json!(d.megapixels),
            "Output area; the aspect ratio follows the project or input image",
        ),
        param("steps", "Steps", ParamKind::Int { min: 1, max: 150, step: 1 }, json!(d.steps), ""),
        param(
            "cfg",
            "CFG scale",
            ParamKind::Float { min: 0.0, max: 30.0, step: 0.5 },
            json!(d.cfg),
            "Prompt adherence",
        ),
        param(
            "denoise",
            "Denoise strength",
            ParamKind::Float { min: 0.0, max: 1.0, step: 0.05 },
            json!(0.6),
            "Image-to-image only: how far to move away from the input image",
        ),
    ]
}

/// A picker, or nothing when the server gave no choices.
pub(crate) fn select_param(key: &str, label: &str, options: &[SelectOption], default: &str) -> Option<ParamSpec> {
    let default = options.iter().find(|o| o.value == default).or(options.first())?.value.clone();
    (options.len() > 1).then(|| param(key, label, ParamKind::Select { options: options.to_vec() }, json!(default), ""))
}

/// Pixel size read from a PNG, JPEG, GIF or WebP header.
pub(crate) fn image_dims(d: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]) as u32);
    let le16 = |i: usize| Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]) as u32);
    let le24 = |i: usize| Some(u32::from_le_bytes([*d.get(i)?, *d.get(i + 1)?, *d.get(i + 2)?, 0]));
    let dims = if d.starts_with(b"\x89PNG") {
        let be32 = |i: usize| Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?));
        (be32(16)?, be32(20)?)
    } else if d.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if d.len() > 30 && &d[0..4] == b"RIFF" && &d[8..12] == b"WEBP" {
        match &d[12..16] {
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            b"VP8L" => {
                let b = u32::from_le_bytes(d.get(21..25)?.try_into().ok()?);
                ((b & 0x3fff) + 1, ((b >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
            _ => return None,
        }
    } else if d.starts_with(b"\xFF\xD8") {
        // Walk JPEG segments until a start-of-frame marker.
        let mut i = 2;
        loop {
            while *d.get(i)? != 0xFF {
                i += 1;
            }
            let marker = *d.get(i + 1)?;
            if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                break (be16(i + 7)?, be16(i + 5)?);
            }
            i += 2 + be16(i + 2)? as usize;
        }
    } else {
        return None;
    };
    (dims.0 > 0 && dims.1 > 0).then_some(dims)
}

/// Runs a closure when dropped unless disarmed: used to tell a server to stop a job whose
/// future was dropped (the harness cancels by dropping).
pub(crate) struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    pub fn new(f: F) -> Self {
        Self(Some(f))
    }

    pub fn disarm(&mut self) {
        self.0 = None;
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f()
        }
    }
}

/// Fire-and-forget on the current runtime (no-op outside one).
pub(crate) fn spawn_detached(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(h) = tokio::runtime::Handle::try_current() {
        h.spawn(fut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dims() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(image_dims(&png), Some((640, 480)));
        // SOI, APP0 (len 4), SOF0 with height 300, width 200.
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8, 1, 44, 0, 200, 3];
        assert_eq!(image_dims(&jpeg), Some((200, 300)));
        assert_eq!(image_dims(b"nope"), None);
    }

    #[test]
    fn defaults() {
        assert_eq!(checkpoint_defaults("v1-5-pruned-emaonly.safetensors").megapixels, 0.4);
        assert_eq!(checkpoint_defaults("sd_xl_base_1.0.safetensors").megapixels, 1.0);
        assert_eq!(checkpoint_defaults("flux1-schnell-fp8.safetensors").steps, 4);
    }
}
