//! `looks.*`: the look library (`crate::looks`): list, import from other apps' files and
//! folders, apply to clips, save a clip's grade (into the library, or as a `.cube` other apps
//! open), remove.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kimchi_core::effects::LOOKS;
use kimchi_core::{ClipPatch, Edit};
use serde_json::{Value, json};

use crate::looks::Library;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    let lib = Library::new(&s.data_dir);
    match cx.spec.name {
        "looks.list" => {
            let q = a.opt_str("query").map(str::to_lowercase).filter(|q| !q.trim().is_empty());
            let hit = |fields: &[&str]| q.as_ref().is_none_or(|q| fields.iter().any(|f| f.to_lowercase().contains(q.as_str())));
            let mut out: Vec<Value> = LOOKS
                .iter()
                .filter(|l| hit(&[l.id, l.label, l.doc, "kimchi"]))
                .map(|l| {
                    let values: serde_json::Map<String, Value> = l.values.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
                    json!({ "id": l.id, "name": l.label, "builtIn": true, "folder": null, "description": l.doc, "values": values })
                })
                .collect();
            for l in lib.list() {
                let format = kimchi_interop::looks::LOOK_FORMATS.iter().find(|f| f.id == l.format);
                let apps: Vec<&str> = format.map(|f| f.apps.iter().filter_map(|a| kimchi_interop::apps::app(a)).map(|a| a.name).collect()).unwrap_or_default();
                let mut fields = vec![l.name.as_str(), l.id.as_str(), l.folder.as_deref().unwrap_or(""), format.map_or("", |f| f.label)];
                fields.extend(apps);
                if hit(&fields) {
                    out.push(crate::looks::describe(&l));
                }
            }
            Ok(json!(out))
        }
        "looks.import" => import(s, &a).await,
        "looks.apply" => {
            let look = lib.find(a.str("look")?)?;
            let strength = a.opt_f64("strength");
            let p = s.project()?;
            let ids = resolve::clips(&p, &a.strings("clipIds"))?;
            if ids.is_empty() {
                return Err("`clipIds` is empty".into());
            }
            let mut edits = vec![];
            for id in &ids {
                let clip = p.clip(*id).ok_or("clip not found")?;
                if !has_picture(&p, *id) {
                    return Err(format!("“{}” has no picture to put a look on.", clip.name));
                }
                let effects = look.apply(&clip.effects, strength)?;
                edits.push(Edit::UpdateClip { clip_id: *id, patch: ClipPatch { effects: Some(effects), ..Default::default() } });
            }
            crate::commands::clip::apply_all(s, cx, &edits, None)?;
            Ok(json!({ "look": look.name(), "clipIds": ids }))
        }
        "looks.save" => {
            let p = s.project()?;
            let id = resolve::clip(&p, a.str("clipId")?)?;
            let clip = p.clip(id).ok_or("clip not found")?;
            let name = a.opt_str("name").map(str::to_string).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| clip.name.clone());
            let fx = &clip.effects;
            let grades = kimchi_core::effects::EFFECT_PROPS.iter().filter(|p| !matches!(**p, "vignette" | "sharpen")).any(|p| fx.get(p).unwrap_or(0.0).abs() > 1e-9);
            if !grades && fx.lut.is_none() && fx.vignette == 0.0 && fx.sharpen == 0.0 {
                return Err(format!("“{}” has no colour corrections or LUT to save as a look.", clip.name));
            }
            let mut left_out = vec![];
            if let Some(path) = a.opt_str("path") {
                // A .cube other apps open: the colours only.
                let path = crate::commands::media::absolute(path)?;
                let path = if path.extension().is_none() { path.with_extension("cube") } else { path };
                if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cube")) {
                    return Err(format!("{} should end in .cube: kimchi writes looks as Cube LUTs.", path.display()));
                }
                let text = kimchi_media::render::grade::bake_cube(fx, 33).map_err(|e| e.to_string())?;
                let title = name.replace('"', "'");
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("Can't make {}: {e}", dir.display()))?;
                }
                std::fs::write(&path, format!("TITLE \"{title}\"\n{text}")).map_err(|e| format!("Can't write {}: {e}", path.display()))?;
                for (on, what) in [(fx.vignette > 0.0, "the vignette"), (fx.sharpen > 0.0, "sharpening"), (fx.chroma_key.is_some(), "the chroma key"), (!fx.plugins.is_empty(), "plugins")] {
                    if on {
                        left_out.push(what);
                    }
                }
                if clip.keyframes.keys().any(|k| kimchi_core::effects::EFFECT_PROPS.contains(&k.as_str())) {
                    left_out.push("animation (the LUT holds the values without keyframes)");
                }
                return Ok(json!({
                    "path": crate::commands::media::path_str(&path),
                    "size": 33,
                    "notIncluded": left_out,
                    "note": if left_out.is_empty() { "A 33-point 3D .cube: Premiere (Lumetri › Creative › Look), Resolve, Final Cut (Custom LUT) and others open it.".to_string() } else { format!("A LUT holds colours only, so it leaves out {}.", left_out.join(", ")) },
                }));
            }
            let mut report = kimchi_interop::Report::new("saved");
            if fx.chroma_key.is_some() || !fx.plugins.is_empty() {
                report.dropped("The chroma key and plugins stay with the clip");
            }
            let look = lib.add(&name, a.opt_str("folder").map(str::to_string).or(Some("Saved".into())), "saved", None, fx, report)?;
            s.toast(crate::session::ToastKind::Success, format!("Saved the look “{}”", look.name));
            Ok(crate::looks::describe(&look))
        }
        "looks.remove" => {
            let look = lib.remove(a.str("look")?)?;
            Ok(json!({ "removed": look.name, "id": look.id }))
        }
        _ => Err(super::unhandled(cx)),
    }
}

/// Whether the clip draws a picture (not a sound).
fn has_picture(p: &kimchi_core::Project, id: kimchi_core::Id) -> bool {
    p.locate_clip(id).is_some_and(|(t, _)| p.tracks[t].kind == kimchi_core::TrackKind::Video)
}

/// `looks.import`: every file (folders with their subfolders), each look checked and copied in.
async fn import(s: &Arc<Session>, a: &Args) -> CmdResult {
    let paths = a.strings("paths");
    if paths.is_empty() {
        return Err("`paths` is empty: give LUT or preset files, or folders of them.".into());
    }
    let folder = a.opt_str("folder").map(str::to_string);
    // (file, the folder it shows in)
    let mut files: Vec<(PathBuf, Option<String>)> = vec![];
    let mut failed: Vec<Value> = vec![];
    for p in &paths {
        let path = crate::commands::media::absolute(p)?;
        if path.is_dir() {
            let base = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let found = kimchi_interop::looks::files_in(&path);
            if found.is_empty() {
                failed.push(json!({ "path": p, "error": "No LUTs or presets in this folder (kimchi reads .cube, .3dl, .csp, .spi1d, .spi3d, Hald CLUT pictures, .xmp, .lrtemplate and .prfpset)." }));
            }
            for f in found {
                // Subfolders show as "Folder/Sub".
                let sub = f.parent().and_then(|d| d.strip_prefix(&path).ok()).map(|r| r.to_string_lossy().replace('\\', "/")).filter(|r| !r.is_empty());
                let shown = match (&folder, &base, sub) {
                    (Some(f), _, Some(sub)) => Some(format!("{f}/{sub}")),
                    (Some(f), _, None) => Some(f.clone()),
                    (None, Some(b), Some(sub)) => Some(format!("{b}/{sub}")),
                    (None, b, None) => b.clone(),
                    (None, None, Some(sub)) => Some(sub),
                };
                files.push((f, shown));
            }
        } else if path.is_file() {
            let shown = folder.clone().or_else(|| path.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()));
            files.push((path, shown));
        } else {
            failed.push(json!({ "path": p, "error": "Nothing is at this path." }));
        }
    }
    let data_dir = s.data_dir.clone();
    let files_count = files.len();
    // Parsing and checking LUTs reads files: off the async threads.
    let results = tokio::task::spawn_blocking(move || {
        let lib = Library::new(&data_dir);
        let mut added = vec![];
        let mut failed = vec![];
        for (file, shown) in files {
            match kimchi_interop::looks::read(&file) {
                Ok(looks) => {
                    for look in looks {
                        if let Some(l) = &look.effects.lut
                            && let Err(e) = kimchi_media::render::grade::lut(Path::new(&l.path))
                        {
                            failed.push(json!({ "path": file, "error": format!("Its LUT can't be used: {e}") }));
                            continue;
                        }
                        if look.effects.is_default() {
                            failed.push(json!({ "path": file, "name": look.name, "error": "Nothing in it that kimchi can use.", "dropped": look.report.dropped }));
                            continue;
                        }
                        let format = look.report.format.clone();
                        match lib.add(&look.name, shown.clone(), &format, Some(&file), &look.effects, look.report.clone()) {
                            Ok(l) => added.push(json!({
                                "id": l.id,
                                "name": l.name,
                                "folder": l.folder,
                                "file": file,
                                "kept": look.report.kept,
                                "approximated": look.report.approximated,
                                "dropped": look.report.dropped,
                            })),
                            Err(e) => failed.push(json!({ "path": file, "error": e })),
                        }
                    }
                }
                Err(e) => failed.push(json!({ "path": file, "error": e })),
            }
        }
        (added, failed)
    })
    .await
    .map_err(|e| format!("The import stopped: {e}"))?;
    let (added, more) = results;
    failed.extend(more);
    if added.is_empty() && !failed.is_empty() && files_count <= 1 {
        let why = failed.iter().filter_map(|f| f["error"].as_str()).collect::<Vec<_>>().join(" ");
        return Err(format!("No look was added. {why}"));
    }
    if !added.is_empty() {
        let text = if added.len() == 1 { format!("Added the look “{}”", added[0]["name"].as_str().unwrap_or("")) } else { format!("Added {} looks", added.len()) };
        s.toast(crate::session::ToastKind::Success, text);
    }
    Ok(json!({ "added": added, "failed": failed }))
}
