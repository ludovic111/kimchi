//! End-to-end tests against the real ffmpeg. Skipped when ffmpeg isn't installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use kimchi_core::{
    Asset, AssetOrigin, Clip, ClipContent, MediaKind, Project, ProjectSettings, TextStyle, Track, TrackKind, Transform,
};
use kimchi_media::export::{ExportFormat, ExportSettings, Quality, export};
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
    let (video, silent, webm, image, audio, rotated) = (
        p("video.mp4"),
        p("silent.mp4"),
        p("clip.webm"),
        p("image.png"),
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
    ff(tools, &["-f", "lavfi", "-i", "sine=f=1000:d=2", "-ac", "2", &s(&audio)]);
    ff(tools, &["-display_rotation", "90", "-i", &s(&silent), "-c", "copy", &s(&rotated)]);
    Fixtures { _dir: dir, root, video, silent, webm, image, audio, rotated }
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
async fn sample_project(tools: &Tools, fx: &Fixtures) -> Project {
    let video = asset(MediaKind::Video, &fx.video, probe(tools, &fx.video).await.unwrap().meta);
    let image = asset(MediaKind::Image, &fx.image, probe(tools, &fx.image).await.unwrap().meta);
    let audio = asset(MediaKind::Audio, &fx.audio, probe(tools, &fx.audio).await.unwrap().meta);
    let media = |a: &Asset, start, duration| Clip::new(&a.name, start, duration, ClipContent::Media { asset_id: a.id });

    // A "title" that is only a white box (a space on a background) near the bottom.
    let bar = TextStyle { content: " ".into(), font_size: 40.0, background: Some("#ffffff".into()), shadow: false, ..TextStyle::default() };
    let mut text = Clip { fade_in: 0.5, ..Clip::new("title", 0.0, 2.0, ClipContent::Text { style: bar }) };
    text.transform.y = 60.0;
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
    project.tracks = vec![
        Track { clips: vec![text], ..Track::new(TrackKind::Video, "Text") },
        Track { clips: vec![overlay], ..Track::new(TrackKind::Video, "Overlay") },
        Track { clips: vec![fast, solid], ..Track::new(TrackKind::Video, "Video 2") },
        Track { clips: vec![base], ..Track::new(TrackKind::Video, "Video 1") },
        Track { clips: vec![music], ..Track::new(TrackKind::Audio, "Music") },
    ];
    project.assets = vec![video, image, audio];
    project
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
        encoder: Default::default(),
    }
}

#[tokio::test]
async fn exports_a_project() {
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);
    let project = sample_project(&tools, &fx).await;
    let frame = 1.0 / FPS;

    for (name, format, has_video, has_audio, tolerance) in [
        ("out.mp4", ExportFormat::Mp4, true, true, frame + 0.03),
        ("out.webm", ExportFormat::Webm, true, true, frame + 0.03),
        ("out.gif", ExportFormat::Gif, true, false, 1.0 / 15.0 + 0.01),
        ("out.m4a", ExportFormat::Audio, false, true, 0.05),
    ] {
        let st = settings(&fx, name, format);
        let last = std::sync::Mutex::new(0.0f64);
        export(&tools, &project, &st, |p| *last.lock().unwrap() = p, CancellationToken::new())
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
    export(&tools, &project, &st, |_| {}, CancellationToken::new()).await.unwrap();
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
    let mut project = sample_project(&tools, &fx).await;
    project.tracks[2].clips[1].start = 21.0; // the solid
    project.tracks[4].clips[0].start = 38.0; // the music, ends at 40 s
    let end = project.duration();
    assert_eq!(end, 40.0);

    let st = settings(&fx, "sparse.mp4", ExportFormat::Mp4);
    export(&tools, &project, &st, |_| {}, CancellationToken::new()).await.unwrap();
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
        Ok(_) => assert!(Path::new(&st.path).exists()),
        Err(e) => panic!("{e}"),
    }
    assert!(!fx.root.join(".midway.webm.part").exists());
}

#[tokio::test]
async fn cancels_and_reports_errors() {
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);
    let project = sample_project(&tools, &fx).await;

    let cancel = CancellationToken::new();
    cancel.cancel();
    let st = settings(&fx, "cancelled.mp4", ExportFormat::Mp4);
    let res = export(&tools, &project, &st, |_| {}, cancel).await;
    assert!(matches!(res, Err(MediaError::Cancelled)), "{res:?}");
    assert!(!Path::new(&st.path).exists());
    assert!(!fx.root.join(".cancelled.mp4.part").exists());

    // A corrupt source: the error carries ffmpeg's explanation, not just an exit code.
    let mut broken = project.clone();
    std::fs::write(&fx.silent, b"definitely not a video").unwrap();
    broken.assets[0].path = fx.silent.to_string_lossy().into();
    let st = settings(&fx, "broken.mp4", ExportFormat::Mp4);
    match export(&tools, &broken, &st, |_| {}, CancellationToken::new()).await {
        Err(MediaError::Ffmpeg(msg)) => {
            eprintln!("ffmpeg error: {msg}");
            assert!(msg.contains("silent.mp4") && msg.contains("Invalid data"), "{msg}");
            assert!(!msg.contains("Conversion failed"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!Path::new(&st.path).exists());
}

#[tokio::test]
async fn exports_originals_and_rejects_missing_or_broken_pictures() {
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);
    let original = fx.root.join("original.mp4");
    let preview = fx.root.join("preview.mp4");
    for (path, color) in [(&original, "red"), (&preview, "blue")] {
        ff(&tools, &["-f", "lavfi", "-i", &format!("color=c={color}:s=64x64:r=10:d=0.3"), "-an", path.to_str().unwrap()]);
    }
    let mut a = asset(MediaKind::Video, &original, probe(&tools, &original).await.unwrap().meta);
    a.proxy = Some(preview.to_string_lossy().into());
    let mut p = Project::new("originals", ProjectSettings { width: 64, height: 64, fps: 10.0, ..Default::default() });
    p.tracks = vec![Track { clips: vec![Clip::new("picture", 0.0, 0.3, ClipContent::Media { asset_id: a.id })], ..Track::new(TrackKind::Video, "Video") }];
    p.assets = vec![a];
    let st = settings(&fx, "original-export.mp4", ExportFormat::Mp4);
    export(&tools, &p, &st, |_| {}, CancellationToken::new()).await.unwrap();
    let pixels = Command::new(&tools.ffmpeg).args(["-v", "error", "-i", &st.path, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).output().unwrap();
    assert!(pixels.status.success());
    assert!(pixels.stdout[0] > 200 && pixels.stdout[2] < 30, "export must use the red original, not the blue proxy");
    let frame = kimchi_media::preview::render_frame(&tools, &p, 0.0, 64, 64).await.unwrap();
    assert!(frame.pixel(32, 32)[2] > 200, "preview should still use the proxy");
    p.assets[0].path = fx.root.join("missing.mp4").to_string_lossy().into();
    let missing = settings(&fx, "missing-export.mp4", ExportFormat::Mp4);
    let err = export(&tools, &p, &missing, |_| {}, CancellationToken::new()).await.unwrap_err();
    assert!(err.to_string().contains("missing media"), "{err}");
    assert!(!Path::new(&missing.path).exists());
    let broken = fx.root.join("broken.mp4");
    std::fs::write(&broken, b"broken").unwrap();
    p.assets[0].path = broken.to_string_lossy().into();
    assert!(export(&tools, &p, &missing, |_| {}, CancellationToken::new()).await.is_err());
}

/// The same ffmpeg under another path, so hardware assumed for it stays out of the other tests.
#[cfg(unix)]
fn private_tools(tools: &Tools, dir: &Path) -> Tools {
    let (ffmpeg, ffprobe) = (dir.join("ffmpeg-alias"), dir.join("ffprobe-alias"));
    std::os::unix::fs::symlink(&tools.ffmpeg, &ffmpeg).unwrap();
    std::os::unix::fs::symlink(&tools.ffprobe, &ffprobe).unwrap();
    Tools { ffmpeg, ffprobe }
}

#[cfg(unix)]
#[tokio::test]
async fn falls_back_to_the_cpu_when_the_hardware_encoder_fails() {
    use kimchi_media::accel::{CANDIDATES, Verified};
    use kimchi_media::{EncoderChoice, Hardware};
    let Some(tools) = tools() else { return };
    let fx = fixtures(&tools);
    let project = sample_project(&tools, &fx).await;
    let tools = private_tools(&tools, &fx.root);
    // An NVIDIA encoder that "passed" detection: on a machine without one, the export fails on it.
    let nvenc = *CANDIDATES.iter().find(|c| c.name == "h264_nvenc").unwrap();
    Hardware::assume(&tools, Hardware { encoders: vec![Verified { encoder: nvenc, constant_quality: true }], vaapi_device: None })
        .await;

    let st = settings(&fx, "fallback.mp4", ExportFormat::Mp4);
    assert_eq!(kimchi_media::export::planned_encoder(&tools, &project, &st).await.unwrap().as_deref(), Some("h264_nvenc"));
    let done = export(&tools, &project, &st, |_| {}, CancellationToken::new()).await.unwrap();
    // Either this machine really has NVENC, or the export was redone on the CPU.
    assert_ne!(done.hardware, done.fell_back, "{done:?}");
    if done.fell_back {
        assert_eq!(done.encoder.as_deref(), Some("libx264"));
    }
    let (duration, w, _, audio) = shape(&tools, Path::new(&st.path)).await;
    assert!((duration - 2.5).abs() < 0.1 && w == Some(W) && audio);
    assert!(!fx.root.join(".fallback.mp4.part").exists());

    // Asked for the CPU: the GPU isn't touched.
    let mut st = settings(&fx, "cpu.mp4", ExportFormat::Mp4);
    st.encoder = EncoderChoice::Software;
    let done = export(&tools, &project, &st, |_| {}, CancellationToken::new()).await.unwrap();
    assert_eq!((done.encoder.as_deref(), done.hardware, done.fell_back), (Some("libx264"), false, false));

    // Asked for hardware where there is none for the format: a clear error, nothing written.
    let mut st = settings(&fx, "gpu.webm", ExportFormat::Webm);
    st.encoder = EncoderChoice::Hardware;
    match export(&tools, &project, &st, |_| {}, CancellationToken::new()).await {
        Err(MediaError::Unsupported(msg)) => assert!(msg.contains("no hardware VP9/AV1 encoder"), "{msg}"),
        other => panic!("{other:?}"),
    }
    assert!(!Path::new(&st.path).exists());
}

#[tokio::test]
async fn detects_hardware_encoders() {
    let Some(tools) = tools() else { return };
    let started = std::time::Instant::now();
    let (hw, formats) = kimchi_media::export::encoders(&tools).await.unwrap();
    eprintln!("hardware here: {hw:?} in {:?}", started.elapsed());
    assert!(started.elapsed().as_secs() < 30);
    let mp4 = formats.iter().find(|f| f.format == ExportFormat::Mp4).unwrap();
    assert!(mp4.software.is_some() && mp4.auto.is_some());
    assert_eq!(mp4.hardware.is_some(), hw.best(kimchi_media::accel::Codec::H264).is_some());
    assert_eq!(mp4.auto, mp4.hardware.clone().or(mp4.software.clone()));
}

#[cfg(unix)]
#[tokio::test]
async fn retries_failed_hardware_decoding_without_driver_libraries() {
    use std::os::unix::fs::PermissionsExt;
    let Some(real) = tools() else { return };
    let fx = fixtures(&real);
    let wrapper = fx.root.join("ffmpeg-wrapper");
    let attempts = fx.root.join("decode-attempts");
    // A driver loader can abort ffmpeg before its own automatic software fallback runs.
    // Force that failure, while forwarding software runs to the real bundled ffmpeg.
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
    std::fs::write(&wrapper, format!("#!/bin/sh\nfor arg in \"$@\"; do\n if [ \"$arg\" = '-hwaccel' ]; then\n  echo attempted >> {}\n  echo 'missing driver library' >&2\n  exit 1\n fi\ndone\nexec {} \"$@\"\n", quote(&attempts), quote(&real.ffmpeg))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let tools = Tools { ffmpeg: wrapper, ffprobe: real.ffprobe.clone() };
    kimchi_media::Hardware::assume(&tools, kimchi_media::Hardware::none()).await;
    // VP9 is heavy even at a small resolution. This exercises filmstrip's software retry.
    filmstrip(&tools, &fx.webm, 2.0, &fx.root.join("retry-strip.jpg"), 48).await.unwrap();
    proxy(&tools, &fx.webm, &fx.root.join("retry-proxy.mp4")).await.unwrap();
    let a = asset(MediaKind::Video, &fx.webm, probe(&tools, &fx.webm).await.unwrap().meta);
    let mut p = Project::new("decode", ProjectSettings { width: W, height: H, fps: FPS, ..Default::default() });
    p.tracks = vec![Track { clips: vec![Clip::new("VP9", 0.0, 2.0, ClipContent::Media { asset_id: a.id })], ..Track::new(TrackKind::Video, "Video") }];
    p.assets = vec![a];
    let caps = kimchi_media::Caps::new(9, ["libx264"]).with_hwaccels(["cuda", "videotoolbox"]);
    let mut r = kimchi_media::render::Renderer::for_export(&tools, &p, W, H, FPS).with_hardware_decoding(caps);
    let frame = r.frame(0.0).unwrap();
    assert!(frame.pixels().iter().any(|px| px.red() > 40));
    assert!(std::fs::read_to_string(attempts).unwrap().lines().count() >= 2, "filmstrip and compositor must try hardware then recover");
}
