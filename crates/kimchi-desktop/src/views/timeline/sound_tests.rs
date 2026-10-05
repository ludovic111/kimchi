//! The volume line on a real timeline: dragged with the pointer it sets the clip's level in one
//! undo step; a double-click on it adds a keyframe, whose point then drags.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use kimchi_control::{Session, SessionOptions, Source};
use kimchi_core::Project;
use serde_json::{Value, json};

use super::Timeline;
use super::geom::{TRACK_GAP, track_h};

struct Fixture {
    rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
    session: Arc<Session>,
}

impl Fixture {
    fn call(&self, name: &str, params: Value) -> Value {
        self.rt.block_on(kimchi_control::call(&self.session, Source::Cli, name, params)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn settle(&self, cx: &mut VisualTestContext, done: impl Fn(&Project) -> bool) -> Project {
        let start = Instant::now();
        loop {
            cx.run_until_parked();
            let p = self.session.read(|ed| ed.project().clone()).unwrap();
            if done(&p) || start.elapsed() > Duration::from_secs(3) {
                return p;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn drag(cx: &mut VisualTestContext, from: gpui::Point<gpui::Pixels>, to: gpui::Point<gpui::Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    for i in 1..=5 {
        let f = i as f32 / 5.;
        cx.simulate_mouse_move(point(from.x + (to.x - from.x) * f, from.y + (to.y - from.y) * f), MouseButton::Left, Modifiers::default());
    }
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn the_volume_line_drags_in_one_step_and_takes_keyframes(cx: &mut TestAppContext) {
    let Ok(tools) = kimchi_media::Tools::locate() else { return eprintln!("ffmpeg not found; skipping") };
    cx.executor().allow_parking();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let session = {
        let _g = rt.enter();
        Session::new(SessionOptions { data_dir: Some(dir.path().join("data")), config_dir: Some(dir.path().join("config")), secrets: None, headless: true }).unwrap()
    };
    let f = Fixture { rt, _dir: dir, session };
    f.call("project.create", json!({ "name": "Sound" }));
    let wav = f.session.data_dir.join("tone.wav");
    std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=330:duration=3"]).arg(&wav).status().unwrap();
    f.call("media.import", json!({ "paths": [wav], "place": true, "start": 0, "trackId": "Audio 1" }));
    let (handle, session) = (f.rt.handle().clone(), f.session.clone());
    cx.update(|cx| {
        gpui_tokio::init_from_handle(cx, handle);
        crate::app::init(session, cx);
    });
    let (view, cx) = cx.add_window_view(Timeline::new);
    cx.run_until_parked();
    let b = cx.update(|_, cx| view.read(cx).body.read(cx).lanes.get());
    // Audio 1 is the second row; at volume 1 its line sits at `line_y(1.0)` in the clip.
    let top = b.origin.y + px(track_h(kimchi_core::TrackKind::Video) + TRACK_GAP + 2.);
    let h = track_h(kimchi_core::TrackKind::Audio);
    let y = top + px(super::body::line_y_for_tests(1.0, h));
    let steps = f.session.read(|ed| ed.undo_steps().len()).unwrap();
    let from = point(b.origin.x + px(90.), y);
    drag(cx, from, point(from.x, from.y - px(8.)));
    let p = f.settle(cx, |p| p.tracks[1].clips[0].volume > 1.01);
    let v = p.tracks[1].clips[0].volume;
    assert!(v > 1.01 && v < 4.0, "louder, got {v}");
    assert_eq!(f.session.read(|ed| ed.undo_steps().len()).unwrap(), steps + 1, "one undo step");

    // A double-click on the line: a keyframe there.
    let y = top + px(super::body::line_y_for_tests(v, h));
    let at = point(b.origin.x + px(60.), y);
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_event(gpui::MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 2, first_mouse: false });
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    let p = f.settle(cx, |p| p.tracks[1].clips[0].keyframes.contains_key("volume"));
    let keys = &p.tracks[1].clips[0].keyframes["volume"];
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!((keys[0].time - 1.0).abs() < 0.05, "at 1 s (60 px), got {}", keys[0].time);
}
