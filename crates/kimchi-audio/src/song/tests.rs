use std::collections::HashMap;

use kimchi_core::anim::{Keyframe, set_key};
use kimchi_core::audio::{Bus, Insert, Send as Sending, effect_key};
use kimchi_core::new_id;
use ryolune_engine::model::{ClipData, Note, fader_gain};
use ryolune_engine::tempo::TempoPoint;

use super::*;
use crate::testing::{asset, project};

/// A ryolune song made with ryolune's own model: the empty song's bass playing four bars of
/// notes, a tempo change at bar 2, two markers.
fn song(dir: &Path) -> PathBuf {
    let mut s = ryolune_engine::store::empty();
    s.name = "Night drive.ryolune".into();
    s.transport.tempo = 120.0;
    s.tempo_changes = vec![TempoPoint { bar: 2.0, bpm: 60.0, ramp: false }];
    let notes = (0..16).map(|i| Note { id: format!("n{i}"), start: i as f64, length: 0.5, pitch: 40 + (i % 5) as u8, velocity: 100, agent: false, channel: 0 }).collect();
    s.clips.push(ryolune_engine::model::Clip {
        id: "bass-1".into(),
        name: "Bass line".into(),
        agent: false,
        track_id: "bass".into(),
        start_bar: 0.0,
        length_bars: 4.0,
        data: ClipData::Midi { notes, controllers: vec![] },
    });
    s.markers = vec![
        ryolune_engine::model::Marker { id: "m1".into(), bar: 0.0, name: "Intro".into(), color: None },
        ryolune_engine::model::Marker { id: "m2".into(), bar: 3.0, name: "Drop".into(), color: None },
    ];
    s.normalize();
    s.validate().unwrap();
    let path = dir.join("night drive.ryolune");
    ryolune_engine::document::save(&s, &HashMap::new(), &path).unwrap();
    path
}

#[test]
fn songs_are_read_and_rendered_by_ryolunes_engine() {
    let dir = tempfile::tempdir().unwrap();
    let path = song(dir.path());
    let i = info(&path).unwrap();
    assert_eq!((i.name.as_str(), i.tempo, i.beats_per_bar), ("Night drive", 120.0, 4.0));
    // Two bars at 120 (4 s), then two at 60 (8 s).
    assert!((i.seconds - 12.0).abs() < 1e-9, "{}", i.seconds);
    assert_eq!(i.tracks, [("bass".to_string(), "Bass".to_string())]);
    assert_eq!(i.markers[1].1, "Drop");
    assert!((i.markers[1].0 - 8.0).abs() < 1e-9, "{:?}", i.markers);
    // Beats follow the tempo change: half a second apart, then a second.
    assert!((i.beats[1] - 0.5).abs() < 1e-9 && (i.beats[9] - i.beats[8] - 1.0).abs() < 1e-9, "{:?}", &i.beats[..10]);
    assert_eq!(i.beats.len(), 17);
    let beats = i.beats();
    assert_eq!((beats.source.as_str(), beats.beats_per_bar), ("ryolune", 4));
    assert!((beats.tempo - 80.0).abs() < 1e-6, "{}", beats.tempo);

    // The mix, at the asked rate, with a tail.
    let out = dir.path().join("mix.wav");
    render(&path, None, &out, 44_100).unwrap();
    let wav = hound::WavReader::open(&out).unwrap();
    assert_eq!(wav.spec().sample_rate, 44_100);
    let seconds = wav.duration() as f64 / 44_100.0;
    assert!((seconds - 15.0).abs() < 0.05, "{seconds}");
    let samples: Vec<f32> = hound::WavReader::open(&out).unwrap().into_samples::<f32>().map(|s| s.unwrap()).collect();
    assert!(samples.iter().any(|s| s.abs() > 0.01), "the bass plays");
    // One track's stem, by name.
    let stem = dir.path().join("bass.wav");
    render(&path, Some("bass"), &stem, 48_000).unwrap();
    assert!(hound::WavReader::open(&stem).unwrap().duration() > 0);
    assert!(render(&path, Some("Drums"), &stem, 48_000).is_ok(), "an empty track renders silence");
    assert!(render(&path, Some("Kazoo"), &stem, 48_000).unwrap_err().contains("Bass"));
    assert!(info(&dir.path().join("missing.ryolune")).is_err());
}

#[test]
fn fader_positions_invert_ryolunes_fader_law() {
    for db in [-60.0, -20.0, -6.0, -1.0, 0.0, 3.0, 6.0] {
        let back = gain_to_db(fader_gain(fader_position(db)) as f64);
        assert!((back - db).abs() < 0.01, "{db} → {back}");
    }
    assert_eq!(fader_position(MIN_DB), 0.0);
    assert_eq!(fader_position(12.0), 1.0, "ryolune's faders stop at +6 dB");
}

#[test]
fn a_cut_becomes_a_session_ryolune_opens() {
    let mut p = project(vec![asset("tone:440:0.5", 60.0), asset("tone:220:0.5", 60.0)], vec![vec![(0, 0.0, 2.0), (0, 4.0, 2.0)], vec![(1, 1.0, 3.0)]]);
    p.name = "Trailer".into();
    let bus = new_id();
    p.mixer.buses.push(Bus { id: bus, name: "Dialogue".into(), muted: false, mix: Default::default() });
    p.mixer.buses[0].mix.effects.push(Insert::new("bus-eq", "stock:Channel EQ", "Channel EQ"));
    let t = &mut p.tracks[0];
    t.mix.gain_db = -6.0;
    t.mix.pan = -0.5;
    t.mix.output = Some(bus);
    let mut comp = Insert::new("c1", "stock:ryolune Comp", "ryolune Comp");
    comp.params.insert(0, -20.0);
    t.mix.effects.push(comp);
    set_key(&mut t.mix.keyframes, "pan", Keyframe::new(0.0, -0.5, Default::default()));
    set_key(&mut t.mix.keyframes, "pan", Keyframe::new(4.0, 0.5, Default::default()));
    set_key(&mut t.mix.keyframes, &effect_key("c1", 0), Keyframe::new(1.0, -30.0, Default::default()));
    set_key(&mut t.mix.keyframes, &effect_key("c1", 0), Keyframe::new(3.0, -10.0, Default::default()));
    p.tracks[1].mix.sends.push(Sending { bus, level_db: -12.0, pre_fader: false });
    p.mixer.master.gain_db = -2.0;
    p.markers.push(kimchi_core::Marker { id: new_id(), time: 4.0, label: "Title".into(), color: "#ff8800".into() });
    let rate = 48_000;
    let mut sounds = SessionSounds { rate, ..Default::default() };
    for track in &p.tracks {
        for c in &track.clips {
            let frames: Vec<Frame> = (0..(c.duration * rate as f64) as usize).map(|i| [(i as f32 * 0.01).sin() * 0.3; 2]).collect();
            sounds.clips.insert(c.id, ClipSound { frames, start: c.start, gain_db: -3.0, fade_in: 0.5, fade_out: 0.25, curve: FadeCurve::EqualPower });
        }
    }
    // The second track was ducked by 10 dB from 1.5 s.
    sounds.ducking.insert(p.tracks[1].id, vec![(0.0, 0.0), (1.0, 0.0), (1.5, -10.0), (3.0, -10.0)]);

    let dir = tempfile::tempdir().unwrap();
    let path = write_session(&p, &sounds, &dir.path().join("Trailer")).unwrap();
    assert_eq!(path.extension().unwrap(), "ryolune");
    // ryolune's own reader and checks.
    let (s, library) = ryolune_engine::document::load(&path).unwrap();
    s.validate().unwrap();
    assert_eq!(s.name, "Trailer.ryolune");
    assert_eq!((s.transport.tempo, s.transport.time_signature.numerator), (120.0, 4));
    let names: Vec<(&str, &str)> = s.tracks.iter().map(|t| (t.name.as_str(), t.kind.as_str())).collect();
    assert_eq!(names, [("Dialogue", "bus"), ("Track 1", "audio"), ("Track 2", "audio")]);
    let one = &s.tracks[1];
    assert!((gain_to_db(fader_gain(one.volume) as f64) + 6.0).abs() < 0.01);
    assert_eq!(one.pan, -50.0);
    assert_eq!(one.output.as_deref(), Some(s.tracks[0].id.as_str()));
    let strip = &s.strips[&one.id];
    assert_eq!(strip.inserts[0].plugin, "stock:ryolune Comp");
    assert_eq!(strip.inserts[0].params[&0], -20.0);
    assert_eq!(s.strips[&s.tracks[2].id].sends[0].bus.as_deref(), Some(s.tracks[0].id.as_str()));
    assert_eq!(s.strips[&s.tracks[0].id].inserts[0].name, "Channel EQ");
    // The master: its fader, and kimchi's limiter as ryolune's.
    assert!((gain_to_db(fader_gain(s.master_volume) as f64) + 2.0).abs() < 0.01);
    assert!(s.strips["master"].inserts.iter().any(|i| i.plugin == "stock:Limiter"));
    // Clips at their bars (120 in 4/4: two seconds a bar), with their sound, fades and gain.
    let second = s.clips.iter().find(|c| c.track_id == one.id && c.start_bar > 0.0).unwrap();
    assert!((second.start_bar - 2.0).abs() < 1e-9 && (second.length_bars - 1.0).abs() < 1e-6);
    let ClipData::Audio { source_id, fade_in, gain_db, fade_curve, .. } = &second.data else { panic!("an audio clip") };
    assert_eq!((*fade_in, *gain_db, *fade_curve), (0.5, -3.0, ryolune_engine::model::FadeCurve::EqualPower));
    assert_eq!(library[source_id].frames.len(), 2 * rate as usize);
    // Automation: the pan lane, the compressor's threshold, and the ducking on the second fader.
    let lane = |target: &AutomationTarget| s.automation.iter().find(|l| l.target == *target);
    let pan = lane(&AutomationTarget::TrackPan { track_id: one.id.clone() }).unwrap();
    assert_eq!((pan.points[0].value, pan.points.last().unwrap().value, pan.points.last().unwrap().beat), (-50.0, 50.0, 8.0));
    let threshold = s.automation.iter().find(|l| matches!(&l.target, AutomationTarget::PluginParameter { parameter_id: 0, .. })).unwrap();
    assert_eq!((threshold.points[0].beat, threshold.points[0].value), (2.0, -30.0));
    let ducked = lane(&AutomationTarget::TrackVolume { track_id: s.tracks[2].id.clone() }).unwrap();
    let low = ducked.points.iter().map(|p| p.value).fold(1.0, f64::min);
    assert!((gain_to_db(fader_gain(low as f32) as f64) + 10.0).abs() < 0.05, "{low}");
    // A marker on its bar.
    assert_eq!((s.markers[0].name.as_str(), s.markers[0].bar), ("Title", 2.0));
    // And ryolune plays it.
    let wav = dir.path().join("bounce.wav");
    ryolune_engine::render::bounce(&s, &library, &wav, 48_000).unwrap();
    let samples: Vec<i32> = hound::WavReader::open(&wav).unwrap().into_samples::<i32>().map(|s| s.unwrap()).collect();
    assert!(samples.iter().any(|s| s.abs() > 100_000));
    assert!(fits(3600.0, 48_000).is_err() && fits(600.0, 48_000).is_ok());
}
