//! Titles and the live preview against the real ffmpeg. Skipped when ffmpeg isn't installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use chrono::Utc;
use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, MediaKind, Project, ProjectSettings, TextStyle, Track, TrackKind};
use kimchi_media::preview::{CHANNELS, Frame, PreviewStream, SAMPLE_RATE, render_frame};
use kimchi_media::text::measure;
use kimchi_media::tiny_skia::Pixmap;
use kimchi_media::{Tools, probe};

const W: u32 = 320;
const H: u32 = 180;
const FPS: f64 = 25.0;
const BG: [u8; 3] = [0x20, 0x30, 0x40];
const SOLID: [u8; 3] = [0xe0, 0x20, 0x20];
const BOX: [u8; 3] = [0x20, 0xd0, 0x40];

fn tools() -> Option<Tools> {
    match Tools::locate() {
        Ok(t) => Some(t),
        Err(_) => {
            eprintln!("ffmpeg not found; skipping integration test");
            None
        }
    }
}

fn ff(tools: &Tools, args: &[&str]) {
    let out = Command::new(&tools.ffmpeg).args(["-hide_banner", "-v", "error", "-y"]).args(args).output().unwrap();
    assert!(out.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

async fn asset(tools: &Tools, kind: MediaKind, path: &Path) -> Asset {
    Asset {
        id: kimchi_core::new_id(),
        name: path.file_name().unwrap().to_string_lossy().into(),
        kind,
        path: path.to_string_lossy().into(),
        meta: probe(tools, path).await.unwrap().meta,
        origin: AssetOrigin::Imported,
        created_at: Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
        beats: None,
    }
}

/// 2 s: a test-pattern video with a tone, a full-frame red solid from 1 s, and a text layer that is
/// only a green box (a space with a background) in the middle for the whole length.
async fn project(tools: &Tools, root: &Path) -> Project {
    let video = root.join("video.mp4");
    let src = format!("testsrc2=s={W}x{H}:r={FPS}:d=2");
    let args = ["-f", "lavfi", "-i", &src, "-f", "lavfi", "-i", "sine=f=440:d=2", "-pix_fmt", "yuv420p", "-shortest"];
    ff(tools, &[&args[..], &[video.to_str().unwrap()]].concat());
    let video = asset(tools, MediaKind::Video, &video).await;
    let style = TextStyle {
        content: " ".into(),
        font_family: "Manrope".into(),
        font_size: 40.0,
        background: Some("#20d040".into()),
        shadow: false,
        ..TextStyle::default()
    };
    let text = Clip::new("box", 0.0, 2.0, ClipContent::Text { style });
    let solid = Clip::new("solid", 1.0, 1.0, ClipContent::Solid { color: "#e02020".into() });
    let base = Clip::new("v", 0.0, 2.0, ClipContent::Media { asset_id: video.id });
    let mut p = Project::new("p", ProjectSettings { width: W, height: H, fps: FPS, background: "#203040".into(), sample_rate: 48_000 });
    p.tracks = vec![
        Track { clips: vec![text], ..Track::new(TrackKind::Video, "Text") },
        Track { clips: vec![solid], ..Track::new(TrackKind::Video, "Solid") },
        Track { clips: vec![base], ..Track::new(TrackKind::Video, "Video") },
    ];
    p.assets = vec![video];
    p
}

fn close(a: [u8; 4], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

#[tokio::test]
async fn draws_titles_without_files() {
    let Some(tools) = tools() else { return };
    let mut p = Project::new("t", ProjectSettings { width: W, height: H, ..Default::default() });
    let style = TextStyle { content: "Hi".into(), background: Some("#20d040".into()), font_size: 40.0, shadow: false, ..TextStyle::default() };
    p.tracks[0].clips.push(Clip::new("t", 0.0, 1.0, ClipContent::Text { style: style.clone() }));
    let f = render_frame(&tools, &p, 0.5, W, H).await.unwrap();
    assert!(close(f.pixel(0, 0), [0, 0, 0]));
    // The box's left end, clear of the glyphs, is the background colour.
    let left = (measure(&style).width / 2.0 + 7.0) as u32;
    assert!(close(f.pixel(W / 2 - left, H / 2), [0x20, 0xd0, 0x40]), "{:?}", f.pixel(W / 2 - left, H / 2));
    let _ = Pixmap::new(1, 1);
}

#[tokio::test]
async fn renders_preview_frames() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let p = project(&tools, dir.path()).await;

    let started = Instant::now();
    let f = render_frame(&tools, &p, 0.5, 160, 90).await.unwrap();
    eprintln!("render_frame 160x90: {:?}", started.elapsed());
    assert_eq!((f.width, f.height, f.rgba.len()), (160, 90, 160 * 90 * 4));
    // The test pattern shows at the corner, the text box in the middle.
    assert!(!close(f.pixel(4, 4), BG) && !close(f.pixel(4, 4), SOLID), "{:?}", f.pixel(4, 4));
    assert!(close(f.pixel(80, 45), BOX), "{:?}", f.pixel(80, 45));
    // From 1 s the solid covers the video; the text stays on top.
    let f = render_frame(&tools, &p, 1.5, 160, 90).await.unwrap();
    assert!(close(f.pixel(4, 4), SOLID) && close(f.pixel(80, 45), BOX), "{:?}", f.pixel(4, 4));
    // The last frame still shows the clips; past the end only the background, without ffmpeg.
    let f = render_frame(&tools, &p, 1.99, 160, 90).await.unwrap();
    assert!(close(f.pixel(4, 4), SOLID));
    assert_eq!(render_frame(&tools, &p, 2.0, 160, 90).await.unwrap(), Frame::solid(160, 90, "#203040"));
    let empty = Project::new("e", p.settings.clone());
    assert!(close(render_frame(&tools, &empty, 0.0, 160, 90).await.unwrap().pixel(0, 0), BG));

    // Missing media is left out of the preview instead of failing it; a proxy is preferred.
    let mut gone = p.clone();
    gone.assets[0].path = dir.path().join("gone.mp4").to_string_lossy().into();
    let f = render_frame(&tools, &gone, 0.5, 160, 90).await.unwrap();
    assert!(close(f.pixel(4, 4), BG) && close(f.pixel(80, 45), BOX));
    let proxy = dir.path().join("proxy.mp4");
    ff(&tools, &["-f", "lavfi", "-i", &format!("color=c=0x2040e0:s={W}x{H}:r={FPS}:d=2"), "-pix_fmt", "yuv420p", proxy.to_str().unwrap()]);
    gone.assets[0].proxy = Some(proxy.to_string_lossy().into());
    let f = render_frame(&tools, &gone, 0.5, 160, 90).await.unwrap();
    assert!(close(f.pixel(4, 4), [0x20, 0x40, 0xe0]), "{:?}", f.pixel(4, 4));
}

#[tokio::test]
async fn streams_frames_and_sound() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let p = project(&tools, dir.path()).await;

    let started = Instant::now();
    let mut stream = PreviewStream::start(&tools, &p, 0.0, 160, 90, FPS).await.unwrap();
    assert_eq!((stream.from(), stream.size(), stream.duration()), (0.0, (160, 90), 2.0));
    let mut audio = stream.audio().expect("the video has sound");
    assert!(stream.audio().is_none());
    let mut frames: Vec<(f64, Frame)> = vec![];
    while let Some(f) = stream.next_frame().await.unwrap() {
        frames.push(f);
    }
    eprintln!("stream 160x90, 2 s: {:?}", started.elapsed());
    assert_eq!(frames.len(), 50);
    for (i, (pts, _)) in frames.iter().enumerate() {
        assert!((pts - i as f64 / FPS).abs() < 1e-9);
    }
    assert!(!close(frames[10].1.pixel(4, 4), SOLID) && close(frames[37].1.pixel(4, 4), SOLID));
    assert!(close(frames[49].1.pixel(80, 45), BOX));

    let mut samples = vec![];
    let mut next_start = 0.0;
    while let Some(chunk) = audio.recv().await {
        assert!((chunk.start - next_start).abs() < 1e-9);
        next_start += chunk.samples.len() as f64 / CHANNELS as f64 / SAMPLE_RATE as f64;
        samples.extend(chunk.samples);
    }
    assert_eq!(samples.len(), 2 * SAMPLE_RATE as usize * CHANNELS as usize);
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    // ffmpeg's sine is at 1/8 amplitude, a bit less after AAC.
    assert!(peak > 0.05 && peak <= 0.2, "{peak}");

    // From the middle: frames start there; muted tracks send silence so they can be
    // unmuted without restarting playback; past the end, nothing.
    let mut silent = p.clone();
    silent.tracks.iter_mut().for_each(|t| t.muted = true);
    let mut stream = PreviewStream::start(&tools, &silent, 1.01, 160, 90, FPS).await.unwrap();
    let mut audio = stream.audio().expect("muted sources keep the live mixer running");
    let (pts, _) = stream.next_frame().await.unwrap().unwrap();
    assert_eq!(pts, 1.0);
    let mut n = 1;
    while stream.next_frame().await.unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 25);
    let mut silent_samples = 0;
    while let Some(chunk) = audio.recv().await {
        assert!(chunk.samples.iter().all(|s| s.abs() < 1e-6));
        silent_samples += chunk.samples.len();
    }
    assert_eq!(silent_samples, SAMPLE_RATE as usize * CHANNELS as usize);
    let mut done = PreviewStream::start(&tools, &p, 5.0, 160, 90, FPS).await.unwrap();
    assert!(done.next_frame().await.unwrap().is_none() && done.audio().is_none());

    // Dropping a stream midway stops it.
    let mut stream = PreviewStream::start(&tools, &p, 0.0, 160, 90, FPS).await.unwrap();
    stream.next_frame().await.unwrap().unwrap();
    drop(stream);
}

/// Throughput on a heavier project (1080p sources, three layers, text) at preview size.
/// `cargo test -p kimchi-media --test preview -- --ignored --nocapture`
#[tokio::test]
#[ignore = "benchmark"]
async fn preview_speed() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = |n: &str| -> PathBuf { dir.path().join(n) };
    let (a, b) = (path("a.mp4"), path("b.mp4"));
    for (out, src) in [(&a, "testsrc2"), (&b, "rgbtestsrc")] {
        let src = format!("{src}=s=1920x1080:r=30:d=12");
        let enc = ["-c:v", "libx264", "-preset", "veryfast", "-g", "60", "-pix_fmt", "yuv420p", "-shortest", "-c:a", "aac"];
        ff(&tools, &[&["-f", "lavfi", "-i", &src, "-f", "lavfi", "-i", "sine=f=330:d=12"][..], &enc[..], &[out.to_str().unwrap()]].concat());
    }
    let (a, b) = (asset(&tools, MediaKind::Video, &a).await, asset(&tools, MediaKind::Video, &b).await);
    let mut pip = Clip::new("pip", 0.0, 12.0, ClipContent::Media { asset_id: b.id });
    pip.transform.scale = 0.4;
    pip.transform.x = 500.0;
    pip.transform.rotation = 5.0;
    let title = Clip::new("title", 0.0, 12.0, ClipContent::Text { style: TextStyle { content: "kimchi preview".into(), ..TextStyle::default() } });
    let mut p = Project::new("bench", ProjectSettings { width: 1920, height: 1080, fps: 30.0, ..Default::default() });
    p.tracks = vec![
        Track { clips: vec![title], ..Track::new(TrackKind::Video, "Text") },
        Track { clips: vec![pip], ..Track::new(TrackKind::Video, "PiP") },
        Track { clips: vec![Clip::new("a", 0.0, 12.0, ClipContent::Media { asset_id: a.id })], ..Track::new(TrackKind::Video, "A") },
    ];
    p.assets = vec![a, b];

    let mut times = vec![];
    for t in [0.5, 3.3, 6.1, 9.7, 11.2, 4.4, 7.9, 2.0] {
        let started = Instant::now();
        render_frame(&tools, &p, t, 640, 360).await.unwrap();
        times.push(started.elapsed());
    }
    times.sort();
    eprintln!("render_frame 640x360: min {:?} median {:?} max {:?}", times[0], times[times.len() / 2], times[times.len() - 1]);

    let started = Instant::now();
    let mut stream = PreviewStream::start(&tools, &p, 0.0, 640, 360, 30.0).await.unwrap();
    let mut audio = stream.audio().unwrap();
    let drain = tokio::spawn(async move { while audio.recv().await.is_some() {} });
    let mut first = None;
    let mut n = 0;
    while stream.next_frame().await.unwrap().is_some() {
        first.get_or_insert(started.elapsed());
        n += 1;
    }
    drain.await.unwrap();
    let total = started.elapsed();
    eprintln!(
        "stream 640x360@30, 12 s: {n} frames in {total:?} ({:.1}x real time), first frame after {:?}",
        12.0 / total.as_secs_f64(),
        first.unwrap()
    );
}
