//! The AI actions that tie generation into the cut (as in kimchi 0.1's editor):
//! each one grabs the frames it needs with `media.frame` and prefills the composer
//! (`Store::compose`); the person still writes the prompt and presses Generate.
//! Regenerate and Variation run `generate.regenerate` instead: the request is already written.

use gpui::App;
use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, Id, MediaKind};
use serde_json::json;

use crate::store::{ComposeRef, ComposeRequest, ComposeTarget, Store, StoreExt};

/// Saves the frame `clip_id` shows at `time`, then hands its path to `then`.
fn frame(clip_id: Id, time: f64, cx: &mut App, then: impl FnOnce(&mut Store, String, &mut gpui::Context<Store>) + 'static) {
    cx.store().update(cx, |s, cx| {
        s.run_then("media.frame", json!({ "clipId": clip_id, "time": time }), cx, move |s, v, cx| match v["path"].as_str() {
            Some(p) => then(s, p.to_string(), cx),
            None => s.error("Couldn't grab the frame.", cx),
        })
    });
}

fn media_asset(clip: &Clip) -> Option<Id> {
    match clip.content {
        ClipContent::Media { asset_id } => Some(asset_id),
        _ => None,
    }
}

fn frame_label(clip: &Clip, t: f64) -> String {
    format!("{} @ {t:.2}s", clip.name)
}

/// The playhead, kept inside the clip.
pub fn time_in(clip: &Clip, cx: &App) -> f64 {
    let t = cx.store().read(cx).playback.read(cx).playhead;
    if clip.contains(t) { t } else { clip.start }
}

/// Turn the frame under the playhead into a moving shot, landing after the clip.
pub fn animate_frame(clip: &Clip, cx: &mut App) {
    let time = time_in(clip, cx);
    let (label, asset, after) = (frame_label(clip, time), media_asset(clip), clip.end());
    frame(clip.id, time, cx, move |s, path, cx| {
        s.compose(
            ComposeRequest {
                video: true,
                refs: vec![ComposeRef { role: "start_frame", path, label, asset_id: asset }],
                target: Some(ComposeTarget { track_id: None, start: after, duration: 5.0, label: "after the clip".into() }),
                ..Default::default()
            },
            cx,
        )
    });
}

/// Continue a clip from its last frame, landing right after it on the same track.
pub fn extend_clip(clip: &Clip, cx: &mut App) {
    let fps = cx.store().read(cx).fps();
    let time = clip.end() - 1.0 / fps;
    let track = cx.store().read(cx).track_of(clip.id).map(|t| t.id);
    let (name, asset, after) = (clip.name.clone(), media_asset(clip), clip.end());
    frame(clip.id, time, cx, move |s, path, cx| {
        s.compose(
            ComposeRequest {
                video: true,
                refs: vec![ComposeRef { role: "start_frame", path, label: format!("Last frame of {name}"), asset_id: asset }],
                target: Some(ComposeTarget { track_id: track, start: after, duration: 5.0, label: format!("after “{name}”") }),
                ..Default::default()
            },
            cx,
        )
    });
}

/// Restyle / edit the frame under the playhead as a still.
pub fn restyle_frame(clip: &Clip, cx: &mut App) {
    let time = time_in(clip, cx);
    let (label, asset) = (frame_label(clip, time), media_asset(clip));
    frame(clip.id, time, cx, move |s, path, cx| {
        s.compose(
            ComposeRequest {
                video: false,
                refs: vec![ComposeRef { role: "reference", path, label, asset_id: asset }],
                target: Some(ComposeTarget { track_id: None, start: time, duration: 3.0, label: "at the playhead".into() }),
                ..Default::default()
            },
            cx,
        )
    });
}

/// A transition shot between two clips: the first one's last frame → the second one's first.
pub fn bridge(a: &Clip, b: &Clip, cx: &mut App) {
    let (first, last) = if a.start < b.start { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
    let fps = cx.store().read(cx).fps();
    let track = cx.store().read(cx).track_of(first.id).map(|t| t.id);
    let gap = (last.start - first.end()).max(0.0);
    let end_time = first.end() - 1.0 / fps;
    frame(first.id, end_time, cx, move |s, start_path, cx| {
        s.run_then("media.frame", json!({ "clipId": last.id, "time": last.start }), cx, move |s, v, cx| {
            let Some(end_path) = v["path"].as_str() else { return s.error("Couldn't grab the frame.", cx) };
            s.compose(
                ComposeRequest {
                    video: true,
                    refs: vec![
                        ComposeRef { role: "start_frame", path: start_path, label: format!("End of {}", first.name), asset_id: None },
                        ComposeRef { role: "end_frame", path: end_path.to_string(), label: format!("Start of {}", last.name), asset_id: None },
                    ],
                    target: Some(ComposeTarget {
                        track_id: track,
                        start: first.end(),
                        duration: if gap > 0.0 { gap } else { 4.0 },
                        label: "between the clips".into(),
                    }),
                    ..Default::default()
                },
                cx,
            )
        })
    });
}

/// Runs a generated clip's request again with `generate.regenerate` (its input pictures,
/// resolution and settings too; `variation`: a new seed); the result lands after the clip.
pub fn regenerate_clip(clip: &Clip, variation: bool, cx: &mut App) {
    let params = json!({ "clipId": clip.id, "variation": variation });
    cx.store().update(cx, |s, cx| s.run("generate.regenerate", params, cx));
}

/// Runs a generated media item's request again (as [`regenerate_clip`]); the result goes to the
/// library.
pub fn regenerate_asset(asset: &Asset, variation: bool, cx: &mut App) {
    if matches!(asset.origin, AssetOrigin::Generated(_)) {
        let params = json!({ "assetId": asset.id, "variation": variation });
        cx.store().update(cx, |s, cx| s.run("generate.regenerate", params, cx));
    }
}

/// An image as the first frame of a new video (media panel: "Animate with AI").
pub fn animate_image(asset: &Asset, cx: &mut App) {
    if asset.kind != MediaKind::Image {
        return;
    }
    let r = ComposeRef { role: "start_frame", path: asset.path.clone(), label: asset.name.clone(), asset_id: Some(asset.id) };
    cx.store().update(cx, |s, cx| s.compose(ComposeRequest { video: true, refs: vec![r], ..Default::default() }, cx));
}

/// An image as the reference of a new still (media panel: "Edit with AI").
pub fn edit_image(asset: &Asset, cx: &mut App) {
    if asset.kind != MediaKind::Image {
        return;
    }
    let r = ComposeRef { role: "reference", path: asset.path.clone(), label: asset.name.clone(), asset_id: Some(asset.id) };
    cx.store().update(cx, |s, cx| s.compose(ComposeRequest { video: false, refs: vec![r], ..Default::default() }, cx));
}
