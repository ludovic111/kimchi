//! Other editors' projects: what kimchi opens and writes (`project.formats`), opening one
//! (`project.importFrom`), writing the open project for one (`project.exportTo`), and finding
//! media that moved (`media.relink`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kimchi_core::{Asset, ClipContent, Id, MediaKind, Project};
use kimchi_interop::timeline::{self, ExportOptions, Support};
use kimchi_interop::{Report, apps, looks};
use serde_json::{Value, json};

use crate::commands::media::{absolute, path_str, spawn_previews};
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Location, Session, err};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "project.formats" => formats(a.opt_str("app")),
        "project.importFrom" => import_from(s, cx, &a).await,
        "project.exportTo" => export_to(s, &a).await,
        "media.relink" => relink(s, cx, &a).await,
        _ => Err(super::unhandled(cx)),
    }
}

fn formats(app: Option<&str>) -> CmdResult {
    let Some(id) = app else {
        return Ok(json!({ "formats": timeline::FORMATS, "looks": looks::LOOK_FORMATS, "apps": apps::APPS }));
    };
    let Some(app) = apps::app(id) else {
        let ids: Vec<&str> = apps::APPS.iter().map(|a| a.id).collect();
        let hint = kimchi_core::closest(id, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        return Err(format!("Unknown app `{id}`.{hint} Apps: {}.", ids.join(", ")));
    };
    let fmts: Vec<_> = timeline::FORMATS.iter().filter(|f| app.opens.contains(&f.id) || app.writes.contains(&f.id)).collect();
    let lks: Vec<_> = looks::LOOK_FORMATS.iter().filter(|l| l.apps.contains(&app.id)).collect();
    Ok(json!({ "formats": fmts, "looks": lks, "apps": [app] }))
}

/// Every file name under `dir` (a few levels deep), by lower-cased name.
fn index_folder(dir: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 200_000 {
                return out;
            }
            let p = e.path();
            if p.is_dir() {
                if depth < 8 {
                    stack.push((p, depth + 1));
                }
            } else if let Some(n) = p.file_name() {
                out.entry(n.to_string_lossy().to_lowercase()).or_insert(p);
            }
        }
    }
    out
}

fn name_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_lowercase()
}

/// Points missing media at files of the same name in `dir`; how many were found.
fn find_in(project: &mut Project, dir: &Path) -> Vec<Id> {
    let index = index_folder(dir);
    let mut found = vec![];
    for a in &mut project.assets {
        if Path::new(&a.path).exists() {
            continue;
        }
        if let Some(p) = index.get(&name_of(&a.path)) {
            a.path = path_str(p);
            a.thumbnail = None;
            a.filmstrip = None;
            a.waveform = None;
            a.proxy = None;
            found.push(a.id);
        }
    }
    found
}

/// Reads what the files really are, keeping the timing the project gave the clips.
async fn probe_all(s: &Session, assets: &mut [Asset]) {
    let Ok(tools) = s.tools() else { return };
    for a in assets.iter_mut().filter(|a| Path::new(&a.path).is_file()) {
        if let Ok(p) = kimchi_media::probe(&tools, Path::new(&a.path)).await {
            let sound_only = a.kind == MediaKind::Audio;
            a.meta = p.meta;
            if sound_only {
                a.meta.has_video = false;
            } else {
                a.kind = p.kind;
            }
        }
    }
}

fn report_json(r: &Report) -> Value {
    let mut v = json!(r);
    v["exact"] = json!(r.is_exact());
    v
}

async fn import_from(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let path = absolute(a.str("path")?)?;
    let format = a.opt_str("format").map(str::to_string);
    let p2 = path.clone();
    let imported = tokio::task::spawn_blocking(move || timeline::import(&p2, format.as_deref())).await.map_err(err)??;
    let mut project = imported.project;
    let mut report = imported.report;
    if let Some(dir) = a.opt_str("mediaFolder") {
        let dir = absolute(dir)?;
        let found = find_in(&mut project, &dir);
        report.missing_media.retain(|m| !project.assets.iter().any(|a| found.contains(&a.id) && name_of(&a.path) == name_of(m)));
        if !found.is_empty() {
            report.kept(format!("{} missing file{} found in {}", found.len(), if found.len() == 1 { "" } else { "s" }, dir.display()));
        }
    }
    probe_all(s, &mut project.assets).await;
    let into = a.opt_str("into").unwrap_or("new");
    match into {
        "new" => {
            if let Some(n) = a.opt_str("name").map(str::trim).filter(|n| !n.is_empty()) {
                project.name = n.to_string();
            }
            s.library.save(&project).map_err(err)?;
            let id = project.id;
            let assets = project.assets.clone();
            let (name, clips) = (project.name.clone(), project.clips().count());
            s.open_doc(project, Location::Library);
            for asset in assets {
                spawn_previews(s, id, asset);
            }
            Ok(json!({ "projectId": id, "name": name, "clips": clips, "report": report_json(&report) }))
        }
        "open" => {
            let project_id = s.current_id().ok_or(crate::session::NO_PROJECT)?;
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead).max(0.0);
            let assets = project.assets.clone();
            let clips = project.clips().count();
            s.edit(cx.label(), cx.source, |ed| {
                for asset in &assets {
                    ed.apply(&kimchi_core::Edit::AddAsset { asset: asset.clone() }, None).map_err(err)?;
                }
                let mut next = ed.project().clone();
                // Its video tracks above the open project's, its sound below, shifted to `start`.
                let mut video = vec![];
                let mut audio = vec![];
                for mut t in project.tracks.clone() {
                    if t.clips.is_empty() {
                        continue;
                    }
                    for c in &mut t.clips {
                        c.start += start;
                    }
                    if t.kind == kimchi_core::TrackKind::Video { video.push(t) } else { audio.push(t) }
                }
                next.tracks.splice(0..0, video);
                next.tracks.extend(audio);
                for mut m in project.markers.clone() {
                    m.time += start;
                    next.markers.push(m);
                }
                next.markers.sort_by(|x, y| x.time.total_cmp(&y.time));
                ed.replace(next).map_err(err)
            })?;
            for asset in assets {
                spawn_previews(s, project_id, asset);
            }
            Ok(json!({ "projectId": project_id, "clips": clips, "report": report_json(&report) }))
        }
        other => Err(format!("`into` is new or open, not `{other}`.")),
    }
}

/// The format to write: `format`, else the app's best, else from the file name.
fn pick_format(a: &Args, path: &Path) -> CmdResult<&'static timeline::Format> {
    if let Some(f) = a.opt_str("format") {
        return timeline::format(f);
    }
    if let Some(id) = a.opt_str("app") {
        let app = apps::app(id).ok_or_else(|| {
            let ids: Vec<&str> = apps::APPS.iter().filter(|a| !a.writes.is_empty()).map(|a| a.id).collect();
            let hint = kimchi_core::closest(id, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
            format!("Unknown app `{id}`.{hint} kimchi writes projects for {}.", ids.join(", "))
        })?;
        let best = app.writes.iter().filter_map(|w| timeline::format(w).ok()).find(|f| f.export != Support::No);
        return best.ok_or_else(|| format!("{} doesn't open any project file kimchi writes: export the video and import it there.", app.name));
    }
    timeline::format_for_path(path).filter(|f| f.export != Support::No).ok_or_else(|| "Give `format` or `app`: the file name doesn't say which format to write.".to_string())
}

async fn export_to(s: &Arc<Session>, a: &Args) -> CmdResult {
    let mut path = absolute(a.str("path")?)?;
    let f = pick_format(a, &path)?;
    if path.extension().is_none() {
        path.set_extension(if f.id == "xmeml" { "xml" } else { f.extensions[0] });
    }
    let mut project = s.project()?;
    let mut opts = ExportOptions::default();
    let mut notes: Vec<String> = vec![];
    let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let beside = |what: &str| path.with_file_name(format!("{stem} {what}"));
    if a.bool_or("renderMotion", true) {
        let drawn: Vec<kimchi_core::Clip> = project.clips().filter(|(_, c)| matches!(c.content, ClipContent::Text { .. } | ClipContent::Solid { .. } | ClipContent::Motion { .. })).map(|(_, c)| c.clone()).collect();
        if !drawn.is_empty() {
            let dir = beside("renders");
            std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
            let tools = s.tools()?;
            for c in drawn {
                let file = dir.join(format!("{}-{}.mov", sanitize(&c.name), &c.id.to_string()[..8]));
                let mut alone = project.clone();
                alone.tracks = vec![kimchi_core::Track { clips: vec![c.clone()], ..kimchi_core::Track::new(kimchi_core::TrackKind::Video, "clip") }];
                let settings = kimchi_media::export::ExportSettings {
                    path: path_str(&file),
                    format: kimchi_media::export::ExportFormat::Prores,
                    quality: kimchi_media::export::Quality::High,
                    width: None,
                    height: None,
                    fps: None,
                    range: Some((c.start, c.end())),
                    encoder: Default::default(),
                    audio: Default::default(),
                };
                match kimchi_media::export::export(&tools, &alone, &settings, |_| {}, Default::default()).await {
                    Ok(_) => {
                        opts.rendered.insert(c.id, file);
                    }
                    Err(e) => notes.push(format!("\u{201c}{}\u{201d} couldn't be rendered ({e})", c.name)),
                }
            }
            if !opts.rendered.is_empty() {
                notes.push(format!("{} title, colour or motion clip{} rendered to ProRes files in {} (over black: no transparency yet)", opts.rendered.len(), if opts.rendered.len() == 1 { "" } else { "s" }, dir.display()));
            }
        }
    }
    let mut collected = 0;
    if a.bool_or("collect", false) {
        let dir = beside("media");
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
        let mut done: HashMap<String, String> = HashMap::new();
        for asset in &mut project.assets {
            if let Some(p) = done.get(&asset.path) {
                asset.path = p.clone();
                continue;
            }
            let src = PathBuf::from(&asset.path);
            if !src.is_file() {
                continue;
            }
            let to = dir.join(src.file_name().unwrap_or_default());
            if !to.exists() {
                std::fs::copy(&src, &to).map_err(|e| format!("Couldn't copy {}: {e}", src.display()))?;
            }
            collected += 1;
            done.insert(asset.path.clone(), path_str(&to));
            asset.path = path_str(&to);
        }
    }
    let (fid, p2) = (f.id, path.clone());
    let mut report = tokio::task::spawn_blocking(move || timeline::export_with(&project, fid, &p2, &opts)).await.map_err(err)??;
    for n in notes {
        report.approximated(n);
    }
    if collected > 0 {
        report.kept(format!("{collected} media file{} copied beside it", if collected == 1 { "" } else { "s" }));
    }
    Ok(json!({ "path": path, "format": f.id, "report": report_json(&report) }))
}

fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).take(40).collect();
    if s.trim().is_empty() { "clip".into() } else { s.trim().to_string() }
}

async fn relink(s: &Arc<Session>, cx: &Ctx, a: &Args) -> CmdResult {
    let mut project = s.project()?;
    let changed: Vec<Id> = match (a.opt_str("assetId"), a.opt_str("path"), a.opt_str("folder")) {
        (Some(key), Some(path), _) => {
            let id = resolve::asset(&project, key)?;
            let path = absolute(path)?;
            if !path.is_file() {
                return Err(format!("{} isn't a file.", path.display()));
            }
            let asset = project.asset_mut(id).ok_or("media not found")?;
            asset.path = path_str(&path);
            asset.thumbnail = None;
            asset.filmstrip = None;
            asset.waveform = None;
            asset.proxy = None;
            vec![id]
        }
        (_, _, Some(folder)) => {
            let dir = absolute(folder)?;
            if !dir.is_dir() {
                return Err(format!("{} isn't a folder.", dir.display()));
            }
            find_in(&mut project, &dir)
        }
        _ => return Err("Give assetId and path (one file), or folder (every missing file, found by name).".into()),
    };
    let missing: Vec<String> = project.assets.iter().filter(|a| !Path::new(&a.path).exists()).map(|a| a.name.clone()).collect();
    if changed.is_empty() {
        return Ok(json!({ "relinked": 0, "missing": missing }));
    }
    let mut assets: Vec<Asset> = project.assets.iter().filter(|a| changed.contains(&a.id)).cloned().collect();
    probe_all(s, &mut assets).await;
    for a in &assets {
        if let Some(slot) = project.asset_mut(a.id) {
            *slot = a.clone();
        }
    }
    let project_id = project.id;
    s.edit(cx.label(), cx.source, |ed| ed.replace(project).map_err(err))?;
    for a in assets {
        spawn_previews(s, project_id, a);
    }
    Ok(json!({ "relinked": changed.len(), "missing": missing }))
}
