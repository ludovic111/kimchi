//! End-to-end tests against the real ffmpeg. Skipped when ffmpeg isn't installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use kimchi_core::{
    Asset, AssetOrigin, Clip, ClipContent, MediaKind, Project, ProjectSettings, TextStyle, Track, TrackKind, Transform,
};
use kimchi_media::export::{ExportFormat, ExportSettings, Overlays, Quality, export};
use kimchi_media::{MediaError, Tools, filmstrip, grab_frame, needs_proxy, probe, proxy, thumbnail, waveform};
use tokio_util::sync::CancellationToken;

const W: u32 = 320;
const H: u32 = 180;
const FPS: f64 = 25.0;

struct Fixtures {
    _dir: tempfile::TempDir,
    root: PathBuf,
    video: PathBuf,
    silent: PathBuf,
    webm: PathBuf,
    image: PathBuf,
    text: PathBuf,
    audio: PathBuf,
    rotated: PathBuf,
}

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

fn fixtures(tools: &Tools) -> Fixtures {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let p = |n: &str| root.join(n);
    let s = |p: &Path| p.to_str().unwrap().to_owned();
    let (video, silent, webm, image, text, audio, rotated) = (
        p("video.mp4"),
        p("silent.mp4"),
        p("clip.webm"),
        p("image.png"),
        p("text.png"),
        p("audio.wav"),
        p("rotated.mp4"),
    );
    let src = format!("testsrc2=s={W}x{H}:r={FPS}:d=2");
    ff(
        tools,
        &[
            "-f",
            "lavfi",
            "-i",
            &src,
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=2",
            "-pix_fmt",
            "yuv420p",
            "-shortest",
            &s(&video),
        ],
    );
    ff(tools, &["-f", "lavfi", "-i", &src, "-pix_fmt", "yuv420p", &s(&silent)]);
    ff(
        tools,
        &["-f", "lavfi", "-i", "testsrc2=s=160x90:r=10:d=1", "-c:v", "libvpx-vp9", "-deadline", "realtime", &s(&webm)],
    );
    ff(tools, &["-f", "lavfi", "-i", &format!("testsrc2=s={W}x{H}"), "-frames:v", "1", &s(&image)]);
    // A "rasterised text" overlay: transparent canvas with a white bar near the bottom.
    let text_src = format!(
        "color=c=white:s={W}x{H},format=rgba,geq=r=255:g=255:b=255:a='255*between(X,100,219)*between(Y,140,159)'"
    );
    ff(tools, &["-f", "lavfi", "-i", &text_src, "-frames:v", "1", &s(&text)]);
    ff(tools, &["-f", "lavfi", "-i", "sine=f=1000:d=2", "-ac", "2", &s(&audio)]);
    ff(tools, &["-display_rotation", "90", "-i", &s(&silent), "-c", "copy", &s(&rotated)]);
    Fixtures { _dir: dir, root, video, silent, webm, image, text, audio, rotated }
}

/// (duration, width, height, has audio) of the first video/audio streams.
async fn shape(tools: &Tools, path: &Path) -> (f64, Option<u32>, Option<u32>, bool) {
    let p = probe(tools, path).await.unwrap();
    (p.meta.duration.unwrap_or(0.0), p.meta.width, p.meta.height, p.meta.has_audio)
}

/// RGB of the pixel at (x, y) of the frame at `t`.
fn pixel(tools: &Tools, path: &Path, t: f64, x: u32, y: u32) -> [u8; 3] {
    let out = Command::new(&tools.ffmpeg)
        .args(["-v", "error", "-ss", &t.to_string(), "-i", path.to_str().unwrap(), "-frames:v", "1"])
        .args(["-vf", &format!("format=rgb24,crop=1:1:{x}:{y}"), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .unwrap();
    assert!(out.status.success() && out.stdout.len() == 3, "{}", String::from_utf8_lossy(&out.stderr));
    [out.stdout[0], out.stdout[1], out.stdout[2]]
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

#[tokio::test]
async fn probes_and_previews() {
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);

    let v = probe(&tools, &fx.video).await.unwrap();
    assert_eq!(v.kind, MediaKind::Video);
    assert_eq!((v.meta.width, v.meta.height), (Some(W), Some(H)));
    assert!((v.meta.duration.unwrap() - 2.0).abs() < 0.1, "{:?}", v.meta);
    assert_eq!(v.meta.fps, Some(FPS));
    assert!(v.meta.has_video && v.meta.has_audio && v.meta.size_bytes > 0);
    assert_eq!(v.meta.video_codec.as_deref(), Some("h264"));
    assert!(!needs_proxy(&v.meta, &fx.video));

    let img = probe(&tools, &fx.image).await.unwrap();
    assert_eq!(img.kind, MediaKind::Image);
    assert_eq!((img.meta.width, img.meta.duration), (Some(W), None));

    let a = probe(&tools, &fx.audio).await.unwrap();
    assert_eq!(a.kind, MediaKind::Audio);
    assert!(!a.meta.has_video && a.meta.has_audio);
    assert!(!needs_proxy(&a.meta, &fx.audio));

    let rot = probe(&tools, &fx.rotated).await.unwrap();
    assert_eq!((rot.meta.width, rot.meta.height), (Some(H), Some(W)));

    let webm = probe(&tools, &fx.webm).await.unwrap();
    assert_eq!(webm.meta.video_codec.as_deref(), Some("vp9"));
    assert_eq!(needs_proxy(&webm.meta, &fx.webm), cfg!(target_os = "macos"));

    let not_media = fx.root.join("notes.txt");
    std::fs::write(&not_media, "hello").unwrap();
    assert!(matches!(probe(&tools, &not_media).await, Err(MediaError::Unsupported(_))));

    // Thumbnails.
    let thumb = fx.root.join("thumb.jpg");
    assert!(thumbnail(&tools, &fx.video, MediaKind::Video, &thumb, 160).await.unwrap());
    assert_eq!(shape(&tools, &thumb).await.1, Some(160));
    assert!(thumbnail(&tools, &fx.image, MediaKind::Image, &thumb, 1000).await.unwrap());
    assert_eq!(shape(&tools, &thumb).await.1, Some(W), "never upscales");
    assert!(!thumbnail(&tools, &fx.audio, MediaKind::Audio, &thumb, 160).await.unwrap());

    // Filmstrip: 2 s at one frame per 0.5 s.
    let strip_path = fx.root.join("strip.jpg");
    let strip = filmstrip(&tools, &fx.video, 2.0, &strip_path, 48).await.unwrap();
    assert_eq!((strip.frames, strip.frame_height, strip.interval), (4, 48, 0.5));
    assert_eq!(strip.frame_width, 84); // 85.3 rounded down to even
    let (_, sw, sh, _) = shape(&tools, &strip_path).await;
    assert_eq!((sw, sh), (Some(strip.frame_width * strip.frames), Some(48)));

    // Waveform: a steady sine is loud everywhere.
    let wf_path = fx.root.join("wave.f32");
    let wf = waveform(&tools, &fx.video, &wf_path, 50).await.unwrap();
    assert_eq!(wf.peaks_per_second, 50);
    let peaks: Vec<f32> =
        std::fs::read(&wf_path).unwrap().as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    assert!((99..=102).contains(&peaks.len()), "{}", peaks.len());
    assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)));
    assert!(peaks[10..90].iter().all(|p| *p > 0.8));
    assert!(waveform(&tools, &fx.silent, &wf_path, 50).await.is_err());

    // Proxies.
    let px = fx.root.join("proxy.mp4");
    proxy(&tools, &fx.webm, &px).await.unwrap();
    let p = probe(&tools, &px).await.unwrap();
    assert_eq!((p.meta.video_codec.as_deref(), p.meta.width), (Some("h264"), Some(160)));
    let apx = fx.root.join("audio-proxy.mp4");
    proxy(&tools, &fx.audio, &apx).await.unwrap();
    let p = probe(&tools, &apx).await.unwrap();
    assert_eq!((p.kind, p.meta.audio_codec.as_deref()), (MediaKind::Audio, Some("aac")));

    // Frame grabs, including past the end.
    let frame = fx.root.join("frame.png");
    grab_frame(&tools, &fx.video, 1.0, &frame).await.unwrap();
    assert_eq!(shape(&tools, &frame).await.1, Some(W));
    grab_frame(&tools, &fx.video, 60.0, &frame).await.unwrap();
    grab_frame(&tools, &fx.image, 0.0, &frame).await.unwrap();
}

fn asset(kind: MediaKind, path: &Path, meta: kimchi_core::MediaMeta) -> Asset {
    Asset {
        id: kimchi_core::new_id(),
        name: path.file_name().unwrap().to_string_lossy().into(),
        kind,
        path: path.to_string_lossy().into(),
        meta,
        origin: AssetOrigin::Imported,
        created_at: Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
    }
}

const BG: [u8; 3] = [0x20, 0x30, 0x40];
const SOLID: [u8; 3] = [0xe0, 0x20, 0x20];

/// Two video tracks, a rotated half-transparent image, a solid, a text overlay,
/// a 2x clip and an audio clip with fades. 2.5 s long.
async fn sample_project(tools: &Tools, fx: &Fixtures) -> (Project, Overlays) {
    let video = asset(MediaKind::Video, &fx.video, probe(tools, &fx.video).await.unwrap().meta);
    let image = asset(MediaKind::Image, &fx.image, probe(tools, &fx.image).await.unwrap().meta);
    let audio = asset(MediaKind::Audio, &fx.audio, probe(tools, &fx.audio).await.unwrap().meta);
    let media = |a: &Asset, start, duration| Clip::new(&a.name, start, duration, ClipContent::Media { asset_id: a.id });

    let text = Clip { fade_in: 0.5, ..Clip::new("title", 0.0, 2.0, ClipContent::Text { style: TextStyle::default() }) };
    let mut overlay = media(&image, 0.5, 1.5);
    overlay.transform = Transform { x: 60.0, y: -30.0, scale: 0.5, rotation: 15.0, opacity: 0.5, ..Default::default() };
    let fast = Clip { speed: 2.0, ..media(&video, 0.0, 1.0) };
    let solid = Clip::new("solid", 1.0, 1.0, ClipContent::Solid { color: "#e02020".into() });
    let base = media(&video, 0.0, 2.0);
    let music = Clip { fade_in: 0.3, fade_out: 0.3, volume: 0.8, ..media(&audio, 0.5, 2.0) };

    let mut project = Project::new(
        "it",
        ProjectSettings { width: W, height: H, fps: FPS, background: "#203040".into(), sample_rate: 48_000 },
    );
    let overlays = Overlays::from([(text.id, fx.text.clone())]);
    project.tracks = vec![
        Track { clips: vec![text], ..Track::new(TrackKind::Video, "Text") },
        Track { clips: vec![overlay], ..Track::new(TrackKind::Video, "Overlay") },
        Track { clips: vec![fast, solid], ..Track::new(TrackKind::Video, "Video 2") },
        Track { clips: vec![base], ..Track::new(TrackKind::Video, "Video 1") },
        Track { clips: vec![music], ..Track::new(TrackKind::Audio, "Music") },
    ];
    project.assets = vec![video, image, audio];
    (project, overlays)
}

fn settings(fx: &Fixtures, name: &str, format: ExportFormat) -> ExportSettings {
    ExportSettings {
        path: fx.root.join(name).to_string_lossy().into(),
        format,
        quality: Quality::Draft,
        width: None,
        height: None,
        fps: None,
        range: None,
    }
}

#[tokio::test]
async fn exports_a_project() {
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);
    let (project, overlays) = sample_project(&tools, &fx).await;
    let frame = 1.0 / FPS;

    for (name, format, has_video, has_audio, tolerance) in [
        ("out.mp4", ExportFormat::Mp4, true, true, frame + 0.03),
        ("out.webm", ExportFormat::Webm, true, true, frame + 0.03),
        ("out.gif", ExportFormat::Gif, true, false, 1.0 / 15.0 + 0.01),
        ("out.m4a", ExportFormat::Audio, false, true, 0.05),
    ] {
        let st = settings(&fx, name, format);
        let last = std::sync::Mutex::new(0.0f64);
        export(&tools, &project, &overlays, &st, |p| *last.lock().unwrap() = p, CancellationToken::new())
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(*last.lock().unwrap(), 1.0);
        let out = Path::new(&st.path);
        let p = probe(&tools, out).await.unwrap();
        let duration = p.meta.duration.unwrap();
        assert!((duration - 2.5).abs() <= tolerance, "{name}: {duration}");
        assert_eq!(p.meta.has_video, has_video, "{name}");
        assert_eq!(p.meta.has_audio, has_audio, "{name}");
        if has_video {
            assert_eq!((p.meta.width, p.meta.height), (Some(W), Some(H)), "{name}");
        }
        assert!(!fx.root.join(format!(".{name}.part")).exists());
    }

    let mp4 = fx.root.join("out.mp4");
    // After every clip: only the background.
    assert!(close(pixel(&tools, &mp4, 2.3, 5, 5), BG), "{:?}", pixel(&tools, &mp4, 2.3, 5, 5));
    // The solid covers the bottom video from 1 s to 2 s (the corner is clear of the rotated image).
    assert!(close(pixel(&tools, &mp4, 1.5, 5, 5), SOLID), "{:?}", pixel(&tools, &mp4, 1.5, 5, 5));
    // The text bar, fully faded in.
    assert!(close(pixel(&tools, &mp4, 1.5, 160, 150), [255, 255, 255]));
    // The music is audible between its fades.
    let wf = waveform(&tools, &fx.root.join("out.m4a"), &fx.root.join("out.f32"), 10).await.unwrap();
    let peaks: Vec<f32> =
        std::fs::read(&wf.path).unwrap().as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    assert!(peaks[12] > 0.5 && peaks[24] < peaks[12], "{peaks:?}");

    // A range renders just that window.
    let mut st = settings(&fx, "range.mp4", ExportFormat::Mp4);
    st.range = Some((1.25, 2.25));
    st.width = Some(160);
    export(&tools, &project, &overlays, &st, |_| {}, CancellationToken::new()).await.unwrap();
    let (duration, w, h, audio) = shape(&tools, Path::new(&st.path)).await;
    assert!((duration - 1.0).abs() <= frame + 0.03, "{duration}");
    assert_eq!((w, h, audio), (Some(160), Some(90), true));
    assert!(close(pixel(&tools, Path::new(&st.path), 0.25, 3, 3), SOLID));
}

/// Sparse 40 s timeline: late inputs wait on the overlay, long adelay, a range in the middle.
#[tokio::test]
async fn exports_a_sparse_timeline_and_cancels_midway() {
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);
    let (mut project, overlays) = sample_project(&tools, &fx).await;
    project.tracks[2].clips[1].start = 21.0; // the solid
    project.tracks[4].clips[0].start = 38.0; // the music, ends at 40 s
    let end = project.duration();
    assert_eq!(end, 40.0);

    let st = settings(&fx, "sparse.mp4", ExportFormat::Mp4);
    export(&tools, &project, &overlays, &st, |_| {}, CancellationToken::new()).await.unwrap();
    let out = Path::new(&st.path);
    let (duration, _, _, audio) = shape(&tools, out).await;
    assert!((duration - end).abs() <= 1.0 / FPS + 0.03 && audio, "{duration}");
    assert!(close(pixel(&tools, out, 10.0, 5, 5), BG));
    assert!(close(pixel(&tools, out, 21.5, 5, 5), SOLID));
    assert!(close(pixel(&tools, out, 30.0, 5, 5), BG));

    // Cancel once ffmpeg reports progress: the child is killed and nothing is left behind.
    let mut st = settings(&fx, "midway.mp4", ExportFormat::Webm);
    st.quality = Quality::High;
    let cancel = CancellationToken::new();
    let seen = std::sync::atomic::AtomicBool::new(false);
    let res = export(
        &tools,
        &project,
        &overlays,
        &st,
        |p| {
            if p > 0.0 {
                seen.store(true, std::sync::atomic::Ordering::Relaxed);
                cancel.cancel();
            }
        },
        cancel.clone(),
    )
    .await;
    match res {
        Err(MediaError::Cancelled) => assert!(seen.into_inner()),
        // A fast machine may finish before the first progress report.
        Ok(()) => assert!(Path::new(&st.path).exists()),
        Err(e) => panic!("{e}"),
    }
    assert!(!fx.root.join(".midway.webm.part").exists());
}

#[tokio::test]
async fn cancels_and_reports_errors() {
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);
    let (project, overlays) = sample_project(&tools, &fx).await;

    let cancel = CancellationToken::new();
    cancel.cancel();
    let st = settings(&fx, "cancelled.mp4", ExportFormat::Mp4);
    let res = export(&tools, &project, &overlays, &st, |_| {}, cancel).await;
    assert!(matches!(res, Err(MediaError::Cancelled)), "{res:?}");
    assert!(!Path::new(&st.path).exists());
    assert!(!fx.root.join(".cancelled.mp4.part").exists());

    // A corrupt source: the error carries ffmpeg's explanation, not just an exit code.
    let mut broken = project.clone();
    std::fs::write(&fx.silent, b"definitely not a video").unwrap();
    broken.assets[0].path = fx.silent.to_string_lossy().into();
    let st = settings(&fx, "broken.mp4", ExportFormat::Mp4);
    match export(&tools, &broken, &overlays, &st, |_| {}, CancellationToken::new()).await {
        Err(MediaError::Ffmpeg(msg)) => {
            eprintln!("ffmpeg error: {msg}");
            assert!(msg.contains("silent.mp4") && msg.contains("Invalid data"), "{msg}");
            assert!(!msg.contains("Conversion failed"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!Path::new(&st.path).exists());
}
