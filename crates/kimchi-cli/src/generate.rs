//! `kimchi-cli generate`: one generation straight to files, without a project or the app.
//! Kept from the first CLI because it is handy for trying a model; inside a project use
//! `generate.submit` (and its siblings), which place the result on the timeline.
//! Keys come from the OS keychain (as saved in the app) or the usual environment variables
//! (OPENROUTER_API_KEY, FAL_KEY…).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use kimchi_gen::{GenRequest, Harness, ImageRole, InputImage, JobStatus, Task};

/// Splits `--flag value` pairs (and bare `--flag` switches) from positionals.
fn parse(args: &[String]) -> (Vec<String>, HashMap<String, String>) {
    let mut pos = vec![];
    let mut flags = HashMap::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            let value = match it.peek() {
                Some(v) if !v.starts_with("--") && name != "video" => it.next().cloned().unwrap_or_default(),
                _ => "true".into(),
            };
            flags.insert(name.to_string(), value);
        } else {
            pos.push(a.clone());
        }
    }
    (pos, flags)
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let (pos, flags) = parse(args);
    let [provider, model, prompt] = &pos[..] else { return Err("generate needs <provider> <model> \"<prompt>\"".into()) };
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

    let h = Arc::new(Harness::new(kimchi_cli::kimchi_control::secrets::default_store()));
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
