//! The mixer on real files: ffmpeg decoding, exports, the preview's sound, speech, stems.

use std::path::{Path, PathBuf};

use kimchi_audio::Frame;
use kimchi_audio::mixer::{Mixer, Mode};
use kimchi_core::audio::{Bus, db_to_gain};
use kimchi_core::{Asset, Clip, ClipContent, Project, ProjectSettings, Track, TrackKind};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::export::{AudioFormat, AudioOptions, ExportFormat, ExportSettings, Quality, export};

fn tools() -> Option<Tools> {
    Tools::locate().ok()
}

fn ff(tools: &Tools, args: &[&str]) {
    let st = crate::process::blocking(&tools.ffmpeg).args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).status().unwrap();
    assert!(st.success(), "{args:?}");
}

async fn asset(tools: &Tools, path: &Path) -> Asset {
    let p = crate::probe(tools, path).await.unwrap();
    serde_json::from_value(serde_json::json!({
        "id": kimchi_core::new_id(), "name": path.file_name().unwrap().to_string_lossy(), "kind": "audio",
        "path": path, "meta": p.meta, "origin": { "type": "imported" }, "created_at": "2026-10-05T00:00:00Z",
    }))
    .unwrap()
}

fn project(assets: Vec<Asset>, tracks: Vec<Vec<Clip>>) -> Project {
    let mut p = Project::new("sound", ProjectSettings { width: 64, height: 64, fps: 10.0, ..Default::default() });
    p.tracks = tracks.into_iter().enumerate().map(|(i, clips)| Track { clips, ..Track::new(TrackKind::Audio, format!("Track {}", i + 1)) }).collect();
    p.assets = assets;
    p
}

fn clip(a: &Asset, start: f64, duration: f64) -> Clip {
    Clip::new(&a.name, start, duration, ClipContent::Media { asset_id: a.id })
}

fn rms(s: &[Frame]) -> f64 {
    (s.iter().map(|f| (f[0] as f64).powi(2)).sum::<f64>() / s.len().max(1) as f64).sqrt()
}

/// Raw f32le stereo samples of a sound file, as ffmpeg decodes it.
fn decode(tools: &Tools, path: &Path, rate: u32) -> Vec<Frame> {
    let out = crate::process::blocking(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-ac", "2", "-ar", &rate.to_string(), "-"])
        .output()
        .unwrap();
    out.stdout.as_chunks::<8>().0.iter().map(|b| [f32::from_le_bytes([b[0], b[1], b[2], b[3]]), f32::from_le_bytes([b[4], b[5], b[6], b[7]])]).collect()
}

/// A rising line from -0.5 to 0.5 over 4 s at 48 kHz (to tell forwards from backwards), a mono
/// tone and a 5.1 file with only the centre speaking.
fn fixtures(tools: &Tools, dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let ramp = dir.join("ramp.wav");
    ff(tools, &["-f", "lavfi", "-i", "aevalsrc=-0.5+t/4|-0.5+t/4:d=4:s=48000", "-c:a", "pcm_f32le", ramp.to_str().unwrap()]);
    let mono = dir.join("mono.flac");
    ff(tools, &["-f", "lavfi", "-i", "sine=f=500:d=3:r=44100,volume=4", "-ac", "1", mono.to_str().unwrap()]);
    let surround = dir.join("centre.wav");
    ff(tools, &["-f", "lavfi", "-i", "sine=f=300:d=3:r=48000,volume=4", "-af", "pan=5.1|FC=c0", surround.to_str().unwrap()]);
    (ramp, mono, surround)
}

#[tokio::test]
async fn ffmpeg_sources_play_forwards_backwards_faster_and_folded() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (ramp, mono, surround) = fixtures(&tools, dir.path());
    let (ramp, mono, surround) = (asset(&tools, &ramp).await, asset(&tools, &mono).await, asset(&tools, &surround).await);
    let mut p = project(vec![ramp.clone()], vec![vec![clip(&ramp, 0.0, 4.0)]]);
    p.mixer.master.limiter = false;
    let out = render(&tools, &p, &Range::default()).await.unwrap();
    assert_eq!(out.len(), 4 * 48_000);
    // Forwards, sample-exact from a seek in the middle too.
    assert!((out[96_000][0] - 0.0).abs() < 1e-3, "{:?}", out[96_000]);
    let part = render(&tools, &p, &Range { span: Some((2.0, 3.0)), ..Default::default() }).await.unwrap();
    assert!((part[0][0] - out[96_000][0]).abs() < 1e-4, "{:?} vs {:?}", part[0], out[96_000]);
    // Backwards (in chunks): it falls instead.
    p.tracks[0].clips[0].reverse = true;
    let back = render(&tools, &p, &Range::default()).await.unwrap();
    assert!((back[48_000][0] - 0.25).abs() < 2e-3 && (back[3 * 48_000][0] + 0.25).abs() < 2e-3, "{:?} {:?}", back[48_000], back[3 * 48_000]);
    // Twice as fast: the line is crossed in half the time.
    p.tracks[0].clips[0].reverse = false;
    p.tracks[0].clips[0].speed = 2.0;
    p.tracks[0].clips[0].duration = 2.0;
    let fast = render(&tools, &p, &Range::default()).await.unwrap();
    assert!((fast[48_000][0] - 0.0).abs() < 0.02, "{:?}", fast[48_000]);
    // A mono file plays at full level on both sides, the 5.1 centre on both at -3 dB (scaled).
    let p = project(vec![mono.clone(), surround.clone()], vec![vec![clip(&mono, 0.0, 2.0)], vec![clip(&surround, 0.0, 2.0)]]);
    let solo = |i: usize| {
        let mut q = p.clone();
        q.tracks[i].mix.solo = true;
        q.mixer.master.limiter = false;
        q
    };
    let m = render(&tools, &solo(0), &Range::default()).await.unwrap();
    let peak = |s: &[Frame], c: usize| s[24_000..72_000].iter().map(|f| f[c].abs()).fold(0.0f32, f32::max);
    assert!((peak(&m, 0) - 0.5).abs() < 0.02 && (peak(&m, 1) - 0.5).abs() < 0.02, "{} {}", peak(&m, 0), peak(&m, 1));
    let c = render(&tools, &solo(1), &Range::default()).await.unwrap();
    let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 / (1.0 + 2.0 * std::f32::consts::FRAC_1_SQRT_2);
    assert!((peak(&c, 0) - expect).abs() < 0.02 && (peak(&c, 1) - expect).abs() < 0.02, "{} vs {expect}", peak(&c, 0));
    // Its samples, for beat detection and waveforms.
    let s = asset_samples(&tools, &mono, 22_050).await.unwrap();
    assert!((s.len() as i64 - 3 * 22_050).abs() < 64, "{}", s.len());
}

#[tokio::test]
async fn exports_are_the_mixers_sound_with_loudness_and_formats() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let tone = dir.path().join("tone.wav");
    ff(&tools, &["-f", "lavfi", "-i", "sine=f=440:d=3:r=48000,volume=2", "-ac", "2", tone.to_str().unwrap()]);
    let a = asset(&tools, &tone).await;
    let mut p = project(vec![a.clone()], vec![vec![Clip { fade_in: 0.5, volume: 0.7, ..clip(&a, 0.5, 2.0) }]]);
    p.tracks[0].mix.pan = -0.4;
    p.mixer.master.limiter = false;
    let st = |name: &str, audio: AudioOptions| ExportSettings {
        path: dir.path().join(name).to_string_lossy().into(),
        format: ExportFormat::Audio,
        quality: Quality::Standard,
        width: None,
        height: None,
        fps: None,
        range: None,
        encoder: Default::default(),
        audio,
    };
    // 32-bit float WAV: exactly what the mixer makes.
    let wav = st("mix.wav", AudioOptions { format: Some(AudioFormat::Wav), bit_depth: Some(32), ..Default::default() });
    export(&tools, &p, &wav, |_| {}, CancellationToken::new()).await.unwrap();
    let file = decode(&tools, Path::new(&wav.path), 48_000);
    let mixed = render(&tools, &p, &Range::default()).await.unwrap();
    assert_eq!(file.len(), mixed.len());
    let diff = file.iter().zip(&mixed).map(|(a, b)| (a[0] - b[0]).abs().max((a[1] - b[1]).abs())).fold(0.0f32, f32::max);
    assert!(diff < 1e-6, "the export differs from the mix by {diff}");
    // (A mono sine at 1/4 of full scale, made stereo by ffmpeg at -3 dB, at volume 0.7.)
    assert!(rms(&file[60_000..100_000]) > 0.06, "{}", rms(&file[60_000..100_000]));

    // Brought to -16 LUFS, peaks held under -1 dBTP.
    p.mixer.master.limiter = true;
    p.mixer.master.loudness = Some(-16.0);
    let loud = st("loud.flac", AudioOptions { format: Some(AudioFormat::Flac), ..Default::default() });
    let seen = std::sync::Mutex::new(vec![]);
    export(&tools, &p, &loud, |x| seen.lock().unwrap().push(x), CancellationToken::new()).await.unwrap();
    let seen = seen.into_inner().unwrap();
    assert_eq!(*seen.last().unwrap(), 1.0);
    assert!(seen.windows(2).all(|w| w[1] >= w[0] - 1e-9), "progress goes forward: {seen:?}");
    let r = kimchi_audio::loudness::measure(&decode(&tools, Path::new(&loud.path), 48_000), 48_000);
    assert!((r.integrated + 16.0).abs() < 0.5, "{r:?}");
    assert!(r.true_peak <= -1.0 + 0.2, "{r:?}");

    // Other file types read back with sound of the right length.
    let caps = crate::Caps::detect(&tools).await.unwrap();
    for (name, format) in [("m.aiff", AudioFormat::Aiff), ("m.mp3", AudioFormat::Mp3), ("m.opus", AudioFormat::Opus), ("m.ogg", AudioFormat::Vorbis), ("m.m4a", AudioFormat::Aac)] {
        if format == AudioFormat::Mp3 && caps.pick(&["libmp3lame"]).is_none() {
            continue;
        }
        let s = st(name, AudioOptions { format: Some(format), sample_rate: Some(44_100), ..Default::default() });
        export(&tools, &p, &s, |_| {}, CancellationToken::new()).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        let probe = crate::probe(&tools, Path::new(&s.path)).await.unwrap();
        assert!(probe.meta.has_audio && (probe.meta.duration.unwrap() - 2.5).abs() < 0.1, "{name}: {:?}", probe.meta.duration);
    }

    // Stems: a folder with one file per track and bus.
    let mut p = project(vec![a.clone()], vec![vec![clip(&a, 0.0, 1.0)], vec![clip(&a, 0.0, 1.0)]]);
    let bus = kimchi_core::new_id();
    p.mixer.buses.push(Bus { id: bus, name: "Music bus".into(), muted: false, mix: Default::default() });
    p.tracks[1].mix.output = Some(bus);
    p.tracks[1].mix.gain_db = -6.0;
    let stems = st("stems", AudioOptions { format: Some(AudioFormat::Wav), stems: true, ..Default::default() });
    export(&tools, &p, &stems, |_| {}, CancellationToken::new()).await.unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&stems.path).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["01-Track 1.wav", "02-Track 2.wav", "03-Music bus.wav"]);
    let one = decode(&tools, &Path::new(&stems.path).join("01-Track 1.wav"), 48_000);
    let two = decode(&tools, &Path::new(&stems.path).join("02-Track 2.wav"), 48_000);
    let ratio = rms(&two[10_000..40_000]) / rms(&one[10_000..40_000]);
    assert!((ratio - db_to_gain(-6.0)).abs() < 0.01, "{ratio}");
    assert!(export(&tools, &p, &stems, |_| {}, CancellationToken::new()).await.is_err(), "an existing folder is never replaced");
}

#[tokio::test]
async fn the_preview_streams_the_mix_live_and_speech_hears_it() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let tone = dir.path().join("tone.wav");
    ff(&tools, &["-f", "lavfi", "-i", "sine=f=440:d=6:r=48000,volume=2", "-ac", "2", tone.to_str().unwrap()]);
    let a = asset(&tools, &tone).await;
    let mut p = project(vec![a.clone()], vec![vec![clip(&a, 0.0, 6.0)]]);
    p.mixer.master.limiter = false;
    let mut stream = crate::preview::PreviewStream::start(&tools, &p, 1.0, 64, 64, 10.0).await.unwrap();
    let mut rx = stream.audio().unwrap();
    let first = rx.recv().await.unwrap();
    assert_eq!(first.start, 1.0);
    let mut got: Vec<f32> = first.samples;
    // Read at about real time, as speakers would (decoders that aren't ready play silence).
    let pace = || tokio::time::sleep(std::time::Duration::from_millis(8));
    while got.len() < 2 * 2 * 48_000 {
        got.extend(rx.recv().await.unwrap().samples);
        pace().await;
    }
    let level = |s: &[f32]| (s.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
    let before = level(&got[2 * 48_000..]);
    assert!(before > 0.05, "{before}");
    // A fader move while playing: heard within a few chunks, without restarting.
    let mut q = p.clone();
    q.tracks[0].mix.gain_db = -20.0;
    stream.update(std::sync::Arc::new(q));
    let mut after = vec![];
    while after.len() < 2 * 2 * 48_000 {
        after.extend(rx.recv().await.unwrap().samples);
        pace().await;
    }
    let quieter = level(&after[2 * 48_000..]);
    assert!((quieter / before - db_to_gain(-20.0)).abs() < 0.02, "{quieter} vs {before}");
    assert!(stream.meters().latest().is_some());
    drop(stream);

    // Speech: 16 kHz mono of the same mix.
    let s = crate::speech_samples(&tools, &p, Some((1.0, 3.0))).await.unwrap();
    assert_eq!(s.len(), 2 * 16_000);
    assert!(s[8_000..].iter().any(|x| x.abs() > 0.1));
    // A scrub snippet.
    let snip = scrub(&tools, &p, 2.0, 0.2).await.unwrap();
    assert_eq!(snip.len(), (0.2 * 48_000.0) as usize);
    assert!(rms(&snip[2_000..8_000]) > 0.05);
    // A clip's own loudness, without its fader.
    let mut q = p.clone();
    q.tracks[0].mix.gain_db = -12.0;
    q.tracks[0].clips[0].volume = 0.25;
    let l = clip_loudness(&tools, &q, q.tracks[0].clips[0].id).await.unwrap();
    let whole = measure(&tools, &p, &Range::default()).await.unwrap();
    assert!((l.integrated - whole.integrated).abs() < 0.2, "{l:?} vs {whole:?}");
}

#[test]
fn offline_mixers_run_far_faster_than_real_time() {
    // 16 tracks of tones, 10 minutes, through the mixer alone (sources made in memory): how many
    // times real time it renders.
    let assets: Vec<Asset> = (0..16).map(|i| kimchi_audio::testing::asset(&format!("tone:{}:0.05", 110 * (i + 1)), 700.0)).collect();
    let tracks: Vec<Vec<(usize, f64, f64)>> = (0..16).map(|i| (0..20).map(|k| (i, k as f64 * 30.0, 30.0)).collect()).collect();
    let mut p = kimchi_audio::testing::project(assets, tracks);
    for (i, t) in p.tracks.iter_mut().enumerate() {
        t.mix.pan = (i as f64 / 8.0) - 1.0;
        t.mix.gain_db = -3.0;
    }
    let mut m = Mixer::new(std::sync::Arc::new(p), kimchi_audio::testing::ToneOpener::new(), 48_000, Mode::Offline).unwrap();
    let start = std::time::Instant::now();
    let cpu_before = cpu_seconds();
    let mut block = vec![[0.0f32; 2]; 4096];
    let frames = 600 * 48_000;
    let mut done = 0;
    while done < frames {
        m.render(&mut block);
        done += block.len();
    }
    let wall = start.elapsed().as_secs_f64();
    // On a busy machine the wall clock says more about the neighbours: count this thread's
    // own time where the system tells it.
    let cpu = cpu_seconds().zip(cpu_before).map(|(b, a)| b - a).filter(|c| *c > 0.0);
    let speed = 600.0 / cpu.unwrap_or(wall);
    eprintln!("16 tracks × 10 minutes mixed at {speed:.0}× real time ({wall:.1} s wall, {cpu:?} s of CPU)");
    assert!(speed > 20.0, "{speed:.1}× real time");
}

/// CPU time of this thread so far (Linux).
fn cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: f64 = fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0)
}
