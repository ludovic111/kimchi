//! ComfyUI: your own nodes and checkpoints, on your GPU.
//!
//! Endpoints: `GET /system_stats` (check), `GET /models/checkpoints` (or
//! `/object_info/CheckpointLoaderSimple` on old builds), `GET /object_info/KSampler` (samplers and
//! schedulers), `POST /upload/image`, `POST /prompt`, `GET /ws?clientId=` for live progress,
//! `GET /history/{id}`, `GET /view`, and `POST /queue {delete}` + `POST /interrupt` on cancel.
//! When the websocket can't be opened (e.g. an https reverse proxy) we poll `/history` instead.
//!
//! # Models
//! * `ckpt:<file>`: every installed checkpoint, run through the built-in SD-style graphs next to
//!   this file (`text_to_image.json`, `image_to_image.json`).
//! * `workflow:<file>`: API-format workflow JSON files (ComfyUI: *Workflow → Export (API)*) dropped
//!   in the workflows folder. ComfyUI's own saved workflows (`/api/userdata?dir=workflows`) are
//!   UI-format graphs that only the browser frontend can convert, so they aren't listed.
//!
//! # Options
//! * `workflows_dir`: folder of custom workflows. Default `~/Documents/kimchi/comfyui-workflows`.
//!
//! # Placeholders
//! Inside any string value of a workflow, `{{name}}` is replaced. A value that is exactly one
//! numeric placeholder becomes a JSON number.
//!
//! | placeholder | value |
//! |---|---|
//! | `{{prompt}}`, `{{negative_prompt}}` | the prompts (negative defaults to empty) |
//! | `{{seed}}` | the request seed, or a random one (returned with the result) |
//! | `{{width}}`, `{{height}}` | project / input-image aspect ratio at `megapixels` (default 1), multiples of 16 |
//! | `{{count}}` | outputs wanted, for batch sizes |
//! | `{{steps}}`, `{{cfg}}`, `{{denoise}}` | sampling knobs (defaults 20, 7, 0.75) |
//! | `{{duration}}`, `{{fps}}`, `{{frames}}` | seconds (default 5), frame rate (default 24), `round(duration × fps)` |
//! | `{{image}}` = `{{start_image}}`, `{{end_image}}` | uploaded file names, for `LoadImage` nodes |
//! | `{{sampler}}`, `{{scheduler}}`, `{{checkpoint}}` | strings (built-in graphs) |
//! | `{{<key>}}` | any value in the request's `params`, e.g. one declared by the sidecar |
//!
//! Tasks are inferred: video when the graph has a video output node (`SaveVideo`,
//! `VHS_VideoCombine`, `SaveAnimatedWEBP`…), image otherwise; "to video"/"to image" from an
//! image placeholder. An optional sidecar `<name>.kimchi.json` overrides that:
//! `{"name", "description", "tasks": ["image_to_video"], "defaults": {"steps": 30, "fps": 16, …},
//! "params": [ParamSpec…]}`.

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::a1111::{OnDrop, checkpoint_defaults, image_dims, offline, param, sd_params, select_param, spawn_detached};
use crate::provider::{Ctx, GenError, GenResult, Provider};
use crate::types::*;
use crate::util;

pub const ID: &str = "comfyui";

const TEXT_TO_IMAGE: &str = include_str!("text_to_image.json");
const IMAGE_TO_IMAGE: &str = include_str!("image_to_image.json");

/// Output nodes that make a workflow a video workflow.
const VIDEO_NODES: &[&str] =
    &["SaveVideo", "SaveWEBM", "SaveAnimatedWEBP", "SaveAnimatedPNG", "CreateVideo", "VHS_VideoCombine"];

/// Placeholders whose value is an integer.
const INT_VARS: &[&str] = &["seed", "width", "height", "steps", "frames", "fps", "count"];

pub struct ComfyUi;

#[async_trait]
impl Provider for ComfyUi {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "ComfyUI".into(),
            kind: ProviderKind::Local,
            tagline: "Your own nodes and checkpoints, on your GPU".into(),
            website: "https://www.comfy.org".into(),
            needs_key: false,
            key_env: vec![],
            key_url: None,
            key_hint: None,
            default_base_url: "http://127.0.0.1:8188".into(),
            base_url_editable: true,
            tasks: vec![Task::TextToImage, Task::ImageToImage, Task::TextToVideo, Task::ImageToVideo],
        }
    }

    async fn models(&self, cx: &Ctx) -> GenResult<Vec<ModelInfo>> {
        let ckpts = checkpoints(cx).await.map_err(offline(cx, "ComfyUI"))?;
        let ks: Value = util::send_json(cx, cx.http.get(cx.url("/object_info/KSampler"))).await.unwrap_or_default();
        let inputs = &ks["KSampler"]["input"]["required"];
        let opts = |v: &Value| combo(v).into_iter().map(|s| SelectOption::new(s.clone(), s)).collect::<Vec<_>>();
        let (samplers, schedulers) = (opts(&inputs["sampler_name"]), opts(&inputs["scheduler"]));

        let mut models: Vec<ModelInfo> = load_workflows(&workflows_dir(cx)).iter().map(Workflow::model).collect();
        models.extend(ckpts.iter().map(|file| {
            let d = checkpoint_defaults(file);
            let (sampler, scheduler) = default_sampler(file);
            let stem = Path::new(file).file_stem().map_or(file.clone(), |s| s.to_string_lossy().into_owned());
            ModelInfo {
                description: Some(format!("Checkpoint {file} · built-in workflow")),
                max_outputs: 4,
                negative_prompt: true,
                seed: true,
                params: sd_params(&d)
                    .into_iter()
                    .chain(select_param("sampler", "Sampler", &samplers, sampler))
                    .chain(select_param("scheduler", "Scheduler", &schedulers, scheduler))
                    .collect(),
                ..ModelInfo::new(ID, format!("ckpt:{file}"), stem, &[Task::TextToImage, Task::ImageToImage])
            }
        }));
        Ok(models)
    }

    async fn check(&self, cx: &Ctx) -> GenResult<String> {
        let req = cx.http.get(cx.url("/system_stats")).timeout(Duration::from_secs(10));
        let s: Value = util::send_json(cx, req).await.map_err(offline(cx, "ComfyUI"))?;
        let mut line = match s["system"]["comfyui_version"].as_str() {
            Some(v) => format!("ComfyUI {v}"),
            None => "ComfyUI".into(),
        };
        if let Some(dev) = s["devices"].get(0) {
            let name = dev["name"].as_str().unwrap_or("?");
            // Device names look like "cuda:0 NVIDIA GeForce RTX 4090 : cudaMallocAsync".
            let name = name.split(" : ").next().unwrap_or(name);
            let name = name.split_once(' ').filter(|(d, _)| d.contains(':')).map_or(name, |(_, n)| n);
            line.push_str(&format!(" · {name}"));
            if let Some(vram) = dev["vram_total"].as_f64().filter(|v| *v > 0.0) {
                line.push_str(&format!(" ({:.0} GB)", vram / 1e9));
            }
        }
        Ok(line)
    }

    async fn generate(&self, cx: &Ctx, req: &GenRequest) -> GenResult<GenOutput> {
        let (mut graph, defaults) = if let Some(ckpt) = req.model.strip_prefix("ckpt:") {
            let template = if req.task == Task::ImageToImage { IMAGE_TO_IMAGE } else { TEXT_TO_IMAGE };
            let graph =
                serde_json::from_str(template).map_err(|e| GenError::Provider(format!("built-in workflow: {e}")))?;
            let d = checkpoint_defaults(ckpt);
            let (sampler, scheduler) = default_sampler(ckpt);
            let defaults = json!({
                "megapixels": d.megapixels, "steps": d.steps, "cfg": d.cfg, "denoise": 0.6,
                "sampler": sampler, "scheduler": scheduler, "checkpoint": ckpt,
            });
            (graph, defaults.as_object().cloned().unwrap_or_default())
        } else if let Some(file) = req.model.strip_prefix("workflow:") {
            let w = Workflow::load(&workflows_dir(cx), file)?;
            let mut defaults = w.side.defaults;
            for p in w.side.params {
                defaults.entry(p.key).or_insert(p.default);
            }
            (w.graph, defaults)
        } else {
            return Err(GenError::Unsupported(format!("unknown ComfyUI model `{}`", req.model)));
        };

        let used = placeholders(&graph);
        let start = req.start_frame().filter(|_| req.task.needs_image());
        let mut vars = build_vars(req, defaults, start.and_then(|i| image_dims(&i.data)));
        for (keys, img, what) in [
            (&["image", "start_image"][..], start, "an input image"),
            (&["end_image"][..], req.image(ImageRole::EndFrame), "an end frame"),
        ] {
            if !keys.iter().any(|k| used.contains(*k)) {
                continue;
            }
            let img = img.ok_or_else(|| GenError::Provider(format!("This workflow needs {what}.")))?;
            cx.report(Progress::message("Uploading image"));
            let name = upload(cx, img).await?;
            for k in keys {
                vars.insert((*k).into(), json!(name));
            }
        }
        substitute(&mut graph, &vars);
        let seed = vars.get("seed").and_then(Value::as_i64);

        // Open the socket before queueing so no message is missed.
        let client_id = uuid::Uuid::new_v4().simple().to_string();
        let ws = connect_ws(cx, &client_id).await;
        let id = queue_prompt(cx, &graph, &client_id).await?;
        let mut cancel = {
            let (http, base, id) = (cx.http.clone(), cx.base_url.clone(), id.clone());
            OnDrop::new(move || {
                spawn_detached(async move {
                    http.post(format!("{base}/queue")).json(&json!({ "delete": [id] })).send().await.ok();
                    http.post(format!("{base}/interrupt")).json(&json!({ "prompt_id": id })).send().await.ok();
                })
            })
        };
        let started = Instant::now();
        let live = match ws {
            Some(ws) => watch(cx, ws, &id, &graph).await,
            None => Ok(false),
        };
        let entry = match live {
            // History is written just before the final "executing" message; allow a little slack.
            Ok(true) => history(cx, &id, Duration::from_millis(250), Duration::from_secs(30), None).await,
            Ok(false) => history(cx, &id, Duration::from_secs(1), Duration::from_secs(6 * 3600), Some(started)).await,
            Err(e) => Err(e),
        };
        cancel.disarm();
        let items = collect_outputs(cx, &entry?, req.task.output() == OutputKind::Video);
        Ok(GenOutput { items, seed, cost_usd: None })
    }
}

// ---- discovery -------------------------------------------------------------

async fn checkpoints(cx: &Ctx) -> GenResult<Vec<String>> {
    match util::send_json::<Vec<String>>(
        cx,
        cx.http.get(cx.url("/models/checkpoints")).timeout(Duration::from_secs(20)),
    )
    .await
    {
        Ok(list) => Ok(list),
        // Builds older than the /models route.
        Err(GenError::Http { .. }) => {
            let v: Value = util::send_json(cx, cx.http.get(cx.url("/object_info/CheckpointLoaderSimple"))).await?;
            Ok(combo(&v["CheckpointLoaderSimple"]["input"]["required"]["ckpt_name"]))
        }
        Err(e) => Err(e),
    }
}

/// Choices of a combo input: `[[…choices], {…}]`, or `["COMBO", {"options": […]}]` for v3 nodes.
fn combo(input: &Value) -> Vec<String> {
    let list = match &input[0] {
        Value::Array(a) => a,
        _ => input[1]["options"].as_array().map_or(&[][..], Vec::as_slice),
    };
    list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

fn default_sampler(ckpt: &str) -> (&'static str, &'static str) {
    let n = ckpt.to_ascii_lowercase();
    if ["flux", "chroma"].iter().any(|k| n.contains(k)) {
        ("euler", "simple")
    } else if ["turbo", "lightning", "hyper", "lcm", "dmd"].iter().any(|k| n.contains(k)) {
        ("euler", "sgm_uniform")
    } else {
        ("dpmpp_2m", "karras")
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

fn workflows_dir(cx: &Ctx) -> PathBuf {
    match cx.option_str("workflows_dir").map(str::trim).filter(|s| !s.is_empty()) {
        Some("~") => home(),
        Some(p) => match p.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => PathBuf::from(p),
        },
        None => home().join("Documents").join("kimchi").join("comfyui-workflows"),
    }
}

/// Optional `<name>.kimchi.json` next to a workflow.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Sidecar {
    name: Option<String>,
    description: Option<String>,
    tasks: Option<Vec<Task>>,
    /// Default values for placeholders (`steps`, `fps`, `megapixels`…).
    defaults: Map<String, Value>,
    /// Extra knobs shown in the Advanced section, substituted as `{{key}}`.
    params: Vec<ParamSpec>,
}

struct Workflow {
    file: String,
    graph: Value,
    side: Sidecar,
}

impl Workflow {
    fn load(dir: &Path, file: &str) -> GenResult<Self> {
        if file.contains(['/', '\\']) || file.starts_with('.') {
            return Err(GenError::Provider(format!("invalid workflow name `{file}`")));
        }
        let path = dir.join(file);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| GenError::Provider(format!("Couldn't read workflow {}: {e}", path.display())))?;
        let graph: Value =
            serde_json::from_str(&text).map_err(|e| GenError::Provider(format!("{file} isn't valid JSON: {e}")))?;
        let is_api =
            graph.as_object().is_some_and(|o| !o.is_empty() && o.values().all(|n| n["class_type"].is_string()));
        if !is_api {
            return Err(GenError::Provider(format!(
                "{file} isn't an API-format workflow. In ComfyUI use Workflow → Export (API)."
            )));
        }
        let side_path = path.with_extension("kimchi.json");
        let side = match std::fs::read_to_string(&side_path) {
            Ok(s) => {
                serde_json::from_str(&s).map_err(|e| GenError::Provider(format!("{}: {e}", side_path.display())))?
            }
            Err(_) => Sidecar::default(),
        };
        Ok(Self { file: file.to_string(), graph, side })
    }

    fn model(&self) -> ModelInfo {
        let used = placeholders(&self.graph);
        let has = |k: &str| used.contains(k);
        let video = self
            .graph
            .as_object()
            .is_some_and(|o| o.values().any(|n| n["class_type"].as_str().is_some_and(|c| VIDEO_NODES.contains(&c))));
        let image_in = has("image") || has("start_image");
        let tasks = self.side.tasks.clone().unwrap_or_else(|| {
            vec![match (video, image_in) {
                (true, true) => Task::ImageToVideo,
                (true, false) => Task::TextToVideo,
                (false, true) => Task::ImageToImage,
                (false, false) => Task::TextToImage,
            }]
        });
        let stem = self.file.trim_end_matches(".json");
        let name = self.side.name.clone().unwrap_or_else(|| stem.replace(['_', '-'], " "));
        let d = |k: &str, fallback: Value| self.side.defaults.get(k).cloned().unwrap_or(fallback);
        let mut params = vec![];
        if has("width") || has("height") {
            let kind = ParamKind::Float { min: 0.1, max: 4.0, step: 0.05 };
            params.push(param("megapixels", "Resolution (MP)", kind, d("megapixels", json!(1.0)), "Output area"));
        }
        if has("steps") {
            params.push(param(
                "steps",
                "Steps",
                ParamKind::Int { min: 1, max: 150, step: 1 },
                d("steps", json!(20)),
                "",
            ));
        }
        if has("cfg") {
            params.push(param(
                "cfg",
                "CFG scale",
                ParamKind::Float { min: 0.0, max: 30.0, step: 0.5 },
                d("cfg", json!(7.0)),
                "",
            ));
        }
        if has("denoise") {
            let kind = ParamKind::Float { min: 0.0, max: 1.0, step: 0.05 };
            params.push(param("denoise", "Denoise strength", kind, d("denoise", json!(0.75)), ""));
        }
        if has("fps") || has("frames") {
            params.push(param(
                "fps",
                "Frame rate",
                ParamKind::Int { min: 1, max: 60, step: 1 },
                d("fps", json!(24)),
                "",
            ));
        }
        params.extend(self.side.params.iter().cloned());
        let mut m = ModelInfo {
            description: Some(
                self.side.description.clone().unwrap_or_else(|| format!("Custom workflow {}", self.file)),
            ),
            max_outputs: if has("count") { 4 } else { 1 },
            negative_prompt: has("negative_prompt"),
            seed: has("seed"),
            end_frame: has("end_image"),
            params,
            ..ModelInfo::new(ID, format!("workflow:{}", self.file), name, &tasks)
        };
        if image_in {
            m.max_images = 1;
        }
        m
    }
}

/// Every API-format workflow in `dir`; anything unreadable is logged and skipped.
fn load_workflows(dir: &Path) -> Vec<Workflow> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.ends_with(".json") && !n.ends_with(".kimchi.json") && !n.starts_with('.'))
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|f| Workflow::load(dir, f).map_err(|e| tracing::warn!("skipping ComfyUI workflow: {e}")).ok())
        .collect()
}

// ---- placeholders ----------------------------------------------------------

/// Placeholder names used anywhere in a graph.
fn placeholders(v: &Value) -> BTreeSet<String> {
    fn walk(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::String(s) => {
                let mut rest = s.as_str();
                while let Some((_, after)) = rest.split_once("{{") {
                    let Some((name, tail)) = after.split_once("}}") else { break };
                    out.insert(name.trim().to_string());
                    rest = tail;
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(v, &mut out);
    out
}

/// Replaces `{{name}}` in every string. A string that is exactly one placeholder takes the
/// variable's JSON type (so numbers stay numbers); unknown placeholders are left alone.
fn substitute(v: &mut Value, vars: &Map<String, Value>) {
    match v {
        Value::String(s) => {
            let whole = s.trim().strip_prefix("{{").and_then(|r| r.strip_suffix("}}")).map(str::trim);
            if let Some(val) = whole.filter(|n| !n.contains("{{")).and_then(|n| vars.get(n)) {
                *v = val.clone();
                return;
            }
            let mut out = String::with_capacity(s.len());
            let mut rest = s.as_str();
            while let Some((before, after)) = rest.split_once("{{") {
                out.push_str(before);
                match after.split_once("}}") {
                    Some((name, tail)) if vars.contains_key(name.trim()) => {
                        match &vars[name.trim()] {
                            Value::String(x) => out.push_str(x),
                            other => out.push_str(&other.to_string()),
                        }
                        rest = tail;
                    }
                    _ => {
                        out.push_str("{{");
                        rest = after;
                    }
                }
            }
            out.push_str(rest);
            *s = out;
        }
        Value::Array(a) => a.iter_mut().for_each(|x| substitute(x, vars)),
        Value::Object(o) => o.values_mut().for_each(|x| substitute(x, vars)),
        _ => {}
    }
}

/// Placeholder values: built-in defaults < workflow defaults < request params < request fields.
fn build_vars(req: &GenRequest, defaults: Map<String, Value>, input_dims: Option<(u32, u32)>) -> Map<String, Value> {
    let mut vars: Map<String, Value> = json!({
        "megapixels": 1.0, "steps": 20, "cfg": 7.0, "denoise": 0.75, "fps": 24, "duration": 5.0,
        "sampler": "euler", "scheduler": "normal",
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    vars.extend(defaults);
    let params = req.params.iter().filter(|(_, v)| !v.is_null() && v.as_str() != Some(""));
    vars.extend(params.map(|(k, v)| (k.clone(), v.clone())));
    let num =
        |vars: &Map<String, Value>, k: &str| vars.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()));

    let ratio = input_dims.map(|(w, h)| util::ratio_string(w, h)).unwrap_or_else(|| req.aspect());
    let (w, h) = util::size_for_ratio(&ratio, num(&vars, "megapixels").unwrap_or(1.0), 16);
    let duration = req.duration.or_else(|| num(&vars, "duration")).unwrap_or(5.0);
    let fps = num(&vars, "fps").unwrap_or(24.0);
    if !req.params.contains_key("frames") {
        vars.insert("frames".into(), json!((duration * fps).round()));
    }
    let seed = req.seed.unwrap_or_else(|| (uuid::Uuid::new_v4().as_u128() as i64) & ((1 << 48) - 1));
    for (k, v) in [
        ("prompt", json!(req.prompt)),
        ("negative_prompt", json!(req.negative_prompt.clone().unwrap_or_default())),
        ("seed", json!(seed)),
        ("count", json!(req.count.max(1))),
        ("width", json!(w)),
        ("height", json!(h)),
        ("duration", json!(duration)),
    ] {
        vars.insert(k.into(), v);
    }
    // ComfyUI rejects 20.0 for an INT input.
    for k in INT_VARS {
        if let Some(n) = num(&vars, k) {
            vars.insert((*k).into(), json!(n.round() as i64));
        }
    }
    vars
}

// ---- running ---------------------------------------------------------------

/// Uploads an input image (named by content hash, so re-runs reuse the file) and returns the
/// name `LoadImage` expects.
async fn upload(cx: &Ctx, img: &InputImage) -> GenResult<String> {
    let mut h = DefaultHasher::new();
    img.data.hash(&mut h);
    let name = format!("kimchi_{:016x}.{}", h.finish(), util::extension_for(&img.mime));
    let part = reqwest::multipart::Part::stream_with_length(img.data.clone(), img.data.len() as u64)
        .file_name(name)
        .mime_str(&img.mime)
        .map_err(|e| GenError::Provider(format!("bad image type {}: {e}", img.mime)))?;
    let form = reqwest::multipart::Form::new().part("image", part).text("type", "input").text("overwrite", "true");
    let v: Value = util::send_json(cx, cx.http.post(cx.url("/upload/image")).multipart(form)).await?;
    let name = v["name"].as_str().ok_or_else(|| util::decode_err(cx, "upload returned no file name"))?;
    Ok(match v["subfolder"].as_str().filter(|s| !s.is_empty()) {
        Some(sub) => format!("{sub}/{name}"),
        None => name.to_string(),
    })
}

/// Queues the graph and returns its prompt id. Validation failures name the offending node.
async fn queue_prompt(cx: &Ctx, graph: &Value, client_id: &str) -> GenResult<String> {
    let prompt_id = uuid::Uuid::new_v4().to_string();
    let body = json!({ "prompt": graph, "client_id": client_id, "prompt_id": prompt_id });
    let resp = cx.http.post(cx.url("/prompt")).json(&body).send().await.map_err(util::net_err(cx))?;
    let status = resp.status();
    let text = resp.text().await.map_err(util::net_err(cx))?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let mut msg = v["error"]["message"].as_str().map_or_else(|| util::error_message(&text), str::to_string);
        if let Some(errors) = v["node_errors"].as_object() {
            for n in errors.values() {
                let class = n["class_type"].as_str().unwrap_or("node");
                for e in n["errors"].as_array().into_iter().flatten() {
                    let detail =
                        e["details"].as_str().filter(|d| !d.is_empty()).or(e["message"].as_str()).unwrap_or("");
                    msg.push_str(&format!("\n{class}: {}", util::truncate(detail, 300)));
                }
            }
        }
        return Err(GenError::Http { provider: cx.provider.clone(), status: status.as_u16(), message: msg });
    }
    // Builds that predate client-chosen ids answer with their own.
    Ok(v["prompt_id"].as_str().unwrap_or(&prompt_id).to_string())
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_ws(cx: &Ctx, client_id: &str) -> Option<Socket> {
    let url = format!("{}/ws?clientId={client_id}", cx.base_url.replacen("http", "ws", 1));
    match tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(url)).await {
        Ok(Ok((ws, _))) => Some(ws),
        other => {
            tracing::debug!("ComfyUI websocket unavailable, polling instead: {other:?}");
            None
        }
    }
}

/// Follows the run over the websocket. `Ok(true)` when it finished, `Ok(false)` when the socket
/// dropped first (the caller then polls).
async fn watch<S>(cx: &Ctx, mut ws: S, id: &str, graph: &Value) -> GenResult<bool>
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut running = false;
    let mut fraction = None;
    while let Some(msg) = ws.next().await {
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(_) => continue, // binary previews, pings
            Err(_) => return Ok(false),
        };
        let Ok(m) = serde_json::from_str::<Value>(text.as_str()) else { continue };
        let data = &m["data"];
        let ours = data["prompt_id"].as_str() == Some(id);
        let class = |node: &Value| {
            node.as_str().map(|n| graph[n]["class_type"].as_str().unwrap_or(n).to_string()).unwrap_or_default()
        };
        match m["type"].as_str().unwrap_or("") {
            "status" if !running => {
                let left = data["status"]["exec_info"]["queue_remaining"].as_u64().unwrap_or(0);
                if left > 1 {
                    cx.report(Progress::message(format!("In queue ({} ahead)", left - 1)));
                }
            }
            "execution_start" if ours => {
                running = true;
                cx.report(Progress::fraction(0.0, "Starting"));
            }
            "executing" if ours => {
                if data["node"].is_null() {
                    return Ok(true);
                }
                running = true;
                cx.report(Progress { fraction, message: Some(format!("Running {}", class(&data["node"]))) });
            }
            "progress" if ours => {
                let (value, max) = (data["value"].as_f64().unwrap_or(0.0), data["max"].as_f64().unwrap_or(0.0));
                if max > 0.0 {
                    let f = (value / max).clamp(0.0, 1.0) * 0.95;
                    fraction = Some(f);
                    let what = class(&data["node"]);
                    let what = if what.contains("Sampler") || what.is_empty() { "Sampling".to_string() } else { what };
                    cx.report(Progress::fraction(f, format!("{what} {value}/{max}")));
                }
            }
            "execution_success" if ours => return Ok(true),
            "execution_error" if ours => return Err(run_error(data)),
            "execution_interrupted" if ours => {
                return Err(GenError::Provider("The run was interrupted in ComfyUI.".into()));
            }
            _ => {}
        }
    }
    Ok(false)
}

fn run_error(data: &Value) -> GenError {
    let node = data["node_type"].as_str().unwrap_or("A node");
    let msg = data["exception_message"].as_str().unwrap_or("unknown error").trim();
    GenError::Provider(format!("{node} failed: {}", util::truncate(msg, 500)))
}

/// Waits for the prompt's history entry. With `estimate_from`, also reports estimated progress.
async fn history(
    cx: &Ctx,
    id: &str,
    every: Duration,
    timeout: Duration,
    estimate_from: Option<Instant>,
) -> GenResult<Value> {
    util::poll(every, timeout, || async {
        let h: Value = util::send_json(cx, cx.http.get(cx.url(&format!("/history/{id}")))).await?;
        let Some(entry) = h.get(id) else {
            if let Some(t) = estimate_from {
                util::estimate(cx, t, Duration::from_secs(40), "Generating");
            }
            return Ok(None);
        };
        if entry["status"]["status_str"].as_str() == Some("error") {
            let messages = entry["status"]["messages"].as_array().cloned().unwrap_or_default();
            let failure = messages.iter().find(|m| m[0] == "execution_error" || m[0] == "execution_interrupted");
            return Err(match failure {
                Some(m) if m[0] == "execution_error" => run_error(&m[1]),
                _ => GenError::Provider("The run was interrupted in ComfyUI.".into()),
            });
        }
        Ok(Some(entry.clone()))
    })
    .await
}

/// Files saved by the run, as `/view` URLs, in node order. Previews (`temp`) only count when
/// nothing was saved; for video tasks, video files win over stills.
fn collect_outputs(cx: &Ctx, entry: &Value, want_video: bool) -> Vec<OutputItem> {
    let mut nodes: Vec<(&String, &Value)> = entry["outputs"].as_object().into_iter().flatten().collect();
    nodes.sort_by_key(|(id, _)| (id.parse::<u64>().unwrap_or(u64::MAX), id.to_string()));
    let files: Vec<&Value> = nodes
        .iter()
        .flat_map(|(_, out)| ["images", "gifs", "videos"].map(|k| &out[k]))
        .filter_map(Value::as_array)
        .flatten()
        .filter(|f| f["filename"].is_string())
        .collect();
    let ext = |f: &Value| {
        f["filename"].as_str().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
    };
    let moving = |f: &Value| matches!(ext(f).as_str(), "mp4" | "webm" | "mov" | "mkv" | "avi" | "gif");
    let saved: Vec<&Value> = files.iter().copied().filter(|f| f["type"] == "output").collect();
    let mut chosen = if saved.is_empty() { files } else { saved };
    // Video graphs often save a still or a last frame too: keep just the clips when there are any.
    if want_video && chosen.iter().any(|f| moving(f)) {
        chosen.retain(|f| moving(f));
    }
    chosen
        .into_iter()
        .map(|f| {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("filename", f["filename"].as_str().unwrap_or(""))
                .append_pair("subfolder", f["subfolder"].as_str().unwrap_or(""))
                .append_pair("type", f["type"].as_str().unwrap_or("output"))
                .finish();
            let kind = if want_video || moving(f) { OutputKind::Video } else { OutputKind::Image };
            OutputItem::url(kind, cx.url(&format!("/view?{query}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_parse() {
        for t in [TEXT_TO_IMAGE, IMAGE_TO_IMAGE] {
            let v: Value = serde_json::from_str(t).unwrap();
            assert!(placeholders(&v).contains("checkpoint"));
        }
    }

    #[test]
    fn substitution() {
        let mut v = json!({"a": "{{seed}}", "b": "a {{prompt}} at {{ width }}px {{unknown}}", "c": ["{{cfg}}", 1]});
        let vars = json!({"seed": 42, "prompt": "cat", "width": 512, "cfg": 6.5});
        substitute(&mut v, vars.as_object().unwrap());
        assert_eq!(v, json!({"a": 42, "b": "a cat at 512px {{unknown}}", "c": [6.5, 1]}));
    }

    #[tokio::test]
    async fn websocket_progress() {
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(vec![]));
        let sink = seen.clone();
        let cx = Ctx::new(ID, reqwest::Client::new(), "http://x").with_progress(move |p| sink.lock().push(p));
        let graph = json!({"5": {"class_type": "KSampler", "inputs": {}}});
        let msg = |v: Value| Ok(Message::Text(v.to_string().into()));
        let script = vec![
            msg(json!({"type": "status", "data": {"status": {"exec_info": {"queue_remaining": 3}}}})),
            Ok(Message::Binary(vec![1, 2, 3].into())),
            msg(json!({"type": "execution_start", "data": {"prompt_id": "p"}})),
            msg(json!({"type": "progress", "data": {"prompt_id": "other", "value": 9, "max": 10}})),
            msg(json!({"type": "progress", "data": {"prompt_id": "p", "node": "5", "value": 5, "max": 10}})),
            msg(json!({"type": "executing", "data": {"prompt_id": "p", "node": null}})),
        ];
        assert!(watch(&cx, futures::stream::iter(script), "p", &graph).await.unwrap());
        {
            let seen = seen.lock();
            assert_eq!(seen[0].message.as_deref(), Some("In queue (2 ahead)"));
            assert_eq!(seen.last().unwrap().message.as_deref(), Some("Sampling 5/10"));
            assert!((seen.last().unwrap().fraction.unwrap() - 0.475).abs() < 1e-9);
        }

        let failing = vec![msg(json!({"type": "execution_error", "data": {
            "prompt_id": "p", "node_type": "KSampler", "exception_message": "CUDA out of memory"}}))];
        let err = watch(&cx, futures::stream::iter(failing), "p", &graph).await.unwrap_err();
        assert!(err.to_string().contains("KSampler failed: CUDA out of memory"), "{err}");
        // A closed socket hands over to polling.
        assert!(!watch(&cx, futures::stream::iter(vec![]), "p", &graph).await.unwrap());
    }

    #[test]
    fn combos() {
        assert_eq!(combo(&json!([["a", "b"], {}])), vec!["a", "b"]);
        assert_eq!(combo(&json!(["COMBO", {"options": ["x"]}])), vec!["x"]);
    }
}
