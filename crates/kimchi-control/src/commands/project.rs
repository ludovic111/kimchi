use std::path::PathBuf;
use std::sync::Arc;

use kimchi_core::{ClipContent, Edit, Fit, MediaKind, Project, ProjectSettings, Transform};
use serde_json::{Value, json};

use crate::registry::{self, Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Location, Session, err, read_project_file, write_project_file};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "project.list" => {
            let open = s.current_id();
            Ok(json!(s.library.list().into_iter().map(|p| {
                let mut v = json!(p);
                v["open"] = json!(Some(p.id) == open);
                v
            }).collect::<Vec<_>>()))
        }
        "project.overview" => overview(s),
        "project.get" => Ok(json!(s.project()?)),
        "project.renderFrame" => {
            let p = s.project()?;
            let times: Vec<f64> = match a.array("times") {
                Some(list) => list.iter().map(|t| t.as_f64().ok_or("times are numbers of seconds")).collect::<Result<_, _>>()?,
                None => vec![a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead)],
            };
            if times.is_empty() || times.len() > 16 {
                return Err("Give between 1 and 16 times.".into());
            }
            let path = crate::commands::motion::render_png(s, &p, &times, a.opt_u32("width")).await?;
            Ok(json!({ "path": path, "times": times, "duration": round(p.duration()) }))
        }
        "project.create" => {
            let name = a.opt_str("name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Untitled");
            let d = ProjectSettings::default();
            let settings = ProjectSettings {
                width: a.opt_u32("width").unwrap_or(d.width),
                height: a.opt_u32("height").unwrap_or(d.height),
                fps: a.opt_f64("fps").unwrap_or(d.fps),
                background: a.opt_str("background").map(str::to_string).unwrap_or(d.background),
                sample_rate: d.sample_rate,
            };
            if settings.width < 16 || settings.height < 16 || settings.fps <= 0.0 {
                return Err("width and height must be at least 16 and fps above 0".into());
            }
            let project = Project::new(name, settings);
            s.library.save(&project).map_err(err)?;
            let id = project.id;
            s.open_doc(project, Location::Library);
            Ok(json!({ "projectId": id, "name": name }))
        }
        "project.open" => {
            if let Some(path) = a.opt_str("path") {
                let path = PathBuf::from(path);
                let project = read_project_file(&path)?;
                let v = json!({ "projectId": project.id, "name": project.name, "path": path });
                s.open_doc(project, Location::File(path));
                return Ok(v);
            }
            let key = a.opt_str("projectId").ok_or("Give projectId (from project.list) or path (a project file).")?;
            let id = resolve::project(&s.library.list(), key)?;
            let project = s.library.load(id).map_err(|e| format!("Couldn't open the project: {e}"))?;
            let v = json!({ "projectId": id, "name": project.name });
            s.open_doc(project, Location::Library);
            Ok(v)
        }
        "project.close" => {
            s.close_doc();
            Ok(json!({ "closed": true }))
        }
        "project.delete" => {
            let id = resolve::project(&s.library.list(), a.str("projectId")?)?;
            if s.current_id() == Some(id) {
                s.close_doc();
            }
            s.library.delete(id).map_err(err)?;
            Ok(json!({ "deleted": id }))
        }
        "project.duplicate" => {
            let id = match a.opt_str("projectId") {
                Some(k) => resolve::project(&s.library.list(), k)?,
                None => s.current_id().ok_or(crate::session::NO_PROJECT)?,
            };
            let mut p = if s.current_id() == Some(id) { s.project()? } else { s.library.load(id).map_err(err)? };
            p.id = kimchi_core::new_id();
            p.name = format!("{} copy", p.name);
            p.created_at = chrono::Utc::now();
            p.updated_at = p.created_at;
            s.library.save(&p).map_err(err)?;
            Ok(json!(kimchi_core::store::summarize(&p)))
        }
        "project.rename" => {
            let edit = Edit::RenameProject { name: a.str("name")?.to_string() };
            let id = match a.opt_str("projectId") {
                Some(k) => resolve::project(&s.library.list(), k)?,
                None => s.current_id().ok_or(crate::session::NO_PROJECT)?,
            };
            if s.current_id() == Some(id) {
                s.apply(cx.label(), cx.source, &edit, a.coalesce())?;
            } else {
                s.with_project(id, |ed| ed.apply(&edit, None))?.map_err(err)?;
            }
            let name = s.library.load(id).map(|p| p.name).map_err(err)?;
            Ok(json!({ "projectId": id, "name": name }))
        }
        "project.setSettings" => {
            let mut settings = s.read(|ed| ed.project().settings.clone())?;
            if let Some(v) = a.opt_u32("width") {
                settings.width = v;
            }
            if let Some(v) = a.opt_u32("height") {
                settings.height = v;
            }
            if let Some(v) = a.opt_f64("fps") {
                settings.fps = v;
            }
            if let Some(v) = a.opt_str("background") {
                settings.background = crate::commands::clip::color(v)?;
            }
            if let Some(v) = a.opt_u32("sampleRate") {
                settings.sample_rate = v;
            }
            s.apply(cx.label(), cx.source, &Edit::SetSettings { settings: settings.clone() }, None)?;
            Ok(json!(settings))
        }
        "project.saveAs" => {
            let path = PathBuf::from(a.str("path")?);
            write_project_file(&path, &s.project()?)?;
            Ok(json!({ "path": path }))
        }
        "project.batch" => batch(s, cx, a).await,
        _ => Err(crate::commands::unhandled(cx)),
    }
}

async fn batch(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    let commands = a.array("commands").cloned().unwrap_or_default();
    let atomic = a.bool_or("atomic", true);
    let label = a.opt_str("label").unwrap_or("batch").to_string();
    // Check everything before running anything.
    let mut calls = Vec::with_capacity(commands.len());
    for (i, c) in commands.iter().enumerate() {
        let name = c.get("command").and_then(Value::as_str).ok_or_else(|| format!("commands[{i}] needs \"command\""))?;
        let spec = registry::spec(name).ok_or_else(|| format!("commands[{i}]: unknown command `{name}`"))?;
        if spec.name == "project.batch" {
            return Err("project.batch can't contain another batch".into());
        }
        if matches!(spec.family(), "project") && spec.perm == crate::registry::Perm::Projects {
            return Err(format!("commands[{i}]: `{name}` switches projects and can't be part of a batch"));
        }
        let params = c.get("params").cloned().unwrap_or(json!({}));
        registry::allowed(s, cx.source, spec)?;
        registry::validate(spec, &params).map_err(|e| format!("commands[{i}]: {e}"))?;
        calls.push((spec, params));
    }
    let open = s.is_open();
    if open {
        s.edit(cx.label(), cx.source, |ed| {
            ed.begin_batch(label.clone(), cx.source.as_str());
            Ok(())
        })?;
    }
    let mut results = Vec::with_capacity(calls.len());
    let mut failure = None;
    for (i, (spec, params)) in calls.into_iter().enumerate() {
        match registry::call_boxed(s, cx.source, spec, params).await {
            Ok(v) => results.push(json!({ "command": spec.name, "ok": true, "result": v })),
            Err(e) => {
                results.push(json!({ "command": spec.name, "ok": false, "error": e }));
                failure = Some(format!("commands[{i}] ({}) failed: {e}", spec.name));
                if atomic {
                    break;
                }
            }
        }
    }
    if open {
        s.edit(cx.label(), cx.source, |ed| {
            if failure.is_some() && atomic {
                ed.rollback_batch();
            } else {
                ed.end_batch();
            }
            Ok(())
        })?;
    }
    match failure {
        Some(e) if atomic => Err(format!("{e}. Nothing was changed.")),
        _ => Ok(json!({ "results": results })),
    }
}

/// `project.overview`: the whole project, bounded, plus what an agent should notice.
fn overview(s: &Arc<Session>) -> CmdResult {
    let (p, history) = s.read(|ed| (ed.project().clone(), json!({
        "canUndo": ed.can_undo(),
        "canRedo": ed.can_redo(),
        "undo": ed.undo_steps().iter().rev().take(10).collect::<Vec<_>>(),
        "redo": ed.redo_steps().iter().take(5).collect::<Vec<_>>(),
    })))?;
    let location = match s.location() {
        Some(Location::File(path)) => json!({ "file": path }),
        _ => json!({ "library": s.library.project_dir(p.id) }),
    };
    let mut problems: Vec<String> = vec![];
    for a in &p.assets {
        if !std::path::Path::new(&a.path).exists() {
            problems.push(format!("Media \"{}\" is missing on disk ({}).", a.name, a.path));
        }
    }
    let tracks: Vec<Value> = p
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if t.hidden && !t.clips.is_empty() {
                problems.push(format!("Track \"{}\" is hidden: its pictures don't show.", t.name));
            }
            if t.muted && !t.clips.is_empty() {
                problems.push(format!("Track \"{}\" is muted.", t.name));
            }
            json!({
                "id": t.id,
                "index": i,
                "name": t.name,
                "kind": t.kind,
                "muted": t.muted,
                "hidden": t.hidden,
                "locked": t.locked,
                "clips": t.clips.iter().map(|c| {
                    if let ClipContent::Pending { prompt, .. } = &c.content {
                        problems.push(format!("Clip \"{}\" is still generating (\"{prompt}\").", c.name));
                    }
                    if let Some(lut) = c.effects.lut.as_ref().filter(|l| !std::path::Path::new(&l.path).is_file()) {
                        problems.push(format!("Clip \"{}\" uses a LUT that is missing ({}): it is drawn without it.", c.name, lut.path));
                    }
                    clip_summary(&p, c)
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    let assets: Vec<Value> = p.assets.iter().map(asset_summary).collect();
    let jobs: Vec<Value> = s
        .harness
        .jobs()
        .into_iter()
        .filter(|j| !j.status.is_done())
        .map(|j| json!({ "id": j.id, "status": j.status, "model": j.model_name, "prompt": j.request.prompt, "progress": j.progress }))
        .collect();
    Ok(json!({
        "project": {
            "id": p.id,
            "name": p.name,
            "settings": p.settings,
            "aspectRatio": p.settings.aspect_ratio(),
            "duration": round(p.duration()),
            "location": location,
        },
        "tracks": tracks,
        "media": assets,
        "markers": p.markers,
        "jobs": jobs,
        "history": history,
        "window": if s.has_ui() { json!(s.ui_state()) } else { Value::Null },
        "problems": problems,
    }))
}

pub fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub fn clip_summary(p: &Project, c: &kimchi_core::Clip) -> Value {
    let mut v = json!({
        "id": c.id,
        "name": c.name,
        "start": round(c.start),
        "end": round(c.end()),
        "duration": round(c.duration),
    });
    match &c.content {
        ClipContent::Media { asset_id } => {
            v["type"] = json!("media");
            v["assetId"] = json!(asset_id);
            if let Some(a) = p.asset(*asset_id) {
                v["media"] = json!(a.name);
                v["kind"] = json!(a.kind);
                if a.kind != MediaKind::Image {
                    v["inPoint"] = json!(round(c.in_point));
                }
            }
        }
        ClipContent::Text { style } => {
            v["type"] = json!("text");
            v["text"] = json!(style.content);
        }
        ClipContent::Solid { color } => {
            v["type"] = json!("solid");
            v["color"] = json!(color);
        }
        ClipContent::Pending { job_id, prompt, model_name, .. } => {
            v["type"] = json!("generating");
            v["jobId"] = json!(job_id);
            v["prompt"] = json!(prompt);
            v["model"] = json!(model_name);
        }
        ClipContent::Motion { scene, template } => {
            v["type"] = json!("motion");
            v["scene"] = json!(if scene.is_3d() { "3d" } else { "2d" });
            v["ids"] = json!(scene.ids());
            if let Some(t) = template {
                v["template"] = json!(t.id);
            }
            if c.in_point > 0.0 {
                v["inPoint"] = json!(round(c.in_point));
            }
        }
    }
    if !c.keyframes.is_empty() {
        v["animated"] = json!(c.keyframes.keys().collect::<Vec<_>>());
    }
    if c.speed != 1.0 {
        v["speed"] = json!(c.speed);
    }
    if c.reverse {
        v["reverse"] = json!(true);
    }
    if !c.effects.is_default() {
        v["effects"] = json!(c.effects);
    }
    if let Some(tr) = &c.transition {
        v["transition"] = json!({ "kind": tr.kind, "duration": round(tr.duration) });
    }
    if c.volume != 1.0 {
        v["volume"] = json!(c.volume);
    }
    if c.fade_in > 0.0 {
        v["fadeIn"] = json!(round(c.fade_in));
    }
    if c.fade_out > 0.0 {
        v["fadeOut"] = json!(round(c.fade_out));
    }
    let t = &c.transform;
    let d = Transform::default();
    if t.x != d.x || t.y != d.y || t.scale != d.scale || t.rotation != d.rotation || t.opacity != d.opacity || t.fit != Fit::Contain {
        v["transform"] = json!(t);
    }
    v
}

pub fn asset_summary(a: &kimchi_core::Asset) -> Value {
    let mut v = json!({
        "id": a.id,
        "name": a.name,
        "kind": a.kind,
        "path": a.path,
        "duration": a.duration().map(round),
        "width": a.meta.width,
        "height": a.meta.height,
        "hasAudio": a.meta.has_audio,
        "previews": {
            "thumbnail": a.thumbnail.is_some(),
            "filmstrip": a.filmstrip.is_some(),
            "waveform": a.waveform.is_some(),
            "proxy": a.proxy.is_some(),
        },
    });
    if let kimchi_core::AssetOrigin::Generated(g) = &a.origin {
        v["generated"] = json!({ "provider": g.provider, "model": g.model_name, "task": g.task, "prompt": g.prompt, "seed": g.seed });
    }
    v
}
