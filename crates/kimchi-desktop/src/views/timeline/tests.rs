//! Pointer tests on a real timeline: a headless session with a project, the
//! timeline as the window's root, simulated mouse events, and the project the
//! session ends up with (the commands run on Tokio, as in the app).

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px};
use kimchi_control::{Session, SessionOptions, Source};
use kimchi_core::Project;
use serde_json::{Value, json};

use super::Timeline;
use crate::store::StoreExt;

struct Fixture {
    _rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
    session: Arc<Session>,
}

impl Fixture {
    fn call(&self, name: &str, params: Value) -> Value {
        self._rt.block_on(kimchi_control::call(&self.session, Source::Cli, name, params)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn project(&self) -> Project {
        self.session.read(|ed| ed.project().clone()).unwrap()
    }

    /// Lets the window and Tokio work until `done` holds (or 3 s pass).
    fn settle(&self, cx: &mut VisualTestContext, done: impl Fn(&Project) -> bool) -> Project {
        let start = Instant::now();
        loop {
            cx.run_until_parked();
            let p = self.project();
            if done(&p) || start.elapsed() > Duration::from_secs(3) {
                return p;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A project with two 2 s titles on one track, at 0 s and 5 s.
fn setup(cx: &mut TestAppContext) -> (Fixture, Entity<Timeline>, &mut VisualTestContext) {
    cx.executor().allow_parking();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let session = {
        let _g = rt.enter();
        Session::new(SessionOptions { data_dir: Some(dir.path().join("data")), config_dir: Some(dir.path().join("config")), secrets: None, headless: true }).unwrap()
    };
    let f = Fixture { _rt: rt, _dir: dir, session };
    f.call("project.create", json!({ "name": "T" }));
    f.call("clip.addText", json!({ "text": "A", "start": 0, "duration": 2 }));
    f.call("clip.addText", json!({ "text": "B", "start": 5, "duration": 2 }));
    let handle = f._rt.handle().clone();
    let session = f.session.clone();
    cx.update(|cx| {
        gpui_tokio::init_from_handle(cx, handle);
        crate::app::init(session, cx);
    });
    let (view, vcx) = cx.add_window_view(Timeline::new);
    vcx.run_until_parked();
    (f, view, vcx)
}

fn lanes(view: &Entity<Timeline>, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.update(|_, cx| view.read(cx).lanes.get())
}

fn drag(cx: &mut VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>, modifiers: Modifiers) {
    cx.simulate_mouse_down(from, MouseButton::Left, modifiers);
    cx.run_until_parked();
    let steps = 5;
    for i in 1..=steps {
        let f = i as f32 / steps as f32;
        cx.simulate_mouse_move(point(from.x + (to.x - from.x) * f, from.y + (to.y - from.y) * f), MouseButton::Left, modifiers);
    }
    cx.simulate_mouse_up(to, MouseButton::Left, modifiers);
}

#[gpui::test]
fn dragging_a_clip_moves_it_with_one_command(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    assert!(b.size.width > px(100.), "the lanes area was measured: {b:?}");
    // 60 px per second: clip A spans x 0..120 of the lanes, row 0 is 2..68 px down.
    let from = point(b.origin.x + px(60.), b.origin.y + px(30.));
    drag(cx, from, point(from.x + px(150.), from.y), Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[0].clips.iter().any(|c| (c.start - 2.5).abs() < 1e-6));
    let starts: Vec<f64> = p.tracks[0].clips.iter().map(|c| c.start).collect();
    assert_eq!(starts, vec![2.5, 5.], "moved by 2.5 s (150 px)");
    // The click selected it.
    let sel = cx.update(|_, cx| cx.store().read(cx).selection.clone());
    assert_eq!(sel, vec![p.tracks[0].clips[0].id]);
}

#[gpui::test]
fn moving_snaps_to_the_next_clip(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    // End of A (2 s) dragged to 4.95 s: within 8 px of B's start (5 s), it sticks there.
    let from = point(b.origin.x + px(60.), b.origin.y + px(30.));
    drag(cx, from, point(from.x + px(177.), from.y), Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[0].clips[0].start > 0.5);
    assert!((p.tracks[0].clips[0].start - 3.).abs() < 1e-6, "snapped to 3 s, got {}", p.tracks[0].clips[0].start);
}

#[gpui::test]
fn trimming_the_end_is_bounded_by_the_neighbour(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    // Right edge of A (x 120) dragged far past B's start.
    let from = point(b.origin.x + px(117.), b.origin.y + px(30.));
    drag(cx, from, point(from.x + px(400.), from.y), Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[0].clips[0].duration > 2.5);
    assert!((p.tracks[0].clips[0].end() - 5.).abs() < 1e-6, "trimmed up to B, got end {}", p.tracks[0].clips[0].end());
}

#[gpui::test]
fn the_ruler_moves_the_playhead(cx: &mut TestAppContext) {
    let (_f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    let ruler_y = b.origin.y - px(10.);
    cx.simulate_mouse_down(point(b.origin.x + px(240.), ruler_y), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(b.origin.x + px(330.), ruler_y), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(point(b.origin.x + px(330.), ruler_y), MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let t = cx.update(|_, cx| cx.store().read(cx).playback.read(cx).playhead);
    assert!((t - 5.5).abs() < 1e-6, "playhead at 5.5 s, got {t}");
}

#[gpui::test]
fn shift_click_adds_to_the_selection(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    let p = f.project();
    cx.simulate_click(point(b.origin.x + px(60.), b.origin.y + px(30.)), Modifiers::default());
    cx.simulate_click(point(b.origin.x + px(360.), b.origin.y + px(30.)), Modifiers { shift: true, ..Default::default() });
    cx.run_until_parked();
    let sel = cx.update(|_, cx| cx.store().read(cx).selection.clone());
    assert_eq!(sel, vec![p.tracks[0].clips[0].id, p.tracks[0].clips[1].id]);
    // A click on empty lane space clears it.
    cx.simulate_click(point(b.origin.x + px(260.), b.origin.y + px(30.)), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.update(|_, cx| cx.store().read(cx).selection.is_empty()));
}

#[gpui::test]
fn ctrl_wheel_zooms_around_the_pointer(cx: &mut TestAppContext) {
    let (_f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    let at = point(b.origin.x + px(300.), b.origin.y + px(30.));
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(60.))),
        modifiers: Modifiers { control: true, ..Default::default() },
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    let (pps, scroll) = cx.update(|_, cx| (cx.store().read(cx).pps, view.read(cx).body.read(cx).scroll_x));
    assert!(pps > 60. * 1.5, "zoomed in, pps {pps}");
    // The time under the pointer (5 s) is still under it.
    assert!(((300. + scroll) / pps - 5.).abs() < 1e-6, "anchor kept: scroll {scroll}, pps {pps}");
}

/// Labels of the open context menu, and runs the one called `pick`.
fn menu(cx: &mut VisualTestContext, pick: Option<&str>) -> Vec<(String, bool)> {
    let entries = cx.update(|_, cx| cx.store().read(cx).menu.clone()).map(|m| m.entries).unwrap_or_default();
    let mut out = vec![];
    for e in entries {
        if let crate::store::MenuEntry::Item(item) = e {
            out.push((item.label.to_string(), item.disabled));
            if pick == Some(item.label.as_ref()) {
                cx.update(|window, cx| (item.action)(window, cx));
            }
        }
    }
    out
}

#[gpui::test]
fn the_gap_menu_closes_the_gap(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    cx.simulate_mouse_down(point(b.origin.x + px(210.), b.origin.y + px(30.)), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let items = menu(cx, Some("Close gap"));
    assert!(items.contains(&("Close gap".into(), false)), "{items:?}");
    assert!(items.iter().any(|(l, d)| l == "Generate video here…" && !d));
    let p = f.settle(cx, |p| (p.tracks[0].clips[1].start - 2.).abs() < 1e-6);
    assert_eq!(p.tracks[0].clips[1].start, 2.);
}

#[gpui::test]
fn the_clip_menu_has_edits_but_no_frame_actions_for_titles(cx: &mut TestAppContext) {
    let (_f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    cx.simulate_mouse_down(point(b.origin.x + px(60.), b.origin.y + px(30.)), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let labels: Vec<String> = menu(cx, None).into_iter().map(|(l, _)| l).collect();
    assert_eq!(
        labels,
        vec!["Split at playhead", "Trim start to playhead", "Trim end to playhead", "Copy", "Cut", "Duplicate", "Dissolve in", "Freeze frame here", "Delete", "Ripple delete"]
    );
}

#[gpui::test]
fn dragging_the_fade_handle_sets_the_fade(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    let fade0 = f.project().tracks[0].clips[0].fade_in;
    // The fade-in handle sits on the top edge where the fade ends (at least 9 px in).
    let x = (fade0 * 60.).max(9.) as f32;
    let from = point(b.origin.x + px(x), b.origin.y + px(2. + 6.));
    drag(cx, from, point(b.origin.x + px(60.), from.y), Modifiers::default());
    let p = f.settle(cx, |p| (p.tracks[0].clips[0].fade_in - 1.).abs() < 1e-6);
    assert!((p.tracks[0].clips[0].fade_in - 1.).abs() < 1e-6, "fade in 1 s, got {}", p.tracks[0].clips[0].fade_in);
    assert_eq!(p.tracks[0].clips[0].start, 0., "the clip didn't move");
}

#[gpui::test]
fn clips_move_to_another_track_and_tracks_reorder(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    f.call("track.add", json!({ "kind": "video", "index": 1 }));
    cx.run_until_parked();
    let b = lanes(&view, cx);
    let (t0, t1) = { let p = f.project(); (p.tracks[0].id, p.tracks[1].id) };
    // Clip B (row 0, 5..7 s) down into row 1 (top 70).
    let from = point(b.origin.x + px(360.), b.origin.y + px(30.));
    drag(cx, from, point(from.x, b.origin.y + px(100.)), Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[1].clips.len() == 1);
    assert_eq!(p.tracks[1].clips.len(), 1, "B is on the second track");
    assert_eq!(p.tracks[1].clips[0].start, 5.);
    // Drag the first track's header below the second one.
    let header = point(b.origin.x - px(150.), b.origin.y + px(30.));
    drag(cx, header, point(header.x, b.origin.y + px(130.)), Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[0].id == t1);
    assert_eq!((p.tracks[0].id, p.tracks[1].id), (t1, t0));
}

/// The timeline under a strip that starts `MediaDrag`s, like the media panel.
struct WithSource {
    asset: kimchi_core::Id,
    timeline: Entity<Timeline>,
}

impl gpui::Render for WithSource {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        let drag = super::dnd::MediaDrag { asset_id: self.asset, name: "still.png".into(), thumbnail: None };
        gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .child(gpui::div().id("source").h(px(40.)).flex_none().on_drag(drag, |d, _, _, cx| cx.new(|_| d.clone())))
            .child(gpui::div().flex_1().min_h_0().child(self.timeline.clone()))
    }
}

#[gpui::test]
fn media_dropped_on_a_track_lands_where_it_was_dropped(cx: &mut TestAppContext) {
    let (f, _, _) = setup(cx);
    let png = f._dir.path().join("still.png");
    image::RgbaImage::from_pixel(64, 36, image::Rgba([200, 80, 60, 255])).save(&png).unwrap();
    let Ok(v) = f._rt.block_on(kimchi_control::call(&f.session, Source::Cli, "media.import", json!({ "paths": [png] }))) else {
        eprintln!("skipped: media.import needs ffprobe");
        return;
    };
    let asset: kimchi_core::Id = v["media"][0]["id"].as_str().unwrap().parse().unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| WithSource { asset, timeline: cx.new(|cx| Timeline::new(window, cx)) });
    cx.run_until_parked();
    let tl = cx.update(|_, cx| view.read(cx).timeline.clone());
    let b = lanes(&tl, cx);
    // From the source strip onto row 0 at 8 s (after clip B, nothing to snap to within 8 px).
    drag(cx, point(px(20.), px(20.)), point(b.origin.x + px(480.), b.origin.y + px(30.)), Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[0].clips.len() == 3);
    let placed: Vec<f64> = p.tracks[0].clips.iter().map(|c| c.start).collect();
    assert_eq!(placed, vec![0., 5., 8.]);
    assert_eq!(p.tracks[0].clips[2].asset_id(), Some(asset));
}

#[gpui::test]
fn double_clicking_a_track_name_renames_it(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    let at = point(b.origin.x - px(100.), b.origin.y + px(30.));
    cx.simulate_event(gpui::MouseDownEvent { position: at, modifiers: Modifiers::default(), button: MouseButton::Left, click_count: 2, first_mouse: false });
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_input("Main");
    cx.simulate_keystrokes("enter");
    let p = f.settle(cx, |p| p.tracks[0].name == "Main");
    assert_eq!(p.tracks[0].name, "Main");
}

#[gpui::test]
fn zooming_from_elsewhere_keeps_the_playhead_in_place(cx: &mut TestAppContext) {
    let (_f, view, cx) = setup(cx);
    cx.update(|_, cx| {
        let pb = cx.store().read(cx).playback.clone();
        pb.update(cx, |p, cx| p.seek(4., cx));
        cx.store().update(cx, |s, cx| s.set_zoom(240., cx));
    });
    cx.run_until_parked();
    let scroll = cx.update(|_, cx| view.read(cx).body.read(cx).scroll_x);
    // At 60 px/s the playhead was 240 px in; it still is.
    assert!((4. * 240. - scroll - 240.).abs() < 1e-6, "scroll {scroll}");
}

#[gpui::test]
fn dragging_on_empty_space_selects_what_it_touches(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    let p = f.project();
    // From 2.5 s (empty, after A) to 6.7 s: the band touches B only.
    drag(cx, point(b.origin.x + px(150.), b.origin.y + px(10.)), point(b.origin.x + px(400.), b.origin.y + px(60.)), Modifiers::default());
    cx.run_until_parked();
    let sel = cx.update(|_, cx| cx.store().read(cx).selection.clone());
    assert_eq!(sel, vec![p.tracks[0].clips[1].id]);
    // The playhead didn't move: that was a selection, not a click.
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).playback.read(cx).playhead), 0.);
}

#[gpui::test]
fn alt_dragging_a_clip_copies_it(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = lanes(&view, cx);
    let original = f.project().tracks[0].clips[0].id;
    // A (0..2 s) dragged 9 s right with ⌥ held: a copy at 9 s, A stays.
    let from = point(b.origin.x + px(60.), b.origin.y + px(30.));
    drag(cx, from, point(from.x + px(540.), from.y), Modifiers { alt: true, ..Default::default() });
    let p = f.settle(cx, |p| p.tracks[0].clips.len() == 3);
    let starts: Vec<f64> = p.tracks[0].clips.iter().map(|c| c.start).collect();
    assert_eq!(starts, vec![0., 5., 9.]);
    assert_eq!(p.tracks[0].clips[0].id, original, "the original didn't move");
}

/// B moved against A: a cut at 2 s.
fn cut(f: &Fixture, cx: &mut VisualTestContext) -> kimchi_core::Id {
    let b = f.project().tracks[0].clips[1].id;
    f.call("clip.move", json!({ "clipId": b, "start": 2 }));
    f.settle(cx, |p| p.tracks[0].clips[1].start == 2.);
    b
}

#[gpui::test]
fn the_plus_on_a_cut_adds_a_dissolve(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = cut(&f, cx);
    let l = lanes(&view, cx);
    // The cut is at x 120; the "+" sits mid-row, over both clips' trim edges.
    cx.simulate_mouse_down(point(l.origin.x + px(120.), l.origin.y + px(35.)), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(point(l.origin.x + px(120.), l.origin.y + px(35.)), MouseButton::Left, Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[0].clips[1].transition.is_some());
    let tr = p.tracks[0].clips[1].transition.clone().expect("a transition on the cut");
    assert_eq!(tr.kind, kimchi_core::TransitionKind::Dissolve);
    assert_eq!((p.tracks[0].clips[0].end(), p.tracks[0].clips[1].start), (2., 2.), "nothing was trimmed");
    assert_eq!(cx.update(|_, cx| cx.store().read(cx).selection.clone()), vec![b]);
}

#[gpui::test]
fn dragging_a_transition_edge_sets_its_length(cx: &mut TestAppContext) {
    let (f, view, cx) = setup(cx);
    let b = cut(&f, cx);
    f.call("transition.set", json!({ "clipIds": [b], "kind": "wipeLeft", "duration": 1 }));
    f.settle(cx, |p| p.tracks[0].clips[1].transition.is_some());
    cx.run_until_parked();
    let l = lanes(&view, cx);
    // The badge spans 1.5–2.5 s (x 90–150); its right edge dragged to 2.75 s makes it 1.5 s long.
    let from = point(l.origin.x + px(147.), l.origin.y + px(35.));
    drag(cx, from, point(l.origin.x + px(165.), from.y), Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[0].clips[1].transition.as_ref().is_some_and(|t| (t.duration - 1.5).abs() < 0.02));
    let tr = p.tracks[0].clips[1].transition.clone().unwrap();
    assert!((tr.duration - 1.5).abs() < 0.02, "1.5 s, got {}", tr.duration);
    assert_eq!(tr.kind, kimchi_core::TransitionKind::WipeLeft, "the kind stayed");
    assert_eq!(p.tracks[0].clips[1].start, 2., "the clip didn't move");
    // Right-click: the kinds, then removing it.
    cx.simulate_mouse_down(point(l.origin.x + px(120.), l.origin.y + px(35.)), MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    let items = menu(cx, Some("Remove transition"));
    assert!(items.iter().any(|(l, _)| l == "Iris") && items.len() == kimchi_core::transition::KINDS.len() + 1, "{items:?}");
    let p = f.settle(cx, |p| p.tracks[0].clips[1].transition.is_none());
    assert!(p.tracks[0].clips[1].transition.is_none());
}
