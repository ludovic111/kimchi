//! Context menus of the timeline (clips, lanes, track headers, markers) and the
//! AI actions that tie generation into the cut: they grab frames with
//! `media.frame` and prefill the composer (`Store::compose`), exactly like the
//! Svelte editor's `actions.ts` did; the person then picks a model and sends.

use gpui::{App, Context, Pixels, Point};
use kimchi_core::{AssetOrigin, Clip, ClipContent, Id, MediaKind, TrackKind};
use serde_json::json;

use super::body::TimelineBody;
use super::geom;
use crate::actions::{Delete, Duplicate, RippleDelete, Split};
use crate::store::{ComposeRef, ComposeRequest, ComposeTarget, MenuEntry, MenuItem, StoreExt};

fn run(name: &'static str, params: serde_json::Value) -> impl Fn(&mut gpui::Window, &mut App) + 'static {
    move |_, cx| cx.store().update(cx, |s, cx| s.run(name, params.clone(), cx))
}

fn seek(cx: &mut App, t: f64) {
    let pb = cx.store().read(cx).playback.clone();
    pb.update(cx, |p, cx| p.seek(t, cx));
}

// ---- menus --------------------------------------------------------------------

pub fn clip_menu(_: &mut TimelineBody, id: Id, position: Point<Pixels>, cx: &mut Context<TimelineBody>) {
    let store = cx.store();
    if !store.read(cx).selection.contains(&id) {
        store.update(cx, |s, cx| s.select(id, false, cx));
    }
    let s = store.read(cx);
    let Some(p) = s.project.clone() else { return };
    let Some((ti, ci)) = p.locate_clip(id) else { return };
    let track = &p.tracks[ti];
    let clip = track.clips[ci].clone();
    let asset = clip.asset_id().and_then(|a| p.asset(a)).cloned();
    let picture = matches!(clip.content, ClipContent::Media { .. }) && asset.as_ref().is_some_and(|a| a.kind != MediaKind::Audio);
    let playhead = s.playback.read(cx).playhead;
    let fps = s.fps();
    let selected: Vec<Clip> = s.selected_clips().into_iter().cloned().collect();
    let locked = track.locked;

    let mut items = vec![
        MenuItem::new("Split at playhead", |w, cx| w.dispatch_action(Box::new(Split), cx)).icon("scissors").shortcut("S").entry(),
        MenuItem::new("Duplicate", |w, cx| w.dispatch_action(Box::new(Duplicate), cx)).icon("copy").shortcut("⌘D").entry(),
    ];
    if picture {
        let inside = playhead.clamp(clip.start, clip.end());
        let (c1, c2, c3) = (clip.clone(), clip.clone(), clip.clone());
        items.push(MenuEntry::Separator);
        items.push(MenuItem::new("Animate this frame", move |_, cx| animate_frame(&c1, inside, cx)).icon("clapperboard").ai().entry());
        items.push(MenuItem::new("Extend shot with AI", move |_, cx| extend_clip(&c2, fps, cx)).icon("arrow-right-to-line").ai().entry());
        let still = playhead.clamp(clip.start, (clip.end() - 0.01).max(clip.start));
        items.push(MenuItem::new("Restyle frame", move |_, cx| restyle_frame(&c3, still, cx)).icon("wand-sparkles").ai().entry());
    }
    if asset.as_ref().is_some_and(|a| a.is_generated()) {
        let (c1, c2) = (clip.clone(), clip.clone());
        items.push(MenuItem::new("Regenerate", move |_, cx| regenerate(&c1, false, cx)).icon("refresh-cw").ai().entry());
        items.push(MenuItem::new("Variation", move |_, cx| regenerate(&c2, true, cx)).icon("dices").ai().entry());
    }
    if selected.len() == 2 && selected.iter().all(|c| p.locate_clip(c.id).is_some_and(|(t, _)| p.tracks[t].kind == TrackKind::Video)) {
        let (a, b) = (selected[0].clone(), selected[1].clone());
        items.push(MenuItem::new("Bridge with AI", move |_, cx| bridge(&a, &b, fps, cx)).icon("waypoints").ai().entry());
    }
    items.push(MenuEntry::Separator);
    items.push(MenuItem::new("Delete", |w, cx| w.dispatch_action(Box::new(Delete), cx)).icon("trash").shortcut("⌫").danger().disabled(locked).entry());
    items.push(MenuItem::new("Ripple delete", |w, cx| w.dispatch_action(Box::new(RippleDelete), cx)).icon("wrap-text").shortcut("⇧⌫").danger().disabled(locked).entry());
    store.update(cx, |s, cx| s.open_menu(position, items, cx));
}

/// Right-click on empty lane space: generate into the gap, close it, add things there.
pub fn lane_menu(_: &mut TimelineBody, index: usize, time: f64, position: Point<Pixels>, cx: &mut Context<TimelineBody>) {
    let store = cx.store();
    let Some(p) = store.read(cx).project.clone() else { return };
    let Some(track) = p.tracks.get(index) else { return };
    let (start, len, in_gap) = geom::gap_at(track, time);
    let label = if in_gap { format!("in the {} gap", geom::short(len)) } else { format!("at {}", geom::short(start)) };
    let audio = track.kind == TrackKind::Audio;
    let tid = track.id;
    let (l1, l2) = (label.clone(), label);
    let items = vec![
        MenuItem::new("Generate video here…", move |_, cx| {
            let target = ComposeTarget { track_id: Some(tid), start, duration: len, label: l1.clone() };
            cx.store().update(cx, |s, cx| s.compose(ComposeRequest { video: true, target: Some(target), ..Default::default() }, cx));
        })
        .icon("sparkles")
        .ai()
        .disabled(audio)
        .entry(),
        MenuItem::new("Generate image here…", move |_, cx| {
            let target = ComposeTarget { track_id: Some(tid), start, duration: len.min(5.), label: l2.clone() };
            cx.store().update(cx, |s, cx| s.compose(ComposeRequest { video: false, target: Some(target), ..Default::default() }, cx));
        })
        .icon("sparkles")
        .ai()
        .disabled(audio)
        .entry(),
        MenuEntry::Separator,
        MenuItem::new("Add text here", move |_, cx| {
            seek(cx, time);
            crate::app::add_text(json!({}), cx);
        })
        .icon("type")
        .disabled(audio)
        .entry(),
        MenuItem::new("Add marker here", run("timeline.addMarker", json!({ "time": time }))).icon("map-pin").entry(),
        MenuItem::new("Move playhead here", move |_, cx| seek(cx, time)).icon("scan-line").entry(),
        MenuEntry::Separator,
        MenuItem::new("Close gap", run("timeline.closeGap", json!({ "trackId": tid, "time": time }))).icon("wrap-text").disabled(!in_gap || track.locked).entry(),
    ];
    store.update(cx, |s, cx| s.open_menu(position, items, cx));
}

pub fn track_menu(_: &mut TimelineBody, i: usize, position: Point<Pixels>, cx: &mut Context<TimelineBody>) {
    let store = cx.store();
    let Some(p) = store.read(cx).project.clone() else { return };
    let Some(track) = p.tracks.get(i) else { return };
    let id = track.id;
    let n = p.tracks.len();
    let this = cx.entity().downgrade();
    let items = vec![
        MenuItem::new("Rename", move |window, cx| {
            this.update(cx, |b, cx| b.start_rename(id, window, cx)).ok();
        })
        .icon("text-cursor-input")
        .entry(),
        MenuEntry::Separator,
        MenuItem::new("Add video track above", run("track.add", json!({ "kind": "video", "index": i }))).icon("film").entry(),
        MenuItem::new("Add audio track below", run("track.add", json!({ "kind": "audio", "index": i + 1 }))).icon("audio-lines").entry(),
        MenuEntry::Separator,
        MenuItem::new("Move up", run("track.move", json!({ "trackId": id, "index": i.saturating_sub(1) }))).icon("chevron-up").disabled(i == 0).entry(),
        MenuItem::new("Move down", run("track.move", json!({ "trackId": id, "index": i + 1 }))).icon("chevron-down").disabled(i + 1 >= n).entry(),
        MenuEntry::Separator,
        MenuItem::new("Delete track", run("track.remove", json!({ "trackId": id }))).icon("trash").danger().entry(),
    ];
    store.update(cx, |s, cx| s.open_menu(position, items, cx));
}

pub fn marker_menu(id: Id, position: Point<Pixels>, cx: &mut App) {
    let items = vec![MenuItem::new("Remove marker", run("timeline.removeMarker", json!({ "markerId": id }))).icon("trash").danger().entry()];
    cx.store().update(cx, |s, cx| s.open_menu(position, items, cx));
}

pub fn add_track_menu(position: Point<Pixels>, cx: &mut App) {
    let items = vec![
        MenuItem::new("Video track", run("track.add", json!({ "kind": "video" }))).icon("film").entry(),
        MenuItem::new("Audio track", run("track.add", json!({ "kind": "audio" }))).icon("audio-lines").entry(),
    ];
    cx.store().update(cx, |s, cx| s.open_menu(position, items, cx));
}

// ---- AI actions ---------------------------------------------------------------

/// Saves the frame `clip` shows at `time` (`media.frame`) and hands its path to `then`.
fn with_frames(frames: Vec<(Id, f64)>, cx: &mut App, then: impl FnOnce(Vec<String>, &mut App) + 'static) {
    let store = cx.store();
    let tasks: Vec<_> = frames.into_iter().map(|(clip, time)| store.update(cx, |s, cx| s.call("media.frame", json!({ "clipId": clip, "time": time }), cx))).collect();
    cx.spawn(async move |cx| {
        let mut paths = vec![];
        for task in tasks {
            match task.await {
                Ok(v) => paths.push(v["path"].as_str().unwrap_or_default().to_string()),
                Err(e) => {
                    store.update(cx, |s, cx| s.error(e, cx));
                    return;
                }
            }
        }
        cx.update(|cx| then(paths, cx));
    })
    .detach();
}

fn compose(req: ComposeRequest, cx: &mut App) {
    cx.store().update(cx, |s, cx| s.compose(req, cx));
}

fn asset_of(clip: &Clip) -> Option<Id> {
    clip.asset_id()
}

fn track_of(clip: Id, cx: &App) -> Option<Id> {
    cx.store().read(cx).track_of(clip).map(|t| t.id)
}

/// Turns the frame under the playhead into a moving shot, placed after the clip.
pub fn animate_frame(clip: &Clip, time: f64, cx: &mut App) {
    let clip = clip.clone();
    with_frames(vec![(clip.id, time)], cx, move |paths, cx| {
        let label = format!("{} @ {time:.2}s", clip.name);
        compose(
            ComposeRequest {
                video: true,
                refs: vec![ComposeRef { role: "start_frame", path: paths[0].clone(), label, asset_id: asset_of(&clip) }],
                target: Some(ComposeTarget { track_id: None, start: clip.end(), duration: 5., label: "after the clip".into() }),
                ..Default::default()
            },
            cx,
        );
    });
}

/// Continues a clip from its last frame, landing right after it on the same track.
pub fn extend_clip(clip: &Clip, fps: f64, cx: &mut App) {
    let clip = clip.clone();
    let t = clip.end() - 1. / fps.max(1.);
    with_frames(vec![(clip.id, t)], cx, move |paths, cx| {
        let track = track_of(clip.id, cx);
        compose(
            ComposeRequest {
                video: true,
                refs: vec![ComposeRef { role: "start_frame", path: paths[0].clone(), label: format!("Last frame of {}", clip.name), asset_id: asset_of(&clip) }],
                target: Some(ComposeTarget { track_id: track, start: clip.end(), duration: 5., label: format!("after “{}”", clip.name) }),
                ..Default::default()
            },
            cx,
        );
    });
}

/// Restyles / edits the frame as a still, landing at that time.
pub fn restyle_frame(clip: &Clip, time: f64, cx: &mut App) {
    let clip = clip.clone();
    with_frames(vec![(clip.id, time)], cx, move |paths, cx| {
        compose(
            ComposeRequest {
                video: false,
                refs: vec![ComposeRef { role: "reference", path: paths[0].clone(), label: format!("{} @ {time:.2}s", clip.name), asset_id: asset_of(&clip) }],
                target: Some(ComposeTarget { track_id: None, start: time, duration: 3., label: "at the playhead".into() }),
                ..Default::default()
            },
            cx,
        );
    });
}

/// A transition shot between two clips: the first one's last frame to the second one's first.
pub fn bridge(a: &Clip, b: &Clip, fps: f64, cx: &mut App) {
    let (first, last) = if a.start < b.start { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
    let frames = vec![(first.id, first.end() - 1. / fps.max(1.)), (last.id, last.start)];
    with_frames(frames, cx, move |paths, cx| {
        let gap = (last.start - first.end()).max(0.);
        let track = track_of(first.id, cx);
        compose(
            ComposeRequest {
                video: true,
                refs: vec![
                    ComposeRef { role: "start_frame", path: paths[0].clone(), label: format!("End of {}", first.name), asset_id: None },
                    ComposeRef { role: "end_frame", path: paths[1].clone(), label: format!("Start of {}", last.name), asset_id: None },
                ],
                target: Some(ComposeTarget { track_id: track, start: first.end(), duration: if gap > 0. { gap } else { 4. }, label: "between the clips".into() }),
                ..Default::default()
            },
            cx,
        );
    });
}

/// Re-opens the composer with a generated clip's original settings (a new seed for a variation).
pub fn regenerate(clip: &Clip, variation: bool, cx: &mut App) {
    let s = cx.store().read(cx);
    let Some(asset) = s.asset_of(clip) else { return };
    let AssetOrigin::Generated(g) = &asset.origin else { return };
    let req = ComposeRequest {
        video: g.task.contains("video"),
        prompt: Some(g.prompt.clone()),
        negative: Some(g.negative_prompt.clone().unwrap_or_default()),
        model: Some(format!("{}::{}", g.provider, g.model)),
        seed: Some(if variation { String::new() } else { g.seed.map(|s| s.to_string()).unwrap_or_default() }),
        duration: g.params.get("duration").and_then(|v| v.as_f64()),
        aspect: g.params.get("aspect_ratio").and_then(|v| v.as_str()).map(str::to_string),
        refs: vec![],
        target: Some(ComposeTarget { track_id: s.track_of(clip.id).map(|t| t.id), start: clip.end(), duration: clip.duration, label: format!("after “{}”", clip.name) }),
    };
    compose(req, cx);
}
