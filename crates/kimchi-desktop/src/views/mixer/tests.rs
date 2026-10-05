//! The mixer in the headless window: a fader dragged with the pointer is one undo step, solo
//! from a strip, an effect added from the browser opens its panel, a parameter changed there,
//! the mixer toggled by its key and reported in `ui.state`.

use std::time::{Duration, Instant};

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use serde_json::json;

use super::{MixerView, Target};
use crate::app::Workspace;
use crate::store::StoreExt;
use crate::tests::{Fixture, setup, store_settles};

/// A tone on Audio 1, the mixer shown; `None` without ffmpeg.
fn with_sound(cx: &mut TestAppContext) -> Option<(Fixture, Entity<Workspace>, &mut VisualTestContext, Entity<MixerView>)> {
    let tools = kimchi_media::Tools::locate().ok()?;
    let (f, view, cx) = setup(cx);
    let wav = f.session.data_dir.join("tone.wav");
    std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=330:duration=3"]).arg(&wav).status().ok()?;
    f.call("media.import", json!({ "paths": [wav], "place": true, "start": 0, "trackId": "Audio 1" }));
    store_settles(cx, |s| s.project.as_ref().is_some_and(|p| p.tracks.iter().any(|t| !t.clips.is_empty() && t.kind == kimchi_core::TrackKind::Audio)));
    cx.simulate_keystrokes("x");
    cx.run_until_parked();
    let mixer = cx.update(|_, cx| view.read(cx).editor().read(cx).timeline.read(cx).mixer.clone());
    Some((f, view, cx, mixer))
}

fn wait(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
    let start = Instant::now();
    while !done(cx) && start.elapsed() < Duration::from_secs(4) {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn audio_track(f: &Fixture) -> kimchi_core::Track {
    f.project().tracks.into_iter().find(|t| t.name == "Audio 1").unwrap()
}

#[gpui::test]
fn the_mixer_key_shows_it_and_ui_state_says_so(cx: &mut TestAppContext) {
    let Some((f, _, cx, _)) = with_sound(cx) else { return eprintln!("ffmpeg not found; skipping") };
    let state = f.call("ui.state", json!({}));
    assert_eq!(state["audio"]["mixer"], true, "{state}");
    cx.simulate_keystrokes("x");
    cx.run_until_parked();
    assert_eq!(f.call("ui.state", json!({}))["audio"]["mixer"], false);
}

#[gpui::test]
fn a_fader_drag_is_one_undo_step(cx: &mut TestAppContext) {
    let Some((f, _, cx, mixer)) = with_sound(cx) else { return eprintln!("ffmpeg not found; skipping") };
    let track = audio_track(&f);
    let key = format!("track:{}", track.id);
    let b = cx.update(|_, cx| mixer.read(cx).faders.borrow().get(&key).copied()).expect("the fader was drawn");
    let steps = f.session.read(|ed| ed.undo_steps().len()).unwrap();
    // The cap sits at 0 dB, about two thirds up: grab it and pull it down a quarter of the travel.
    let from = point(b.origin.x + b.size.width / 2., b.origin.y + b.size.height * (1. - super::widgets::db_to_pos(0.0)));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    for i in 1..=6 {
        cx.simulate_mouse_move(point(from.x, from.y + b.size.height * 0.04 * i as f32), MouseButton::Left, Modifiers::default());
    }
    cx.simulate_mouse_up(point(from.x, from.y + b.size.height * 0.24), MouseButton::Left, Modifiers::default());
    let p = f.settle(cx, |p| p.tracks.iter().any(|t| t.mix.gain_db < -3.0));
    let gain = p.tracks.iter().find(|t| t.id == track.id).unwrap().mix.gain_db;
    assert!(gain < -3.0 && gain > -30.0, "pulled down, got {gain}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.session.read(|ed| ed.undo_steps().len()).unwrap(), steps + 1, "the whole drag is one step");
}

#[gpui::test]
fn solo_and_effects_from_the_mixer(cx: &mut TestAppContext) {
    let Some((f, view, cx, mixer)) = with_sound(cx) else { return eprintln!("ffmpeg not found; skipping") };
    let track = audio_track(&f);
    let target = Target::Track(track.id);
    cx.update(|_, cx| mixer.update(cx, |m, cx| m.toggle(target, "solo", true, cx)));
    f.settle(cx, |p| p.tracks.iter().any(|t| t.mix.solo));

    // The browser opens for the track's chain; adding opens the effect's panel.
    cx.update(|_, cx| super::open_browser(target, point(px(200.), px(200.)), cx));
    cx.run_until_parked();
    let browser = cx.update(|_, cx| cx.store().read(cx).audio.browser.map(|(t, _)| t));
    assert_eq!(browser, Some(target));
    assert!(cx.update(|_, cx| cx.store().read(cx).audio.effect.is_none()));
    let (browser_view, panel) = cx.update(|_, cx| {
        let tl = view.read(cx).editor().read(cx).timeline.read(cx);
        (tl.browser.clone(), tl.effect.clone())
    });
    cx.update(|_, cx| browser_view.update(cx, |b, cx| b.add("Channel EQ", cx)));
    wait(cx, |cx| cx.update(|_, cx| cx.store().read(cx).audio.effect.is_some()));
    let (t, slot) = cx.update(|_, cx| cx.store().read(cx).audio.effect.clone()).expect("the new effect's panel opened");
    assert_eq!(t, target);
    let chain = audio_track(&f).mix.effects;
    assert_eq!((chain.len(), chain[0].id.clone()), (1, slot.clone()));

    // A parameter changed from its panel.
    cx.update(|_, cx| panel.update(cx, |p, cx| p.set(target, &slot, 0, json!("-9 dB"), None, cx)));
    f.settle(cx, |p| p.tracks.iter().any(|t| t.mix.effects.first().is_some_and(|e| e.params.get(&0) == Some(&-9.0))));
    assert_eq!(audio_track(&f).mix.effects[0].params.get(&0), Some(&-9.0));
}
