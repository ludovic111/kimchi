//! Video plugins: the catalogue (`plugins.list`, `plugins.params`, `plugins.rescan`) and the
//! plugins on clips (`clip.addPlugin`, `clip.setPlugin`, `clip.removePlugin`, `clip.movePlugin`).
//! Each change is one `UpdateClip` of the clip's effects (and keyframes), so one undo step.

use std::collections::BTreeMap;
use std::sync::Arc;

use kimchi_core::{Clip, ClipContent, ClipPatch, Edit, Id, KeyValue, PluginEffect, PluginValue};
use kimchi_media::render::plugins::{self as catalogue, Format, ParamInfo, PluginInfo, PluginKind, catalogue as cat, value};
use serde_json::{Map, Value, json};

use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    // The person's folders may have changed in Settings since the session started.
    let st = s.settings();
    catalogue::configure(&s.data_dir, &st.plugins.video_folders);
    match cx.spec.name {
        "plugins.list" => {
            let format = a.opt_str("format").map(Format::parse).transpose()?;
            let q = a.opt_str("query").map(str::to_lowercase);
            let list: Vec<Value> = catalogue::plugins()
                .iter()
                .filter(|p| format.is_none_or(|f| p.format == f))
                .filter(|p| q.as_deref().is_none_or(|q| [&p.name, &p.vendor, &p.category, &p.id, &p.description, &p.format.label().to_string()].iter().any(|f| f.to_lowercase().contains(q))))
                .map(plugin_summary)
                .collect();
            let report = cat::report();
            Ok(json!({ "plugins": list, "failed": report.failed.len(), "scannedAt": report.scanned_at }))
        }
        "plugins.params" => {
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
                (None, None) => (catalogue::lookup(a.opt_str("plugin").ok_or("Give plugin, or clipId and slot.")?)?, None),
                _ => return Err("Give plugin, or clipId with slot.".into()),
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
        "plugins.rescan" => {
            let report = tokio::task::spawn_blocking(|| cat::rescan(true, &|p| tracing::info!(path = %p.display(), "scanning video plugin"))).await.map_err(crate::session::err)??;
            Ok(report_json(&report))
        }
        "clip.addPlugin" => {
            let p = s.project()?;
            let ids = resolve::clips(&p, &a.strings("clipIds"))?;
            if ids.is_empty() {
                return Err("`clipIds` is empty".into());
            }
            let info = catalogue::lookup(a.str("plugin")?)?;
            if info.kind == PluginKind::Transition {
                return Err(format!("{} is a transition: put it between clips with transition.set plugin=\"{}\".", info.name, info.id));
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
    json!({
        "id": p.id,
        "name": p.name,
        "vendor": p.vendor,
        "format": p.format.label(),
        "kind": p.kind.label(),
        "category": p.category,
        "description": p.description,
        "version": p.version,
        "path": p.path,
        "params": p.params.len(),
    })
}

fn param_summary(p: &ParamInfo) -> Value {
    let mut v = json!({ "name": p.name, "type": p.kind.label(), "default": value::default_of(p) });
    if p.label != p.name {
        v["label"] = json!(p.label);
    }
    if matches!(p.kind, catalogue::ParamKind::Number | catalogue::ParamKind::Integer) {
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

/// A plugin on a clip, for listings: missing when the plugin isn't on this computer.
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
    }
    v
}

fn report_json(r: &cat::ScanReport) -> Value {
    let counts: Map<String, Value> = r.counts.iter().map(|(f, n)| (f.prefix().to_string(), json!(n))).collect();
    json!({
        "plugins": counts,
        "described": r.described,
        "failed": r.failed,
        "folders": r.folders,
        "scannedAt": r.scanned_at,
        "audioPlugins": "Sound plugins are scanned by audio.rescanPlugins and listed by audio.effects.",
    })
}

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
        catalogue::ParamKind::Point => value.as_vec(2).is_some(),
        catalogue::ParamKind::Color => matches!(value, KeyValue::Text(_)) || matches!(value, KeyValue::Vector(v) if v.len() == 4),
        _ => value.as_f64().is_some(),
    };
    if !ok {
        return Err(format!("`{}` takes {} keyframes.", p.name, match p.kind {
            catalogue::ParamKind::Point => "[x, y]",
            catalogue::ParamKind::Color => "colour (\"#rrggbb\")",
            _ => "number",
        }));
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
    let info = catalogue::lookup(key)?;
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
