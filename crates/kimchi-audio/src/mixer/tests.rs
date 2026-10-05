use std::sync::Arc;

use kimchi_core::anim::{Keyframe, set_key};
use kimchi_core::audio::{Bus, Duck, FadeCurve, Insert, Send as Sending, effect_key};
use kimchi_core::transition::{Transition, TransitionKind};
use kimchi_core::{Project, new_id};

use super::*;
use crate::testing::{ToneOpener, asset, project};

const RATE: u32 = 48_000;

fn mix(p: &Project, from: f64, seconds: f64) -> Vec<Frame> {
    let mut m = Mixer::new(Arc::new(p.clone()), ToneOpener::new(), RATE, Mode::Offline).unwrap();
    m.seek(from);
    let mut out = vec![[0.0; 2]; (seconds * RATE as f64) as usize];
    m.render(&mut out);
    out
}

/// The frame at timeline time `t` of a mix that started at `from`.
fn at(out: &[Frame], from: f64, t: f64) -> Frame {
    out[((t - from) * RATE as f64).round() as usize]
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 2e-3
}

fn rms(out: &[Frame]) -> f64 {
    (out.iter().map(|f| (f[0] as f64).powi(2)).sum::<f64>() / out.len().max(1) as f64).sqrt()
}

/// The master limiter would bend loud test signals: off for the gain checks.
fn plain(mut p: Project) -> Project {
    p.mixer.master.limiter = false;
    p
}

#[test]
fn levels_follow_clip_volume_track_and_master_faders() {
    let mut p = plain(project(vec![asset("dc:0.5:0.25", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
    p.tracks[0].clips[0].volume = 0.5;
    p.tracks[0].mix.gain_db = -6.0;
    p.mixer.master.gain_db = 3.0;
    let out = mix(&p, 0.0, 3.0);
    let g = (0.5 * db_to_gain(-6.0) * db_to_gain(3.0)) as f32;
    assert!(close(at(&out, 0.0, 1.0)[0], 0.5 * g) && close(at(&out, 0.0, 1.0)[1], 0.25 * g), "{:?}", at(&out, 0.0, 1.0));
    // Past the clip: silence.
    let out = mix(&p, 4.5, 0.5);
    assert_eq!(at(&out, 4.5, 4.7), [0.0, 0.0]);
    // A muted clip sound and a muted track make none.
    let mut q = p.clone();
    q.tracks[0].clips[0].audio.muted = true;
    assert_eq!(rms(&mix(&q, 0.0, 1.0)), 0.0);
    let mut q = p.clone();
    q.tracks[0].muted = true;
    assert_eq!(rms(&mix(&q, 0.0, 1.0)), 0.0);
}

#[test]
fn pans_follow_ryoluness_law_and_keyframes() {
    let mut p = plain(project(vec![asset("dc:0.5:0.5", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
    p.tracks[0].mix.pan = 1.0;
    let f = at(&mix(&p, 0.0, 2.0), 0.0, 1.0);
    assert!(close(f[0], 0.0) && close(f[1], 0.5), "{f:?}");
    p.tracks[0].mix.pan = 0.0;
    p.tracks[0].clips[0].audio.pan = -0.5;
    let f = at(&mix(&p, 0.0, 2.0), 0.0, 1.0);
    assert!(close(f[0], 0.5) && close(f[1], 0.5 * 0.5f32.sqrt()), "{f:?}");
    // Keyframed pan, from left to right over the clip's first two seconds.
    let c = &mut p.tracks[0].clips[0];
    set_key(&mut c.keyframes, "pan", Keyframe::new(0.0, -1.0, Default::default()));
    set_key(&mut c.keyframes, "pan", Keyframe::new(2.0, 1.0, Default::default()));
    let out = mix(&p, 0.0, 3.0);
    assert!(at(&out, 0.0, 0.01)[1] < 0.06 && close(at(&out, 0.0, 2.5)[0], 0.0));
    assert!(close(at(&out, 0.0, 1.0)[0], 0.5) && close(at(&out, 0.0, 1.0)[1], 0.5));
    // Channels: the right side alone, then swapped.
    let mut q = plain(project(vec![asset("dc:0.2:0.6", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
    q.tracks[0].clips[0].audio.channels = Channels::Right;
    let f = at(&mix(&q, 0.0, 2.0), 0.0, 1.0);
    assert!(close(f[0], 0.6) && close(f[1], 0.6), "{f:?}");
    q.tracks[0].clips[0].audio.channels = Channels::Swap;
    let f = at(&mix(&q, 0.0, 2.0), 0.0, 1.0);
    assert!(close(f[0], 0.6) && close(f[1], 0.2), "{f:?}");
    q.tracks[0].clips[0].audio.channels = Channels::Mono;
    let f = at(&mix(&q, 0.0, 2.0), 0.0, 1.0);
    assert!(close(f[0], 0.4) && close(f[1], 0.4), "{f:?}");
}

#[test]
fn fades_follow_their_curve_and_volume_keyframes_glide() {
    for curve in FadeCurve::ALL {
        let mut p = plain(project(vec![asset("dc:0.5:0.5", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
        let c = &mut p.tracks[0].clips[0];
        c.fade_in = 1.0;
        c.fade_out = 2.0;
        c.audio.fade_curve = curve;
        let out = mix(&p, 0.0, 4.0);
        let expect = (0.5 * curve.gain(0.5)) as f32;
        assert!(close(at(&out, 0.0, 0.5)[0], expect), "{curve:?}: {:?} vs {expect}", at(&out, 0.0, 0.5));
        assert!(close(at(&out, 0.0, 3.0)[0], expect), "{curve:?} fade-out");
        assert!(close(at(&out, 0.0, 1.5)[0], 0.5));
    }
    let mut p = plain(project(vec![asset("dc:0.5:0.5", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
    let c = &mut p.tracks[0].clips[0];
    set_key(&mut c.keyframes, "volume", Keyframe::new(0.0, 0.0, Default::default()));
    set_key(&mut c.keyframes, "volume", Keyframe::new(2.0, 1.0, Default::default()));
    let out = mix(&p, 0.0, 3.0);
    assert!(close(at(&out, 0.0, 1.0)[0], 0.25), "{:?}", at(&out, 0.0, 1.0));
    assert!(close(at(&out, 0.0, 2.5)[0], 0.5));
}

#[test]
fn transitions_crossfade_with_equal_power_over_the_cut() {
    let mut p = plain(project(vec![asset("dc:0.5:0.5", 20.0), asset("dc:0.3:0.3", 20.0)], vec![vec![(0, 0.0, 2.0), (1, 2.0, 2.0)]]));
    // Both clips start into their media, so the incoming one can start early.
    p.tracks[0].clips[1].in_point = 5.0;
    p.tracks[0].clips[1].transition = Some(Transition::new(TransitionKind::Dissolve, 1.0));
    let out = mix(&p, 0.0, 4.0);
    // Halfway through the transition both play at sin(45°).
    let h = std::f32::consts::FRAC_1_SQRT_2;
    assert!(close(at(&out, 0.0, 2.0)[0], 0.5 * h + 0.3 * h), "{:?}", at(&out, 0.0, 2.0));
    // Before and after it, one clip each, at full level.
    assert!(close(at(&out, 0.0, 1.2)[0], 0.5) && close(at(&out, 0.0, 2.8)[0], 0.3));
    // Inside, the outgoing clip still sounds past the cut and the incoming one before it.
    let f = at(&out, 0.0, 2.3)[0];
    assert!(f > 0.3 && f < 0.5 + 0.3, "{f}");
    let heard = heard(&p, &p.tracks[0]);
    assert!((heard[0].clip.end() - 2.5).abs() < 1e-9 && (heard[1].clip.start - 1.5).abs() < 1e-9);
    assert_eq!(heard[1].fade_in_curve, FadeCurve::EqualPower);
}

fn two_tracks() -> Project {
    plain(project(vec![asset("dc:0.5:0.5", 10.0), asset("dc:0.25:0.25", 10.0)], vec![vec![(0, 0.0, 4.0)], vec![(1, 0.0, 4.0)]]))
}

fn bus(p: &mut Project, name: &str) -> Id {
    let id = new_id();
    p.mixer.buses.push(Bus { id, name: name.into(), muted: false, mix: Default::default() });
    id
}

#[test]
fn solo_mute_buses_and_sends() {
    let mut p = two_tracks();
    assert!(close(at(&mix(&p, 0.0, 2.0), 0.0, 1.0)[0], 0.75));
    p.tracks[1].mix.solo = true;
    assert!(close(at(&mix(&p, 0.0, 2.0), 0.0, 1.0)[0], 0.25));
    // A soloed track keeps the bus it feeds; the bus's fader applies.
    let b = bus(&mut p, "Dialogue");
    p.tracks[1].mix.output = Some(b);
    p.mixer.buses[0].mix.gain_db = -6.0;
    let f = at(&mix(&p, 0.0, 2.0), 0.0, 1.0)[0];
    assert!(close(f, 0.25 * db_to_gain(-6.0) as f32), "{f}");
    // A soloed bus keeps what feeds it, and a muted bus silences it.
    p.tracks[1].mix.solo = false;
    p.mixer.buses[0].mix.solo = true;
    assert!(close(at(&mix(&p, 0.0, 2.0), 0.0, 1.0)[0], 0.25 * db_to_gain(-6.0) as f32));
    p.mixer.buses[0].mix.solo = false;
    p.mixer.buses[0].muted = true;
    assert!(close(at(&mix(&p, 0.0, 2.0), 0.0, 1.0)[0], 0.5));
    // Sends: before the fader they ignore it, after it they follow it.
    let mut p = two_tracks();
    let b = bus(&mut p, "Reverb");
    p.tracks[0].mix.gain_db = kimchi_core::audio::MIN_DB;
    p.tracks[0].mix.sends.push(Sending { bus: b, level_db: -6.0, pre_fader: true });
    let f = at(&mix(&p, 0.0, 2.0), 0.0, 1.0)[0];
    assert!(close(f, 0.25 + 0.5 * db_to_gain(-6.0) as f32), "{f}");
    p.tracks[0].mix.sends[0].pre_fader = false;
    assert!(close(at(&mix(&p, 0.0, 2.0), 0.0, 1.0)[0], 0.25));
    // Stems: one track alone through its bus, or one bus alone; solo doesn't matter.
    let mut p = two_tracks();
    let b = bus(&mut p, "Music");
    p.tracks[0].mix.output = Some(b);
    p.tracks[1].mix.solo = true;
    let stem = |sel: Selection| {
        let mut m = Mixer::new(Arc::new(p.clone()), ToneOpener::new(), RATE, Mode::Offline).unwrap().with_selection(sel);
        let mut out = vec![[0.0; 2]; RATE as usize];
        m.render(&mut out);
        out[RATE as usize / 2][0]
    };
    assert!(close(stem(Selection { tracks: Some(vec![p.tracks[0].id]), ..Default::default() }), 0.5));
    assert!(close(stem(Selection { bus: Some(b), ..Default::default() }), 0.5));
    assert!(close(stem(Selection { tracks: Some(vec![p.tracks[1].id]), ..Default::default() }), 0.25));
}

#[test]
fn ducking_pulls_music_down_while_the_dialogue_speaks() {
    // Music all along; dialogue from 2 s to 4 s.
    let mut p = plain(project(vec![asset("dc:0.5:0.5", 20.0), asset("tone:300:0.3", 20.0)], vec![vec![(0, 0.0, 8.0)], vec![(1, 2.0, 2.0)]]));
    p.tracks[0].mix.duck = Some(Duck { amount_db: -12.0, attack: 0.05, release: 0.3, ..Default::default() });
    let mut m = Mixer::new(Arc::new(p.clone()), ToneOpener::new(), RATE, Mode::Offline).unwrap().with_selection(Selection::default());
    let mut out = vec![[0.0; 2]; 8 * RATE as usize];
    m.render(&mut out);
    // Music alone before, ducked by 12 dB while the dialogue plays, back after.
    assert!(close(at(&out, 0.0, 1.0)[0], 0.5));
    let ducked = 0.5 * db_to_gain(-12.0) as f32;
    // During the dialogue: the music's level is what's left once the tone is removed; the tone
    // crosses zero every half period, read there.
    let zero = 2.0 + 10.0 / 300.0;
    let f = at(&out, 0.0, 3.0 + (zero - 2.0) % (1.0 / 300.0));
    assert!((f[0] - ducked).abs() < 0.02, "{f:?} vs {ducked}");
    assert!(close(at(&out, 0.0, 6.0)[0], 0.5));
    let snaps = m.meters().latest().unwrap();
    assert!(snaps.ducking_db.contains_key(&p.tracks[0].id));
}

#[test]
fn automation_moves_faders_and_effect_parameters() {
    let mut p = plain(project(vec![asset("dc:0.5:0.5", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
    let k = &mut p.tracks[0].mix.keyframes;
    set_key(k, "gainDb", Keyframe::new(0.0, 0.0, Default::default()));
    set_key(k, "gainDb", Keyframe::new(2.0, -20.0, Default::default()));
    let out = mix(&p, 0.0, 3.0);
    assert!(close(at(&out, 0.0, 1.0)[0], 0.5 * db_to_gain(-10.0) as f32), "{:?}", at(&out, 0.0, 1.0));
    assert!(close(at(&out, 0.0, 2.5)[0], 0.5 * db_to_gain(-20.0) as f32));
    // An effect parameter: Utility's gain from 0 to -12 dB.
    let mut p = plain(project(vec![asset("dc:0.5:0.5", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
    let gain = crate::plugins::param("stock:Utility", "Gain").unwrap();
    p.tracks[0].mix.effects.push(Insert::new("u1", "stock:Utility", "Utility"));
    let k = &mut p.tracks[0].mix.keyframes;
    set_key(k, &effect_key("u1", gain.id), Keyframe::new(0.0, 0.0, Default::default()));
    set_key(k, &effect_key("u1", gain.id), Keyframe::new(1.0, -12.0, Default::default()));
    let out = mix(&p, 0.0, 3.0);
    assert!(close(at(&out, 0.0, 0.0005)[0].max(at(&out, 0.0, 0.1)[0]), 0.5) || at(&out, 0.0, 0.1)[0] > 0.4);
    let f = at(&out, 0.0, 2.0)[0];
    assert!((f - 0.5 * db_to_gain(-12.0) as f32).abs() < 0.01, "{f}");
}

#[test]
fn ryolune_effects_change_the_sound_and_keep_their_state_through_updates() {
    // A low-pass at 200 Hz takes most of a 5 kHz tone away.
    let mut p = plain(project(vec![asset("tone:5000:0.5", 10.0)], vec![vec![(0, 0.0, 4.0)]]));
    let dry = rms(&mix(&p, 1.0, 1.0));
    let cutoff = crate::plugins::param("stock:Filter", "Cutoff").unwrap();
    let mut filter = Insert::new("f1", "stock:Filter", "Filter");
    filter.params.insert(cutoff.id, 200.0);
    p.tracks[0].mix.effects.push(filter.clone());
    let wet = rms(&mix(&p, 1.0, 1.0));
    assert!(wet < dry * 0.1, "{wet} vs {dry}");
    // Bypassed: as dry.
    p.tracks[0].mix.effects[0].state = "bypassed".into();
    assert!((rms(&mix(&p, 1.0, 1.0)) - dry).abs() < dry * 0.01);

    // A reverb rings on after the clip, and a fader move doesn't cut its tail.
    let mut p = plain(project(vec![asset("tone:440:0.5", 10.0)], vec![vec![(0, 0.0, 0.5)]]));
    let mut space = Insert::new("s1", "stock:Space", "Space");
    space.params.insert(crate::plugins::param("stock:Space", "Mix").unwrap().id, 100.0);
    p.tracks[0].mix.effects.push(space);
    let mut m = Mixer::new(Arc::new(p.clone()), ToneOpener::new(), RATE, Mode::Realtime).unwrap();
    let mut out = vec![[0.0; 2]; (0.6 * RATE as f64) as usize];
    m.render(&mut out);
    let mut q = p.clone();
    q.tracks[0].mix.gain_db = -1.0;
    m.update(Arc::new(q));
    let mut tail = vec![[0.0; 2]; RATE as usize / 10];
    m.render(&mut tail);
    assert!(rms(&tail) > 1e-3, "the tail went on: {}", rms(&tail));
    // The same moment from a fresh start (a seek) has no tail.
    let mut fresh = Mixer::new(Arc::new(p.clone()), ToneOpener::new(), RATE, Mode::Realtime).unwrap();
    fresh.seek(0.6);
    fresh.render(&mut tail);
    assert!(rms(&tail) < 1e-6);
    // Clip effects ring past the clip's end too.
    let mut p = plain(project(vec![asset("tone:440:0.5", 10.0)], vec![vec![(0, 0.0, 0.5)]]));
    let mut echo = Insert::new("e1", "stock:Echo", "Echo");
    echo.params.insert(crate::plugins::param("stock:Echo", "Time").unwrap().id, 200.0);
    p.tracks[0].clips[0].audio.effects.push(echo);
    let out = mix(&p, 0.0, 1.0);
    assert!(rms(&out[(0.6 * RATE as f64) as usize..(0.8 * RATE as f64) as usize]) > 1e-3);
}

#[test]
fn the_limiter_holds_the_ceiling_and_the_output_gain_applies_before_it() {
    let mut p = project(vec![asset("tone:997:3.0", 10.0)], vec![vec![(0, 0.0, 4.0)]]);
    p.mixer.master.ceiling_db = -2.0;
    let out = mix(&p, 0.0, 2.0);
    let r = crate::loudness::measure(&out, RATE);
    assert!(r.true_peak <= -2.0 + 0.05, "{r:?}");
    let mut m = Mixer::new(Arc::new(plain(p.clone())), ToneOpener::new(), RATE, Mode::Offline).unwrap();
    m.set_output_gain(0.1);
    let mut out = vec![[0.0; 2]; RATE as usize];
    m.render(&mut out);
    let peak = crate::dsp::peak(&out[1000..]);
    assert!((peak - 0.3).abs() < 0.01, "{peak}");
}

#[test]
fn plugin_delay_is_compensated_and_seeks_are_exact() {
    // A step at 1 s on two tracks, one through ryolune's Limiter (which looks ahead), panned
    // apart: both sides step on the same frame, at 1 s, also with a limiter on the master.
    let mut p = plain(project(vec![asset("step:1.0:0.5", 10.0)], vec![vec![(0, 0.0, 4.0)], vec![(0, 0.0, 4.0)]]));
    p.tracks[0].mix.pan = -1.0;
    p.tracks[1].mix.pan = 1.0;
    p.tracks[1].mix.effects.push(Insert::new("l1", "stock:Limiter", "Limiter"));
    p.mixer.master.effects.push(Insert::new("l2", "stock:Limiter", "Limiter"));
    let mut m = Mixer::new(Arc::new(p.clone()), ToneOpener::new(), RATE, Mode::Offline).unwrap();
    assert!(m.latency() > 0);
    m.seek(0.5);
    let mut out = vec![[0.0; 2]; RATE as usize];
    m.render(&mut out);
    let onset = |c: usize| out.iter().position(|f| f[c].abs() > 0.1).unwrap();
    let expect = (0.5 * RATE as f64) as usize;
    assert!(onset(0).abs_diff(expect) <= 2 && onset(1).abs_diff(expect) <= 2, "{} {} vs {expect}", onset(0), onset(1));
    assert!((m.position() - 1.5).abs() < 1e-9);
}

#[test]
fn real_time_plays_silence_until_a_source_is_ready_and_stays_in_time() {
    // `count` gives each frame its source frame number: after the wait, what plays is what
    // belongs at that time.
    let p = plain(project(vec![asset("count", 100.0)], vec![vec![(0, 0.0, 10.0)]]));
    let mut m = Mixer::new(Arc::new(p), ToneOpener::slow(20), RATE, Mode::Realtime).unwrap();
    m.seek(1.0);
    let mut out = vec![[0.0; 2]; RATE as usize];
    m.render(&mut out);
    assert_eq!(out[0], [0.0, 0.0], "not ready yet: silence");
    let k = RATE as usize / 2;
    let expect = (RATE as f64 * 1.5) as f32;
    assert!((out[k][0] - expect).abs() <= 1.0, "{} vs {expect}", out[k][0]);
    assert!(m.meters().latest().is_some());
}

#[test]
fn update_reopens_only_clips_whose_timing_changed() {
    let p = plain(project(vec![asset("dc:0.5:0.5", 10.0)], vec![vec![(0, 0.0, 4.0), (0, 5.0, 4.0)]]));
    let opener = ToneOpener::new();
    let mut m = Mixer::new(Arc::new(p.clone()), opener.clone(), RATE, Mode::Realtime).unwrap();
    let mut out = vec![[0.0; 2]; RATE as usize / 2];
    m.render(&mut out);
    let opened = || opener.opened.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(opened(), 1);
    let mut q = p.clone();
    q.tracks[0].clips[0].volume = 0.5;
    q.tracks[0].mix.gain_db = -3.0;
    m.update(Arc::new(q.clone()));
    m.render(&mut out);
    assert_eq!(opened(), 1, "levels don't reopen the source");
    assert!(close(out[100][0], (0.25 * db_to_gain(-3.0)) as f32));
    q.tracks[0].clips[0].in_point = 1.0;
    m.update(Arc::new(q));
    m.render(&mut out);
    assert_eq!(opened(), 2, "a new in-point does");
}

#[test]
fn missing_sources_and_unknown_effects_are_reported_not_fatal() {
    let mut p = plain(project(vec![asset("missing", 10.0), asset("dc:0.5:0.5", 10.0)], vec![vec![(0, 0.0, 2.0)], vec![(1, 0.0, 2.0)]]));
    p.tracks[1].mix.effects.push(Insert::new("x", "stock:Nope", "Nope"));
    let mut m = Mixer::new(Arc::new(p), ToneOpener::new(), RATE, Mode::Offline).unwrap();
    let mut out = vec![[0.0; 2]; RATE as usize];
    m.render(&mut out);
    assert!(close(out[RATE as usize / 2][0], 0.5));
    let problems = m.problems();
    assert!(problems.iter().any(|p| p.contains("missing")) && problems.iter().any(|p| p.contains("Nope")), "{problems:?}");
}
