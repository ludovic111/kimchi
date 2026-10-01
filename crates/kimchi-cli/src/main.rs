//! `kimchi` — the editor's engine without the window.
//!
//! ```text
//! kimchi providers
//! kimchi models <provider>
//! kimchi generate <provider> <model> "<prompt>" [--video] [--image PATH] [--end PATH]
//!                 [--aspect 16:9] [--duration 5] [--seed N] [--count N] [--out DIR]
//! kimchi render <project.json> <output> [--format mp4|hevc|prores|webm|gif|audio] [--quality draft|standard|high]
//! ```
//! Keys come from the usual environment variables (OPENROUTER_API_KEY, FAL_KEY…).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use kimchi_gen::{GenRequest, Harness, ImageRole, InputImage, JobStatus, MemorySecrets, Task};
use kimchi_media::export::{ExportFormat, ExportSettings, Quality};

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("providers") => providers(),
        Some("models") => models(&args[1..]).await,
        Some("generate") => generate(&args[1..]).await,
        Some("render") => render(&args[1..]).await,
        _ => {
            eprintln!("{}", USAGE);
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage:
  kimchi providers
  kimchi models <provider>
  kimchi generate <provider> <model> \"<prompt>\" [--video] [--image PATH] [--end PATH] [--aspect R] [--duration S] [--seed N] [--count N] [--out DIR]
  kimchi render <project.json> <output> [--format mp4|hevc|prores|webm|gif|audio] [--quality draft|standard|high]";

type Res = Result<(), String>;

fn harness() -> Arc<Harness> {
    Arc::new(Harness::new(Arc::new(MemorySecrets::default())))
}

/// Splits `--flag value` pairs (and bare `--flag` switches) from positionals.
fn parse(args: &[String]) -> (Vec<String>, HashMap<String, String>) {
    let mut pos = vec![];
    let mut flags = HashMap::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            let value = match it.peek() {
                Some(v) if !v.starts_with("--") => it.next().cloned().unwrap_or_default(),
                _ => "true".into(),
            };
            flags.insert(name.to_string(), value);
        } else {
            pos.push(a.clone());
        }
    }
    (pos, flags)
}

fn providers() -> Res {
    for s in harness().statuses() {
        let state = if s.ready { "ready" } else if s.info.needs_key { "no key" } else { "local" };
        println!("{:<14} {:<8} {:<8} {}", s.info.id, format!("{:?}", s.info.kind).to_lowercase(), state, s.info.tagline);
    }
    Ok(())
}

async fn models(args: &[String]) -> Res {
    let provider = args.first().ok_or("which provider?")?;
    for m in harness().models(provider, true).await.map_err(|e| e.to_string())? {
        let tasks: Vec<&str> = m.tasks.iter().map(|t| t.as_str()).collect();
        println!("{}{:<48} {:<28} {}", if m.featured { "★ " } else { "  " }, m.id, m.name, tasks.join(", "));
    }
    Ok(())
}

async fn generate(args: &[String]) -> Res {
    let (pos, flags) = parse(args);
    let [provider, model, prompt] = &pos[..] else { return Err("expected <provider> <model> \"<prompt>\"".into()) };
    let video = flags.contains_key("video");
    let image = flags.get("image");
    let task = match (video, image.is_some()) {
        (false, false) => Task::TextToImage,
        (false, true) => Task::ImageToImage,
        (true, false) => Task::TextToVideo,
        (true, true) => Task::ImageToVideo,
    };
    let mut req = GenRequest::new(model, task, prompt);
    let role = if video { ImageRole::StartFrame } else { ImageRole::Reference };
    for (path, role) in [(image, role), (flags.get("end"), ImageRole::EndFrame)] {
        if let Some(p) = path {
            req.images.push(InputImage { role, path: p.clone(), mime: String::new(), data: Default::default() });
        }
    }
    req.aspect_ratio = flags.get("aspect").cloned();
    req.duration = flags.get("duration").and_then(|d| d.parse().ok());
    req.seed = flags.get("seed").and_then(|s| s.parse().ok());
    req.count = flags.get("count").and_then(|c| c.parse().ok()).unwrap_or(1);
    let out = PathBuf::from(flags.get("out").map(String::as_str).unwrap_or("."));

    let h = harness();
    let mut events = h.subscribe();
    let job = h.submit(provider, req, out, serde_json::Value::Null).map_err(|e| e.to_string())?;
    loop {
        let j = events.recv().await.map_err(|e| e.to_string())?;
        if j.id != job.id {
            continue;
        }
        let pct = j.progress.fraction.map(|f| format!("{:>3.0}% ", f * 100.0)).unwrap_or_default();
        eprintln!("{pct}{}", j.progress.message.unwrap_or_default());
        match j.status {
            JobStatus::Succeeded => {
                for o in j.outputs {
                    println!("{}", o.path);
                }
                return Ok(());
            }
            JobStatus::Failed => return Err(j.error.unwrap_or_else(|| "generation failed".into())),
            JobStatus::Cancelled => return Err("cancelled".into()),
            _ => {}
        }
    }
}

async fn render(args: &[String]) -> Res {
    let (pos, flags) = parse(args);
    let [project, output] = &pos[..] else { return Err("expected <project.json> <output>".into()) };
    let project: kimchi_core::Project = serde_json::from_slice(&std::fs::read(project).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let format = match flags.get("format").map(String::as_str).unwrap_or("mp4") {
        "mp4" => ExportFormat::Mp4,
        "hevc" => ExportFormat::Hevc,
        "prores" => ExportFormat::Prores,
        "webm" => ExportFormat::Webm,
        "gif" => ExportFormat::Gif,
        "audio" => ExportFormat::Audio,
        f => return Err(format!("unknown format {f}")),
    };
    let quality = match flags.get("quality").map(String::as_str).unwrap_or("standard") {
        "draft" => Quality::Draft,
        "high" => Quality::High,
        _ => Quality::Standard,
    };
    if project.clips().any(|(_, c)| matches!(c.content, kimchi_core::ClipContent::Text { .. })) {
        eprintln!("note: text clips are rasterised by the desktop app and are skipped here");
    }
    let tools = kimchi_media::Tools::locate().map_err(|e| e.to_string())?;
    let settings = ExportSettings { path: output.clone(), format, quality, width: None, height: None, fps: None, range: None };
    kimchi_media::export::export(&tools, &project, &HashMap::new(), &settings, |p| eprint!("\r{:>3.0}%", p * 100.0), Default::default())
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("\r100%");
    println!("{output}");
    Ok(())
}
