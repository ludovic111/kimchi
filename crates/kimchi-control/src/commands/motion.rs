//! `motion.*`: motion graphics and 3D scenes as clips, templates, presets, and the guide agents
//! read before writing a scene. Also the frame renders (`project.renderFrame`) used to check
//! the result.

use std::path::PathBuf;
use std::sync::Arc;

use kimchi_core::anim::{Keyframe, normalize};
use kimchi_core::motion::Scene;
use kimchi_core::templates::{self, Ctx as TemplateCtx};
use kimchi_core::{Clip, ClipContent, ClipPatch, Edit, Id, Project, TemplateRef};
use serde_json::{Map, Value, json};

use crate::commands::project::clip_summary;
use crate::registry::{Args, Ctx};
use crate::resolve;
use crate::session::{CmdResult, Session, err};

/// The guide `motion.guide` returns, by topic.
const GUIDE: &str = include_str!("motion_guide.md");

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "motion.guide" => Ok(json!({ "guide": guide(a.opt_str("topic").unwrap_or("all"))? })),
        "motion.templates" => Ok(json!(templates::TEMPLATES.iter().map(|t| t.describe()).collect::<Vec<_>>())),
        "motion.presets" => Ok(json!(kimchi_core::presets::PRESETS.iter().map(|p| json!({ "id": p.id, "at": p.at, "doc": p.doc })).collect::<Vec<_>>())),
        "motion.add" => {
            let p = s.project()?;
            let scene = parse_scene(&p, a.get("scene").ok_or("Give a scene (see motion.guide).")?)?;
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
            let duration = a.opt_f64("duration").unwrap_or_else(|| (scene.last_key_time() + 1.0).max(3.0)).max(kimchi_core::MIN_CLIP);
            let name = a.opt_str("name").map(str::to_string).unwrap_or_else(|| scene_name(&scene));
            let clip = Clip::new(name, start, duration, ClipContent::Motion { scene, template: None });
            let track_id = a.opt_str("trackId").map(|k| resolve::track(&p, k)).transpose()?;
            let out = s.apply(cx.label(), cx.source, &Edit::AddClip { track_id, clip }, None)?;
            created(s, &out.created_clips)
        }
        "motion.addTemplate" => {
            let p = s.project()?;
            let t = template(a.str("template")?)?;
            let values = a.object("values").cloned().unwrap_or_default();
            let duration = a.opt_f64("duration").unwrap_or(t.duration).max(0.1);
            let scene = t.build(&values, ctx(&p, duration))?;
            check_media(&p, &scene)?;
            let start = a.opt_f64("start").unwrap_or_else(|| s.ui_state().playhead);
            let clip = Clip::new(
                label(t, &values),
                start,
                duration,
                ClipContent::Motion { scene, template: Some(TemplateRef { id: t.id.to_string(), params: values }) },
            );
            let track_id = a.opt_str("trackId").map(|k| resolve::track(&p, k)).transpose()?;
            let out = s.apply(cx.label(), cx.source, &Edit::AddClip { track_id, clip }, None)?;
            created(s, &out.created_clips)
        }
        "motion.get" => {
            let p = s.project()?;
            let (clip, scene, template) = motion_clip(&p, a.str("clipId")?)?;
            match a.opt_str("id") {
                Some(id) => scene.item_json(id).ok_or_else(|| format!("No layer, object or light \"{id}\" in \"{}\". Ids: {}.", clip.name, scene.ids().join(", "))),
                None => Ok(json!({ "clipId": clip.id, "name": clip.name, "duration": clip.duration, "template": template, "scene": scene.to_json() })),
            }
        }
        "motion.update" => {
            let p = s.project()?;
            let (clip, ..) = motion_clip(&p, a.str("clipId")?)?;
            let scene = parse_scene(&p, a.get("scene").ok_or("Give the new scene.")?)?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.setLayer" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let layer = a.get("layer").ok_or("Give the layer (with its id).")?;
            if layer.get("id").and_then(Value::as_str).is_none_or(|i| i.trim().is_empty()) {
                return Err("The layer needs an id.".into());
            }
            scene.upsert(layer, a.opt_str("parent"))?;
            scene.normalize();
            scene.validate()?;
            check_media(&p, &scene)?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.removeLayer" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let id = a.str("id")?;
            if !scene.remove(id) {
                return Err(format!("No layer, object or light \"{id}\" in \"{}\". Ids: {}.", clip.name, scene.ids().join(", ")));
            }
            scene.validate().map_err(|e| format!("Removing \"{id}\" would break the scene: {e}"))?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.setKeyframes" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (id, property) = (a.str("id")?, a.str("property")?);
            let keys = keyframes(&a)?;
            let ids = scene.ids();
            let target = scene.keyframes_mut(id).ok_or_else(|| {
                let extra = if scene_is_3d(&clip) { "camera, scene" } else { "scene" };
                format!("No \"{id}\" in \"{}\". Ids: {}, {extra}.", clip.name, ids.join(", "))
            })?;
            if keys.is_empty() {
                target.remove(property);
            } else {
                target.insert(property.to_string(), keys);
                normalize(target);
            }
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.updateLayer" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let id = a.str("id")?;
            let props = a.object("props").ok_or("props is an object of properties and values.")?.clone();
            let at = clip.scene_time(a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead));
            update_item(&mut scene, id, &props, at).map_err(|e| format!("\"{id}\" in \"{}\": {e}", clip.name))?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.addKeyframe" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (id, property) = (a.str("id")?, a.str("property")?);
            let at = clip.scene_time(a.opt_f64("time").unwrap_or_else(|| s.ui_state().playhead));
            let value = match a.get("value") {
                Some(v) => serde_json::from_value::<kimchi_core::KeyValue>(v.clone()).map_err(|_| format!("value {v} should be a number, a vector or a string"))?,
                None => scene.value(id, property, at).ok_or_else(|| missing(&scene, &clip, id, property))?,
            };
            let easing = a.opt_str("easing").map(kimchi_core::Easing::parse).transpose()?.unwrap_or_default();
            let not_found = missing(&scene, &clip, id, property);
            let mut item = scene.item_mut(id).ok_or(not_found)?;
            // Checked like any keyframe value (and the property must exist).
            item.set(property, &value)?;
            kimchi_core::anim::set_key(item.keyframes_mut(), property, Keyframe { time: at, value, easing });
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.removeKeyframe" => {
            let p = s.project()?;
            let (clip, mut scene, _) = motion_clip(&p, a.str("clipId")?)?;
            let (id, property) = (a.str("id")?, a.str("property")?);
            let held = scene.value(id, property, clip.scene_time(s.ui_state().playhead.clamp(clip.start, clip.end())));
            let not_found = missing(&scene, &clip, id, property);
            let mut item = scene.item_mut(id).ok_or(not_found)?;
            if !item.keyframes().contains_key(property) {
                return Err(format!("`{property}` of \"{id}\" isn't animated."));
            }
            match a.opt_f64("time") {
                Some(t) => {
                    if !kimchi_core::anim::remove_key(item.keyframes_mut(), property, clip.scene_time(t)) {
                        return Err(format!("No `{property}` keyframe of \"{id}\" at {t} s."));
                    }
                }
                None => {
                    item.keyframes_mut().remove(property);
                    if let Some(v) = held {
                        let _ = item.set(property, &v);
                    }
                }
            }
            scene.validate()?;
            set_scene(s, cx, &a, clip.id, scene, None)
        }
        "motion.setTemplate" => {
            let p = s.project()?;
            let (clip, _, template) = motion_clip(&p, a.str("clipId")?)?;
            let Some(mut tref) = template else {
                return Err(format!("\"{}\" wasn't made from a template; change its scene with motion.update or motion.setLayer.", clip.name));
            };
            let t = self::template(&tref.id)?;
            let before = label(t, &tref.params);
            for (k, v) in a.object("values").cloned().unwrap_or_default() {
                tref.params.insert(k, v);
            }
            let scene = t.build(&tref.params, ctx(&p, clip.duration))?;
            check_media(&p, &scene)?;
            // A clip still named after its words follows them.
            if clip.name == before {
                let name = label(t, &tref.params);
                let patch = ClipPatch { name: Some(name), scene: Some(scene), template: Some(Some(tref)), ..Default::default() };
                s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id: clip.id, patch }, a.coalesce())?;
                return s.read(|ed| ed.project().clip(clip.id).map(|c| clip_summary(ed.project(), c)).unwrap_or(Value::Null));
            }
            set_scene(s, cx, &a, clip.id, scene, Some(tref))
        }
        _ => crate::commands::motion_edit::run(s, cx, a).await,
    }
}

/// Changes `props` of one thing in a scene: animated ones get a keyframe at scene time `at`,
/// others are set; fields that aren't animatable (fontFamily, align, stroke caps, material
/// flags…) are merged into its JSON.
pub fn update_item(scene: &mut Scene, id: &str, props: &Map<String, Value>, at: f64) -> CmdResult<()> {
    let mut merge = Map::new();
    let not_found = format!("no such layer, object or light; ids: {}", scene.ids().join(", "));
    {
        let mut item = scene.item_mut(id).ok_or(not_found)?;
        for (name, v) in props {
            let value: Option<kimchi_core::KeyValue> = serde_json::from_value(v.clone()).ok();
            let animated = item.keyframes().contains_key(name);
            match value {
                Some(value) if animated => {
                    item.set(name, &value)?;
                    kimchi_core::anim::set_key(item.keyframes_mut(), name, Keyframe { time: at, value, easing: Default::default() });
                }
                Some(value) if item.set(name, &value).is_ok() => {}
                _ => {
                    merge.insert(name.clone(), v.clone());
                }
            }
        }
    }
    if !merge.is_empty() {
        let mut json = match id {
            "scene" => scene.to_json(),
            "camera" => scene.to_json()["camera"].clone(),
            _ => scene.item_json(id).ok_or("gone")?,
        };
        for (k, v) in merge {
            match (json.get_mut(&k), &v) {
                // Nested objects (stroke, material, shadow…) merge one level deep.
                (Some(Value::Object(old)), Value::Object(new)) => {
                    for (nk, nv) in new {
                        old.insert(nk.clone(), nv.clone());
                    }
                }
                _ => {
                    json[&k] = v;
                }
            }
        }
        match id {
            "scene" => *scene = Scene::from_json(&json)?,
            "camera" => {
                let mut whole = scene.to_json();
                whole["camera"] = json;
                *scene = Scene::from_json(&whole)?;
            }
            _ => {
                scene.upsert(&json, None)?;
            }
        }
    }
    scene.normalize();
    scene.validate()?;
    Ok(())
}

fn missing(scene: &Scene, clip: &Clip, id: &str, property: &str) -> String {
    let mut copy = scene.clone();
    match copy.item_mut(id) {
        None => format!("No \"{id}\" in \"{}\". Ids: {}.", clip.name, scene.ids().join(", ")),
        Some(_) => format!("\"{id}\" has no property `{property}` (motion.guide lists them)."),
    }
}

fn guide(topic: &str) -> CmdResult<String> {
    let sections: Vec<(&str, &str)> = GUIDE
        .split("\n## ")
        .skip(1)
        .map(|sec| {
            let (title, _) = sec.split_once('\n').unwrap_or((sec, ""));
            (title.trim(), sec)
        })
        .collect();
    let pick = |names: &[&str]| -> String {
        let head = GUIDE.split("\n## ").next().unwrap_or("");
        let body: Vec<String> = sections.iter().filter(|(t, _)| names.iter().any(|n| t.to_lowercase().starts_with(n))).map(|(_, b)| format!("## {b}")).collect();
        format!("{head}\n{}", body.join("\n"))
    };
    Ok(match topic {
        "all" | "" => GUIDE.to_string(),
        "2d" => pick(&["2d", "text reveal", "keyframes", "checking"]),
        "3d" => pick(&["3d", "keyframes", "checking"]),
        "keyframes" => pick(&["keyframes", "clip animation"]),
        "templates" => pick(&["templates", "clip animation"]),
        other => return Err(format!("topic is 2d, 3d, keyframes, templates or all, not \"{other}\"")),
    })
}

/// A template clip's automatic name: the template, and its title or words when given.
fn label(t: &templates::Template, values: &Map<String, Value>) -> String {
    match ["title", "text", "label", "quote"].iter().find_map(|k| values.get(*k).and_then(Value::as_str)).filter(|v| !v.trim().is_empty()) {
        Some(v) => format!("{} · {}", t.name, v.lines().next().unwrap_or(v).chars().take(32).collect::<String>()),
        None => t.name.to_string(),
    }
}

fn template(id: &str) -> CmdResult<&'static templates::Template> {
    templates::find(id).ok_or_else(|| {
        let ids = templates::ids();
        let hint = kimchi_core::closest(id, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        format!("No template `{id}`.{hint} Templates: {}.", ids.join(", "))
    })
}

fn ctx(p: &Project, duration: f64) -> TemplateCtx {
    TemplateCtx { width: p.settings.width as f64, height: p.settings.height as f64, duration }
}

fn parse_scene(p: &Project, v: &Value) -> CmdResult<Scene> {
    let scene = Scene::from_json(v)?;
    check_media(p, &scene)?;
    Ok(scene)
}

/// Every picture or model a scene names must be a media item or a file.
fn check_media(p: &Project, scene: &Scene) -> CmdResult<()> {
    for r in scene.media_refs() {
        let known = r.parse::<Id>().ok().and_then(|id| p.asset(id)).is_some()
            || p.assets.iter().any(|a| a.name.eq_ignore_ascii_case(&r))
            || PathBuf::from(&r).is_file();
        if !known {
            return Err(format!("\"{r}\" isn't a media item (id or name from media.list) or a file on this computer."));
        }
    }
    Ok(())
}

fn scene_name(scene: &Scene) -> String {
    match scene {
        Scene::Flat(s) => {
            let mut name = None;
            kimchi_core::motion::walk_layers(&s.layers, &mut |l| {
                if name.is_none()
                    && let kimchi_core::motion::LayerKind::Text(t) = &l.kind
                {
                    name = Some(t.shown().lines().next().unwrap_or("").chars().take(32).collect::<String>());
                }
            });
            name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Motion".into())
        }
        Scene::Space(s) => {
            let mut name = None;
            kimchi_core::motion::walk_objects(&s.objects, &mut |o| {
                if name.is_none()
                    && let kimchi_core::motion::Shape3d::Text { text, .. } = &o.shape
                {
                    name = Some(format!("3D · {}", text.chars().take(28).collect::<String>()));
                }
            });
            name.unwrap_or_else(|| "3D scene".into())
        }
    }
}

fn scene_is_3d(c: &Clip) -> bool {
    matches!(&c.content, ClipContent::Motion { scene, .. } if scene.is_3d())
}

/// The motion clip `key` names, its scene and template.
pub(crate) fn motion_clip(p: &Project, key: &str) -> CmdResult<(Clip, Scene, Option<TemplateRef>)> {
    let id = resolve::clip(p, key)?;
    let clip = p.clip(id).ok_or("clip not found")?.clone();
    match &clip.content {
        ClipContent::Motion { scene, template } => {
            let (scene, template) = (scene.clone(), template.clone());
            Ok((clip, scene, template))
        }
        _ => Err(format!("\"{}\" isn't a motion clip; animate it with clip.setKeyframes or clip.animate.", clip.name)),
    }
}

pub(crate) fn set_scene(s: &Arc<Session>, cx: &Ctx, a: &Args, clip_id: Id, scene: Scene, template: Option<TemplateRef>) -> CmdResult {
    let patch = ClipPatch { scene: Some(scene), template: template.map(Some), ..Default::default() };
    s.apply(cx.label(), cx.source, &Edit::UpdateClip { clip_id, patch }, a.coalesce())?;
    s.read(|ed| {
        let p = ed.project();
        p.clip(clip_id).map(|c| clip_summary(p, c)).unwrap_or(Value::Null)
    })
}

fn created(s: &Session, ids: &[Id]) -> CmdResult {
    s.read(|ed| {
        let p = ed.project();
        json!({
            "clips": ids.iter().filter_map(|id| p.clip(*id)).map(|c| clip_summary(p, c)).collect::<Vec<_>>(),
            "next": "Look at it with project.renderFrame {\"times\": [...]}.",
        })
    })
}

/// The `keyframes` argument, every entry checked.
pub fn keyframes(a: &Args) -> CmdResult<Vec<Keyframe>> {
    let list = a.array("keyframes").ok_or("keyframes is a list, like [[0, 0], [0.5, 1, \"easeOut\"]].")?;
    list.iter().enumerate().map(|(i, k)| Keyframe::from_json(k).map_err(|e| format!("keyframes[{i}]: {e}"))).collect()
}

// ---------------------------------------------------------------------------------------------
// Frame renders

/// Renders the timeline at `times` (one frame, or a labelled sheet) into the project's cache
/// and returns the PNG's path.
pub async fn render_png(s: &Arc<Session>, p: &Project, times: &[f64], width: Option<u32>) -> CmdResult<PathBuf> {
    use kimchi_media::tiny_skia::{Color, IntSize, Pixmap, PixmapPaint, Transform};
    let tools = s.tools()?;
    let sheet = times.len() > 1;
    let w = width.unwrap_or(if sheet { 480 } else { 960 }).clamp(64, 3840);
    let h = ((w as f64 * p.settings.height as f64 / p.settings.width.max(1) as f64).round() as u32).max(2);
    let mut frames = vec![];
    for &t in times {
        let f = kimchi_media::preview::render_frame(&tools, p, t, w, h).await.map_err(err)?;
        let px = Pixmap::from_vec(f.rgba, IntSize::from_wh(f.width, f.height).ok_or("bad size")?).ok_or("bad frame")?;
        frames.push((t, px));
    }
    let image = if !sheet {
        frames.pop().map(|(_, p)| p).ok_or("nothing rendered")?
    } else {
        let cols = (times.len() as f64).sqrt().ceil().max(1.0) as u32;
        let rows = (times.len() as u32).div_ceil(cols);
        let (fw, fh) = (frames[0].1.width(), frames[0].1.height());
        let (gap, label) = (8u32, 28u32);
        let (sw, sh) = (cols * fw + (cols + 1) * gap, rows * (fh + label) + (rows + 1) * gap);
        let mut sheet = Pixmap::new(sw, sh).ok_or("sheet too big")?;
        sheet.fill(Color::from_rgba8(24, 24, 28, 255));
        for (i, (t, f)) in frames.iter().enumerate() {
            let (c, r) = (i as u32 % cols, i as u32 / cols);
            let (x, y) = (gap + c * (fw + gap), gap + r * (fh + label + gap));
            sheet.draw_pixmap(x as i32, (y + label) as i32, f.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            let style = kimchi_core::TextStyle {
                content: format!("{t:.2} s"),
                font_family: "IBM Plex Mono".into(),
                font_size: 15.0,
                font_weight: 500,
                color: "#d0d0d8".into(),
                shadow: false,
                align: kimchi_core::TextAlign::Left,
                ..Default::default()
            };
            let m = kimchi_media::text::measure(&style);
            let tf = kimchi_core::Transform {
                x: x as f64 + m.width / 2.0 - sw as f64 / 2.0,
                y: y as f64 + label as f64 / 2.0 - sh as f64 / 2.0,
                ..Default::default()
            };
            let text = kimchi_media::text::rasterize_text(&style, &tf, sw, sh);
            sheet.draw_pixmap(0, 0, text.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        sheet
    };
    let dir = s.cache_dir(p.id).join("renders");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let name = format!("frame-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f"));
    let out = dir.join(name);
    image.save_png(&out).map_err(err)?;
    Ok(out)
}

/// The value a clip property has at timeline time `t` (for keyframes set without a value).
pub fn current_value(clip: &Clip, property: &str, t: f64) -> CmdResult<kimchi_core::KeyValue> {
    use kimchi_core::KeyValue;
    let pl = clip.placement_at(t);
    let local = t - clip.start;
    let num = |name: &str, base: f64| kimchi_core::anim::number_at(&clip.keyframes, name, local).unwrap_or(base);
    let style = clip.text_at(t);
    Ok(match property {
        "x" => KeyValue::Number(pl.x),
        "y" => KeyValue::Number(pl.y),
        "position" => KeyValue::Vector(vec![pl.x, pl.y]),
        "scale" => KeyValue::Number(num("scale", clip.transform.scale)),
        "scaleX" => KeyValue::Number(num("scaleX", 1.0)),
        "scaleY" => KeyValue::Number(num("scaleY", 1.0)),
        "rotation" => KeyValue::Number(pl.rotation),
        "opacity" => KeyValue::Number(pl.opacity),
        "blur" => KeyValue::Number(pl.blur),
        "volume" => KeyValue::Number(clip.volume_at(t)),
        "fontSize" => KeyValue::Number(style.ok_or("fontSize is for text clips")?.font_size),
        "letterSpacing" => KeyValue::Number(style.ok_or("letterSpacing is for text clips")?.letter_spacing),
        "color" => KeyValue::Text(style.ok_or("color is for text clips")?.color),
        fx if kimchi_core::effects::EFFECT_PROPS.contains(&fx) => KeyValue::Number(clip.effects_at(t).get(fx).unwrap_or(0.0)),
        other => return Err(format!("Clips can't animate `{other}`. Clip properties: {}.", kimchi_core::CLIP_PROPS.join(", "))),
    })
}

/// `{name: keyframes}` as JSON, for answers.
pub fn keys_json(k: &kimchi_core::Keyframes) -> Value {
    let mut m = Map::new();
    for (name, list) in k {
        m.insert(name.clone(), json!(list));
    }
    Value::Object(m)
}
