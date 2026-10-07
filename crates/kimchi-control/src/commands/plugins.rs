//! Plugins (lsuite `PLUGINS.md`): the catalogue (`plugin.list`, `plugin.info`, `plugin.rescan`),
//! the switches (`plugin.enable`, `plugin.disable`), lsuite plugin bundles (`plugin.install`,
//! `plugin.remove`), making plugins with an agent (`plugin.guide`, `plugin.toolchain`,
//! `plugin.new`, `plugin.writeSource`, `plugin.build`, `plugin.publishLocal`, see
//! [`crate::plugin_dev`]), and plugins on clips (`clip.addPlugin`, `clip.setPlugin`,
//! `clip.removePlugin`, `clip.movePlugin`; transitions take one with `transition.set plugin=…`).
//! Each change to a clip is one `UpdateClip` of its effects (and keyframes): one undo step.
//!
//! What kimchi can use:
//! - **stock**: what ships in kimchi: its colour effects, transitions and looks (parameters of a
//!   clip, `clip.setEffects` / `transition.set` / `looks.apply`), ryolune's stock sound effects,
//!   and the plugins built on the SDK that are linked in (Halftone, Chromatic aberration,
//!   Gradient, Radial wipe);
//! - **installed**: lsuite plugins (bundles in `~/.lsuite/plugins/kimchi/`), frei0r filters, and
//!   the CLAP / VST3 / Audio Unit / ryolune sound plugins ryolune's engine found (`audio.effects`,
//!   which leaves out the ones switched off). LUTs are looks (`looks.import`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kimchi_core::{Clip, ClipContent, ClipPatch, Edit, Id, KeyValue, PluginEffect, PluginValue};
use kimchi_media::render::plugins::{self as host, Format, ParamInfo, PluginInfo, PluginKind, bundle, catalogue as cat, value};
use serde_json::{Map, Value, json};

use crate::plugin_dev;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Event, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    // The person's folders may have changed in Settings since the session started.
    crate::session::configure_plugins(s);
    match cx.spec.name {
        "plugin.list" => Ok(list(s, &a).await),
        "plugin.info" => info(s, &a),
        "plugin.enable" | "plugin.disable" => {
            let on = cx.spec.name == "plugin.enable";
            let id = a.str("id")?.trim().to_string();
            if id.starts_with("stock:") {
                return Err("kimchi's own effects, transitions and looks are part of kimchi: they can't be switched off.".into());
            }
            // A video plugin, else a sound plugin (ryolune's engine's).
            let (key, name) = match cat::lookup(&id) {
                Ok(p) => (p.id.clone(), p.name.clone()),
                Err(video) => match kimchi_audio::plugins::effect(&id) {
                    Ok(e) if e.format == "ryolune" => return Err(format!("{} is one of ryolune's stock effects: it can't be switched off.", e.name)),
                    Ok(e) => (e.id.clone(), e.name.clone()),
                    Err(_) => return Err(video),
                },
            };
            s.update_settings(|st| {
                st.plugins.disabled.retain(|d| *d != key);
                if !on {
                    st.plugins.disabled.push(key.clone());
                }
            })?;
            if on {
                host::clear_failure(&key);
            }
            host::set_disabled(&s.settings().plugins.disabled);
            s.emit(Event::SettingsChanged);
            Ok(json!({ "id": key, "enabled": on, "name": name }))
        }
        "plugin.rescan" => {
            let report = tokio::task::spawn_blocking(|| cat::rescan(true, &|p| tracing::info!(path = %p.display(), "scanning video plugin"))).await.map_err(crate::session::err)??;
            s.emit(Event::SettingsChanged);
            Ok(report_json(&report))
        }
        "plugin.install" => {
            let dir = PathBuf::from(a.str("path")?.trim());
            let out = tokio::task::spawn_blocking(move || install(&dir)).await.map_err(crate::session::err)??;
            s.emit(Event::SettingsChanged);
            Ok(out)
        }
        "plugin.remove" => {
            let id = a.str("id")?.trim().to_string();
            let out = tokio::task::spawn_blocking(move || remove(&id)).await.map_err(crate::session::err)??;
            s.emit(Event::SettingsChanged);
            Ok(out)
        }
        "plugin.guide" => Ok(json!({ "guide": guide(), "abi": kimchi_plugin::ABI_VERSION, "sources": plugin_dev::sources()?, "installed": cat::own_folder() })),
        "plugin.toolchain" => Ok(json!(plugin_dev::toolchain().await)),
        "plugin.new" => {
            let made = plugin_dev::new_crate(a.str("name")?, a.opt_str("kind").unwrap_or("effect"), a.opt_str("id"), a.opt_str("description"))?;
            let lib = std::fs::read_to_string(made.path.join("src/lib.rs")).unwrap_or_default();
            let mut v = json!(made);
            v["source"] = json!(lib);
            v["next"] = json!("Write src/lib.rs (plugin.writeSource), then plugin.build.");
            Ok(v)
        }
        "plugin.writeSource" => plugin_dev::write_source(a.str("name")?, a.str("path")?, a.str("contents")?),
        "plugin.build" => {
            let b = plugin_dev::build(a.str("name")?).await?;
            let mut v = json!(b);
            v["next"] = json!(if b.ok { "plugin.publishLocal installs it and loads it in kimchi." } else { "Fix the errors (file, line, message), then plugin.build again." });
            Ok(v)
        }
        "plugin.publishLocal" => {
            let name = a.str("name")?.to_string();
            let b = plugin_dev::build(&name).await?;
            if !b.ok {
                let mut v = json!(b);
                v["published"] = json!(false);
                v["next"] = json!("The build failed: fix the errors, then plugin.publishLocal again.");
                return Ok(v);
            }
            let library = b.library.clone().ok_or("The build made no library (is crate-type = [\"cdylib\"] in Cargo.toml?).")?;
            let dir = plugin_dev::make_bundle(&name, &library)?;
            let mut out = tokio::task::spawn_blocking(move || install(&dir)).await.map_err(crate::session::err)??;
            out["published"] = json!(true);
            out["build"] = json!({ "seconds": b.seconds, "warnings": b.warnings });
            out["next"] = json!("Try it: clip.addPlugin {clipIds, plugin} (transition.set plugin=… for a transition), then project.renderFrame and look.");
            s.emit(Event::SettingsChanged);
            Ok(out)
        }
        "clip.addPlugin" => {
            let p = s.project()?;
            let ids = resolve::clips(&p, &a.strings("clipIds"))?;
            if ids.is_empty() {
                return Err("`clipIds` is empty".into());
            }
            let info = cat::lookup(a.str("plugin")?)?;
            if info.kind == PluginKind::Transition {
                return Err(format!("{} is a transition: put it between clips with transition.set plugin=\"{}\".", info.name, info.id));
            }
            if host::is_off(&info.id) {
                return Err(format!("{} is switched off (plugin.enable switches it on).", info.name));
            }
            let params = params_of(&info, a.object("params"), &BTreeMap::new())?;
            let mut edits = vec![];
            let mut slots = vec![];
            for id in &ids {
                let clip = p.clip(*id).ok_or("clip not found")?;
                if matches!(clip.content, ClipContent::Pending { .. }) || p.locate_clip(*id).is_some_and(|(t, _)| p.tracks[t].kind != kimchi_core::TrackKind::Video) {
                    return Err(format!("\"{}\" has no picture for a video plugin.", clip.name));
                }
                let mut fx = clip.effects.clone();
                let (slot, n) = fx.next_plugin_id();
                fx.last_plugin = n;
                let at = a.opt_i64("index").map(|i| (i.max(0) as usize).min(fx.plugins.len())).unwrap_or(fx.plugins.len());
                fx.plugins.insert(at, PluginEffect { id: slot.clone(), plugin: info.id.clone(), name: info.name.clone(), bypass: false, params: params.clone() });
                slots.push(json!({ "clipId": id, "slot": slot }));
                edits.push(Edit::UpdateClip { clip_id: *id, patch: ClipPatch { effects: Some(fx), ..Default::default() } });
            }
            crate::commands::clip::apply_all(s, cx, &edits, None)?;
            Ok(json!({ "plugin": info.id, "name": info.name, "slots": slots }))
        }
        "clip.setPlugin" => {
            let (id, clip, i) = slot_of(s, &a)?;
            let mut fx = clip.effects.clone();
            let slot = &mut fx.plugins[i];
            if let Some(o) = a.object("params") {
                let info = cat::find(&slot.plugin).ok_or_else(|| format!("{} ({}) isn't on this computer, so its values can't be checked; they are kept as they are.", slot.name, slot.plugin))?;
                slot.params = params_of(&info, Some(o), &slot.params)?;
            }
            if let Some(b) = a.opt_bool("bypass") {
                slot.bypass = b;
            }
            if let Some(n) = a.opt_str("name") {
                slot.name = n.trim().to_string();
            }
            let summary = slot_summary(slot);
            s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id: id, patch: ClipPatch { effects: Some(fx), ..Default::default() } }, a.coalesce())?;
            Ok(json!({ "clipId": id, "slot": summary }))
        }
        "clip.removePlugin" => {
            let (id, clip, i) = slot_of(s, &a)?;
            let mut fx = clip.effects.clone();
            let gone = fx.plugins.remove(i);
            let prefix = format!("plugins.{}.", gone.id);
            let mut keys = clip.keyframes.clone();
            keys.retain(|k, _| !k.starts_with(&prefix));
            let keyframes = (keys != clip.keyframes).then_some(keys);
            s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id: id, patch: ClipPatch { effects: Some(fx), keyframes, ..Default::default() } }, None)?;
            Ok(json!({ "clipId": id, "removed": gone.id, "name": gone.name }))
        }
        "clip.movePlugin" => {
            let (id, clip, i) = slot_of(s, &a)?;
            let mut fx = clip.effects.clone();
            let to = (a.opt_i64("index").ok_or("index is required")?.max(0) as usize).min(fx.plugins.len() - 1);
            let slot = fx.plugins.remove(i);
            fx.plugins.insert(to, slot);
            let order: Vec<&str> = fx.plugins.iter().map(|p| p.id.as_str()).collect();
            let order = json!(order);
            s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id: id, patch: ClipPatch { effects: Some(fx), ..Default::default() } }, None)?;
            Ok(json!({ "clipId": id, "order": order }))
        }
        _ => Err(super::unhandled(cx)),
    }
}

/// The guide an agent reads before writing a plugin, with the three starting templates.
pub fn guide() -> String {
    let mut g = kimchi_plugin::GUIDE.to_string();
    g.push_str("\n## Templates (what plugin.new writes, before it renames them)\n");
    for kind in ["effect", "generator", "transition"] {
        let (code, ..) = kimchi_plugin::template::of(kind).expect("a template per kind");
        g.push_str(&format!("\n### {kind}\n\n```rust\n{}```\n", code));
    }
    g
}

// ---- the catalogue ---------------------------------------------------------------------------

/// kimchi's own picture effects (parameters of every clip), as stock plugins.
const STOCK_EFFECTS: &[(&str, &str, &str)] = &[
    ("brightness", "Brightness", "Darker or brighter."),
    ("contrast", "Contrast", "Flatter or punchier."),
    ("saturation", "Saturation", "Black and white to twice the colour."),
    ("temperature", "Temperature", "Cooler (blue) or warmer (orange)."),
    ("tint", "Tint", "Greener or more magenta."),
    ("vignette", "Vignette", "Darker corners."),
    ("sharpen", "Sharpen", "Crisper edges."),
    ("chromaKey", "Chroma key", "Makes a green or blue screen transparent."),
    ("lut", "LUT", "A colour lookup table (.cube, .3dl, .csp, .spi1d, .spi3d, Hald CLUT pictures)."),
    ("blur", "Blur", "A gaussian blur of the whole clip, keyframable."),
];

/// What ships in kimchi: colour effects, transitions, looks, ryolune's sound effects, and the
/// plugins built on the SDK that are linked in.
pub fn stock() -> Vec<Value> {
    let mut out: Vec<Value> = STOCK_EFFECTS
        .iter()
        .map(|(id, name, doc)| json!({ "id": format!("stock:effect.{id}"), "name": name, "kind": "effect", "format": "kimchi", "source": "stock", "description": doc, "use": "clip.setEffects", "enabled": true }))
        .collect();
    out.extend(kimchi_core::transition::KINDS.iter().map(|k| json!({ "id": format!("stock:transition.{}", k.id), "name": k.label, "kind": "transition", "format": "kimchi", "source": "stock", "description": k.doc, "use": "transition.set", "enabled": true })));
    out.extend(kimchi_core::effects::LOOKS.iter().filter(|l| l.id != "none").map(|l| json!({ "id": format!("stock:look.{}", l.id), "name": l.label, "kind": "look", "format": "kimchi", "source": "stock", "description": l.doc, "use": "looks.apply", "enabled": true })));
    out.extend(host::native::built_ins().iter().map(|p| {
        let mut v = plugin_summary(p);
        v["source"] = json!("stock");
        v
    }));
    out.extend(kimchi_audio::plugins::effects(None).into_iter().filter(|e| e.format == "ryolune").map(|e| {
        json!({ "id": e.id, "name": e.name, "kind": "sound", "format": "ryolune", "source": "stock", "category": e.category, "description": e.description, "use": "audio.addEffect", "enabled": true })
    }));
    out
}

/// Installed plugins: lsuite bundles and frei0r (video), and the sound plugins ryolune's engine
/// found (CLAP, VST3, Audio Units, ryolune native). LUTs are looks (`looks.list`).
async fn installed(s: &Arc<Session>) -> Vec<Value> {
    let mut out: Vec<Value> = cat::plugins().iter().filter(|p| !p.is_built_in()).map(plugin_summary).collect();
    let off = s.settings().plugins.disabled;
    out.extend(kimchi_audio::plugins::effects(None).into_iter().filter(|e| e.format != "ryolune").map(|e| {
        let enabled = !off.contains(&e.id);
        json!({ "id": e.id, "name": e.name, "kind": "sound", "format": e.format, "vendor": e.vendor, "source": "installed", "category": e.category, "description": e.description, "use": "audio.addEffect", "enabled": enabled })
    }));
    out
}

async fn list(s: &Arc<Session>, a: &Args) -> Value {
    let q = a.opt_str("query").map(str::to_lowercase);
    let kind = a.opt_str("kind").map(str::to_lowercase);
    let source = a.opt_str("source").map(str::to_lowercase);
    let keep = |v: &Value| {
        let text = |k: &str| v[k].as_str().unwrap_or("").to_lowercase();
        q.as_deref().is_none_or(|q| ["name", "vendor", "category", "id", "description", "format"].iter().any(|k| text(k).contains(q)))
            && kind.as_deref().is_none_or(|k| text("kind") == k)
            && source.as_deref().is_none_or(|src| text("source") == src)
    };
    let mut all = stock();
    all.extend(installed(s).await);
    let plugins: Vec<Value> = all.into_iter().filter(keep).collect();
    let report = cat::report();
    json!({
        "plugins": plugins,
        "formats": formats(&s.data_dir),
        "failed": report.failed,
        "scannedAt": report.scanned_at,
        "folder": cat::own_folder(),
    })
}

/// What kimchi loads, where it looks, and how many it found of each.
pub fn formats(data_dir: &Path) -> Vec<Value> {
    let luts = crate::looks::Library::new(data_dir).list().iter().filter(|l| l.effects.lut.is_some()).count();
    let folders = cat::folders();
    let of = |f: Format| folders.iter().filter(|x| x.format == Some(f) || x.format.is_none()).map(|x| json!({ "path": x.path, "exists": x.exists })).collect::<Vec<_>>();
    let count = |f: Format| cat::plugins().iter().filter(|p| p.format == f && !p.is_built_in()).count();
    let audio = kimchi_audio::plugins::effects(None);
    let audio_count = |fmt: &str| audio.iter().filter(|e| e.format.eq_ignore_ascii_case(fmt)).count();
    let mut out = vec![
        json!({ "id": "lsuite", "name": "lsuite plugins", "kind": "video", "description": "Effects, generators and transitions in Rust, built with the kimchi-plugin SDK (ask your agent for one).", "folders": of(Format::Kimchi), "found": count(Format::Kimchi) }),
        json!({ "id": "frei0r", "name": "frei0r", "kind": "video", "description": "The open video filters Kdenlive, Shotcut and ffmpeg use (the frei0r-plugins package).", "folders": of(Format::Frei0r), "found": count(Format::Frei0r) }),
        json!({ "id": "lut", "name": "LUTs", "kind": "look", "description": ".cube, .3dl, .csp, .spi1d, .spi3d and Hald CLUT pictures, imported as looks (looks.import).", "folders": [], "found": luts }),
        json!({ "id": "ryolune", "name": "ryolune effects", "kind": "sound", "description": "ryolune's own effects and plugins, through ryolune's engine.", "folders": [], "found": audio_count("Native") + audio_count("ryolune") }),
        json!({ "id": "clap", "name": "CLAP", "kind": "sound", "description": "Sound plugins in the CLAP format, through ryolune's engine.", "folders": [], "found": audio_count("CLAP") }),
        json!({ "id": "vst3", "name": "VST3", "kind": "sound", "description": "Sound plugins in the VST3 format, through ryolune's engine.", "folders": [], "found": audio_count("VST3") }),
    ];
    if cfg!(target_os = "macos") {
        out.push(json!({ "id": "au", "name": "Audio Units", "kind": "sound", "description": "macOS sound plugins, through ryolune's engine.", "folders": [], "found": audio_count("AU") }));
    }
    out
}

fn info(s: &Arc<Session>, a: &Args) -> CmdResult {
    let (info, current) = match (a.opt_str("clipId"), a.get("slot")) {
        (Some(clip), Some(slot)) => {
            let p = s.project()?;
            let id = resolve::clip(&p, clip)?;
            let clip = p.clip(id).ok_or("clip not found")?;
            let i = clip.effects.plugin_index(&slot_key(slot))?;
            let slot = &clip.effects.plugins[i];
            let t = s.ui_state().playhead.clamp(clip.start, clip.end());
            let now = clip.effects_at(t).plugins.into_iter().find(|p| p.id == slot.id).map(|p| p.params).unwrap_or_default();
            match cat::find(&slot.plugin) {
                Some(info) => (info, Some((slot.clone(), now, clip.clone()))),
                None => return Ok(json!({ "plugin": slot.plugin, "slot": slot.id, "missing": true, "values": slot.params, "note": "This plugin isn't on this computer: its settings are kept as they are." })),
            }
        }
        (None, None) => {
            let key = a.opt_str("id").or(a.opt_str("plugin")).ok_or("Give id, or clipId and slot.")?;
            if let Some(st) = stock().into_iter().find(|v| v["id"] == key && !v["id"].as_str().unwrap_or("").starts_with("kimchi:")) {
                return Ok(st);
            }
            (cat::lookup(key)?, None)
        }
        _ => return Err("Give id, or clipId with slot.".into()),
    };
    let params: Vec<Value> = info
        .params
        .iter()
        .map(|p| {
            let mut v = param_summary(p);
            if let Some((slot, now, clip)) = &current {
                let value = now.get(&p.name).map(|v| value::normalize(p, v)).unwrap_or_else(|| value::default_of(p));
                v["value"] = json!(value);
                v["shown"] = json!(value::describe(p, &value));
                let key = format!("plugins.{}.{}", slot.id, p.name);
                if let Some(keys) = clip.keyframes.get(&key) {
                    v["keyframes"] = json!(keys);
                }
            }
            v
        })
        .collect();
    let mut out = plugin_summary(&info);
    out["params"] = json!(params);
    if let Some((slot, _, _)) = &current {
        out["slot"] = slot_summary(slot);
    }
    Ok(out)
}

/// `slot` as text (positions may come as numbers).
fn slot_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn slot_of(s: &Arc<Session>, a: &Args) -> CmdResult<(Id, Clip, usize)> {
    let p = s.project()?;
    let id = resolve::clip(&p, a.str("clipId")?)?;
    let clip = p.clip(id).ok_or("clip not found")?.clone();
    let i = clip.effects.plugin_index(&slot_key(a.get("slot").ok_or("slot is required")?))?;
    Ok((id, clip, i))
}

/// `given` checked against the plugin's parameters, over `base`.
fn params_of(info: &PluginInfo, given: Option<&Map<String, Value>>, base: &BTreeMap<String, PluginValue>) -> CmdResult<BTreeMap<String, PluginValue>> {
    let mut out = base.clone();
    for (name, v) in given.into_iter().flatten() {
        let p = value::param(info, name)?;
        out.insert(p.name.clone(), value::from_json(p, v)?);
    }
    Ok(out)
}

pub fn plugin_summary(p: &PluginInfo) -> Value {
    let failure = host::failure(&p.id);
    json!({
        "id": p.id,
        "name": p.name,
        "vendor": p.vendor,
        "format": if p.format == Format::Kimchi { "lsuite" } else { p.format.prefix() },
        "kind": p.kind.label(),
        "category": p.category,
        "description": p.description,
        "version": p.version,
        "path": if p.is_built_in() { Value::Null } else { json!(p.path) },
        "source": if p.is_built_in() { "stock" } else { "installed" },
        "bundle": if p.bundle.is_empty() { Value::Null } else { json!(p.bundle) },
        "params": p.params.len(),
        "enabled": !host::is_off(&p.id),
        "failed": failure,
        "use": if p.kind == PluginKind::Transition { "transition.set" } else { "clip.addPlugin" },
    })
}

fn param_summary(p: &ParamInfo) -> Value {
    let mut v = json!({ "name": p.name, "type": p.kind.label(), "default": value::default_of(p) });
    if p.label != p.name {
        v["label"] = json!(p.label);
    }
    if matches!(p.kind, host::ParamKind::Number | host::ParamKind::Integer) {
        v["min"] = json!(p.min);
        v["max"] = json!(p.max);
    }
    for (k, s) in [("unit", &p.unit), ("group", &p.group), ("hint", &p.hint)] {
        if !s.is_empty() {
            v[k] = json!(s);
        }
    }
    if !p.choices.is_empty() {
        v["choices"] = json!(p.choices);
    }
    if !p.extensions.is_empty() {
        v["extensions"] = json!(p.extensions);
    }
    if p.kind.animates() {
        v["keyframes"] = json!(true);
    }
    v
}

/// A plugin on a clip, for listings: missing when the plugin isn't on this computer, off when
/// it is switched off.
pub fn slot_summary(p: &PluginEffect) -> Value {
    let mut v = json!({ "slot": p.id, "plugin": p.plugin, "name": p.name });
    if p.bypass {
        v["bypass"] = json!(true);
    }
    if !p.params.is_empty() {
        v["params"] = json!(p.params);
    }
    if cat::find(&p.plugin).is_none() {
        v["missing"] = json!(true);
    } else if host::is_off(&p.plugin) {
        v["off"] = json!(true);
    }
    v
}

fn report_json(r: &cat::ScanReport) -> Value {
    let counts: Map<String, Value> = r.counts.iter().map(|(f, n)| (if *f == Format::Kimchi { "lsuite".to_string() } else { f.prefix().to_string() }, json!(n))).collect();
    json!({
        "plugins": counts,
        "described": r.described,
        "failed": r.failed,
        "folders": r.folders,
        "scannedAt": r.scanned_at,
        "soundPlugins": "Sound plugins are scanned by audio.rescanPlugins and listed by audio.effects.",
    })
}

// ---- bundles ---------------------------------------------------------------------------------

/// Copies a bundle into `~/.lsuite/plugins/kimchi/<id>/` after checking it loads (in a child
/// process, so a broken library can't take kimchi down), then rescans: the new plugins are
/// usable at once, and a new build of an installed one replaces it (hot reload).
pub fn install(dir: &Path) -> CmdResult<Value> {
    let m = bundle::Manifest::read(dir)?;
    m.library(dir)?;
    let plugins = cat::probe(Format::Kimchi, dir).map_err(|e| format!("{} doesn't load: {e}", m.name))?;
    if plugins.is_empty() {
        return Err(format!("{} has no plugins in it.", m.name));
    }
    let root = cat::own_folder().ok_or("Plugins aren't set up in this program.")?;
    std::fs::create_dir_all(&root).map_err(|e| format!("Couldn't create {}: {e}", root.display()))?;
    let dest = root.join(&m.id);
    let fresh = root.join(format!(".{}.{}.new", m.id, std::process::id()));
    let _ = std::fs::remove_dir_all(&fresh);
    copy_dir(dir, &fresh).map_err(|e| format!("Couldn't copy the bundle: {e}"))?;
    let old = root.join(format!(".{}.{}.old", m.id, std::process::id()));
    if dest.exists() {
        std::fs::rename(&dest, &old).map_err(|e| format!("Couldn't replace {}: {e}", dest.display()))?;
    }
    std::fs::rename(&fresh, &dest).map_err(|e| format!("Couldn't install into {}: {e}", dest.display()))?;
    let _ = std::fs::remove_dir_all(&old);
    for p in &plugins {
        host::clear_failure(&p.id);
    }
    let report = cat::rescan(false, &|_| {})?;
    host::bump();
    let ids: Vec<Value> = plugins.iter().map(|p| json!({ "id": p.id, "name": p.name, "kind": p.kind.label() })).collect();
    Ok(json!({ "installed": m.id, "name": m.name, "version": m.version, "folder": dest, "plugins": ids, "failed": report.failed }))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

/// Removes an installed lsuite plugin bundle (by its bundle id or one of its plugins' ids).
pub fn remove(id: &str) -> CmdResult<Value> {
    let root = cat::own_folder().ok_or("Plugins aren't set up in this program.")?;
    let bundle_id = match cat::lookup(id) {
        Ok(p) if p.is_built_in() => return Err(format!("{} ships with kimchi: switch it off with plugin.disable instead.", p.name)),
        Ok(p) if p.format != Format::Kimchi => return Err(format!("{} is a {} plugin, installed outside kimchi: switch it off with plugin.disable.", p.name, p.format.label())),
        Ok(p) if !p.bundle.is_empty() => p.bundle,
        _ if bundle::valid_id(id) && root.join(id).join(bundle::FILE).is_file() => id.to_string(),
        Ok(p) => return Err(format!("{} isn't in kimchi's plugin folder ({}).", p.name, p.path.display())),
        Err(e) => return Err(e),
    };
    let dir = root.join(&bundle_id);
    if !dir.starts_with(&root) || !dir.join(bundle::FILE).is_file() {
        return Err(format!("There is no installed bundle {bundle_id}."));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't remove {}: {e}", dir.display()))?;
    cat::rescan(false, &|_| {})?;
    host::bump();
    Ok(json!({ "removed": bundle_id }))
}

// ---- keyframes and transitions -------------------------------------------------------------

/// [`kimchi_core::check_clip_key_on`], and for plugin keys: the parameter exists and animates.
pub fn check_key(clip: &Clip, property: &str, value: &KeyValue) -> CmdResult<()> {
    kimchi_core::check_clip_key_on(clip, property, value)?;
    let Some((slot, name)) = kimchi_core::effects::plugin_key(property) else { return Ok(()) };
    let effect = clip.effects.plugins.iter().find(|p| p.id == slot).expect("checked above");
    let Some(info) = cat::find(&effect.plugin) else { return Ok(()) };
    let p = value::param(&info, name)?;
    if p.name != name {
        return Err(format!("Plugin keyframes use the parameter's name: plugins.{slot}.{}.", p.name));
    }
    if !p.kind.animates() {
        return Err(format!("`{}` is a {} parameter: it doesn't take keyframes (set it with clip.setPlugin).", p.name, p.kind.label()));
    }
    let ok = match p.kind {
        host::ParamKind::Point => value.as_vec(2).is_some(),
        host::ParamKind::Color => matches!(value, KeyValue::Text(_)) || matches!(value, KeyValue::Vector(v) if v.len() == 4),
        _ => value.as_f64().is_some(),
    };
    if !ok {
        return Err(format!(
            "`{}` takes {} keyframes.",
            p.name,
            match p.kind {
                host::ParamKind::Point => "[x, y]",
                host::ParamKind::Color => "colour (\"#rrggbb\")",
                _ => "number",
            }
        ));
    }
    Ok(())
}

/// A plugin parameter's value at timeline time `t` (for a keyframe at the playhead).
pub fn current_key(clip: &Clip, property: &str, t: f64) -> CmdResult<KeyValue> {
    let (slot, name) = kimchi_core::effects::plugin_key(property).ok_or_else(|| format!("`{property}` isn't plugins.<slot>.<parameter>"))?;
    let i = clip.effects.plugin_index(slot)?;
    let effect = clip.effects_at(t).plugins.swap_remove(i);
    let info = cat::find(&effect.plugin).ok_or_else(|| format!("{} isn't on this computer: give the keyframe's value.", effect.plugin))?;
    let p = value::param(&info, name)?;
    let v = effect.params.get(&p.name).map(|v| value::normalize(p, v)).unwrap_or_else(|| value::default_of(p));
    Ok(v.to_key())
}

/// A transition plugin for `transition.set`, with its values.
pub fn transition_plugin(key: &str, params: Option<&Map<String, Value>>) -> CmdResult<PluginEffect> {
    let info = cat::lookup(key)?;
    if info.kind != PluginKind::Transition {
        return Err(format!("{} is {} plugin, not a transition: put it on clips with clip.addPlugin.", info.name, if info.kind == PluginKind::Effect { "an effect" } else { "a generator" }));
    }
    Ok(PluginEffect { id: "transition".into(), plugin: info.id.clone(), name: info.name.clone(), bypass: false, params: params_of(&info, params, &BTreeMap::new())? })
}

/// New values for a slot, checked against its plugin.
pub fn set_params(slot: &mut PluginEffect, given: &Map<String, Value>) -> CmdResult<()> {
    let info = cat::find(&slot.plugin).ok_or_else(|| format!("{} isn't on this computer.", slot.plugin))?;
    slot.params = params_of(&info, Some(given), &slot.params)?;
    Ok(())
}
