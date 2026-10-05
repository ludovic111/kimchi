//! Renderer benchmarks: frames per second for representative projects.
//!
//! ```text
//! cargo run -p kimchi-media --release --example render_bench -- [names…] [--dump DIR] [--frames N]
//! ```
//!
//! Names (all of them when none is given): `cut` (a 1080p cut of two videos with a dissolve,
//! colour effects and a title), `blur2d` (a 2D template with big blurs), `glow2d` (a 2D template
//! with glows), `3d` (two 3D templates at 1080p, standard engine; `KIMCHI_GPU=0` for the CPU,
//! `KIMCHI_GPU=any` for the GPU path on a software adapter), `path` (the path tracer at
//! 960×540), `export` (a whole 20-second 1080p export), `preview` (the playback stream at
//! 1280×720) and `scrub` (single frames at random times, 1280×720).
//!
//! Media: `KIMCHI_BENCH_MEDIA=dir` with `city.mp4` and `fractal.mp4` (1080p), else two test
//! videos are made once with ffmpeg in the temp folder. `--dump DIR` saves one frame of each
//! benchmark as a PNG to look at.

use std::path::{Path, PathBuf};
use std::time::Instant;

use kimchi_core::templates::{Ctx, find};
use kimchi_core::{Asset, Clip, ClipContent, Effects, Project, ProjectSettings, TextStyle, Track, TrackKind};
use kimchi_media::preview::{PreviewStream, render_frame};
use kimchi_media::render::Renderer;
use kimchi_media::{Tools, probe};
use serde_json::json;

struct Opts {
    dump: Option<PathBuf>,
    /// Multiplies every benchmark's frame count.
    frames: f64,
}

#[tokio::main]
async fn main() {
    allow_profiler();
    let mut names = vec![];
    let mut opts = Opts { dump: None, frames: 1.0 };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dump" => opts.dump = args.next().map(PathBuf::from),
            "--frames" => opts.frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(1.0),
            _ => names.push(a),
        }
    }
    let all = ["cut", "blur2d", "glow2d", "3d", "path", "export", "preview", "scrub"];
    if names.is_empty() {
        names = all.iter().map(|s| s.to_string()).collect();
    }
    let tools = Tools::locate().expect("ffmpeg");
    if let Some(d) = &opts.dump {
        std::fs::create_dir_all(d).unwrap();
    }
    println!("3D engine: {}", Renderer::engine());
    let cut = cut_project(&tools).await;
    for name in &names {
        match name.as_str() {
            "cut" => frames("cut 1920x1080", &tools, &cut, 1920, 1080, 9.0, n(75, &opts), &opts),
            "blur2d" => template("blur2d liquidBackground 1920x1080", &tools, "liquidBackground", json!({}), 1920, 1080, n(40, &opts), &opts),
            "glow2d" => template("glow2d radialBurst 1920x1080", &tools, "radialBurst", json!({}), 1920, 1080, n(40, &opts), &opts),
            "3d" | "logo3d" | "product3d" => {
                if name != "product3d" {
                    template("3d logoExtrude 1920x1080", &tools, "logoExtrude", json!({}), 1920, 1080, n(20, &opts), &opts);
                }
                if name != "logo3d" {
                    template("3d productShot 1920x1080", &tools, "productShot", json!({"engine": "standard"}), 1920, 1080, n(20, &opts), &opts);
                }
            }
            "path" => template("path productShot 960x540", &tools, "productShot", json!({"engine": "path"}), 960, 540, n(1, &opts), &opts),
            "export" => export(&tools, &cut).await,
            "preview" => preview(&tools, &cut, n(150, &opts)).await,
            "scrub" => scrub(&tools, &cut, n(12, &opts), &opts).await,
            other => eprintln!("unknown benchmark {other}; known: {}", all.join(", ")),
        }
    }
}

/// `KIMCHI_BENCH_PTRACE=1` lets a debugger that isn't our parent attach (for sampling stacks
/// with gdb on Linux, where perf may not be allowed).
fn allow_profiler() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("KIMCHI_BENCH_PTRACE").is_some() {
        unsafe extern "C" {
            fn prctl(option: i32, arg: u64, ...) -> i32;
        }
        // PR_SET_PTRACER, PR_SET_PTRACER_ANY
        unsafe { prctl(0x5961_6d61, u64::MAX) };
    }
}

fn n(base: usize, o: &Opts) -> usize {
    ((base as f64 * o.frames).round() as usize).max(1)
}

fn report(name: &str, frames: usize, secs: f64) {
    println!("{name:<40} {:>8.2} frames/s  ({frames} frames in {secs:.2} s, {:.1} ms/frame)", frames as f64 / secs, secs * 1000.0 / frames as f64);
}

/// `count` frames in order from `from`, as an export renders them.
#[allow(clippy::too_many_arguments)]
fn frames(name: &str, tools: &Tools, p: &Project, w: u32, h: u32, from: f64, count: usize, o: &Opts) {
    let fps = p.settings.fps;
    let mut r = Renderer::for_export(tools, p, w, h, fps);
    // The first frame starts decoders and fills caches: not counted.
    let first = r.frame(from).expect("frame");
    dump(o, name, &first);
    let started = Instant::now();
    for k in 1..=count {
        r.frame(from + k as f64 / fps).expect("frame");
    }
    report(name, count, started.elapsed().as_secs_f64());
}

#[allow(clippy::too_many_arguments)]
fn template(name: &str, tools: &Tools, id: &str, values: serde_json::Value, w: u32, h: u32, count: usize, o: &Opts) {
    let t = find(id).expect("template");
    let d = t.duration;
    let values = values.as_object().cloned().unwrap_or_default();
    let scene = t.build(&values, Ctx { width: 1920.0, height: 1080.0, duration: d }).expect("scene");
    let clip = Clip::new(t.name, 0.0, d, ClipContent::Motion { scene, template: None });
    let mut p = Project::new("bench", ProjectSettings { width: 1920, height: 1080, fps: 30.0, background: "#000000".into(), sample_rate: 48_000 });
    p.tracks = vec![Track { clips: vec![clip], ..Track::new(TrackKind::Video, "Motion") }];
    // Frames spread over the middle of the template (where most things move).
    let from = d * 0.3;
    let step = (d * 0.5 / count as f64).min(1.0 / 30.0);
    let mut r = Renderer::for_export(tools, &p, w, h, 30.0);
    let first = r.frame(from).expect("frame");
    dump(o, name, &first);
    let started = Instant::now();
    for k in 1..=count {
        r.frame(from + k as f64 * step).expect("frame");
    }
    report(name, count, started.elapsed().as_secs_f64());
}

async fn export(tools: &Tools, p: &Project) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bench.mp4");
    let settings = serde_json::from_value(json!({
        "path": out.to_string_lossy(), "format": "mp4", "quality": "standard",
        "width": 1920, "height": 1080, "fps": null, "range": null, "encoder": "software"
    }))
    .expect("settings");
    let started = Instant::now();
    let done = kimchi_media::export::export(tools, p, &settings, |_| {}, Default::default()).await.expect("export");
    let secs = started.elapsed().as_secs_f64();
    let frames = (p.duration() * p.settings.fps).round() as usize;
    report(&format!("export 20 s 1920x1080 ({})", done.encoder.unwrap_or_default()), frames, secs);
}

async fn preview(tools: &Tools, p: &Project, count: usize) {
    let started = Instant::now();
    let mut stream = PreviewStream::start(tools, p, 7.0, 1280, 720, p.settings.fps).await.expect("stream");
    let drain = stream.audio().map(|mut a| tokio::spawn(async move { while a.recv().await.is_some() {} }));
    let mut first = None;
    let mut got = 0;
    while got < count {
        match stream.next_frame().await.expect("frame") {
            Some(_) => got += 1,
            None => break,
        }
        if first.is_none() {
            first = Some(Instant::now());
        }
    }
    let secs = first.map_or(0.0, |f| f.elapsed().as_secs_f64());
    drop(stream);
    if let Some(d) = drain {
        d.abort();
    }
    report("preview stream 1280x720", got.saturating_sub(1).max(1), secs);
    println!("{:<40} {:>8.0} ms", "  first frame after", (started.elapsed().as_secs_f64() - secs) * 1000.0);
}

async fn scrub(tools: &Tools, p: &Project, count: usize, o: &Opts) {
    // The same pseudo-random times every run.
    let mut x = 0x2545f491u32;
    let mut times = vec![];
    for _ in 0..count {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        times.push((x % 1000) as f64 / 1000.0 * (p.duration() - 0.1));
    }
    let mut each = vec![];
    for (i, t) in times.iter().enumerate() {
        let started = Instant::now();
        let f = render_frame(tools, p, *t, 1280, 720).await.expect("frame");
        each.push(started.elapsed().as_secs_f64());
        if i == 0
            && let Some(d) = &o.dump
        {
            let img = kimchi_media::tiny_skia::Pixmap::from_vec(f.rgba, kimchi_media::tiny_skia::IntSize::from_wh(f.width, f.height).unwrap()).unwrap();
            img.save_png(d.join("scrub.png")).unwrap();
        }
    }
    let total: f64 = each.iter().sum();
    each.sort_by(f64::total_cmp);
    report("scrub 1280x720 (single frames)", count, total);
    println!("{:<40} {:>8.0} ms median, {:.0} ms worst", "", each[each.len() / 2] * 1000.0, each[each.len() - 1] * 1000.0);
}

fn dump(o: &Opts, name: &str, p: &kimchi_media::tiny_skia::Pixmap) {
    if let Some(d) = &o.dump {
        let file: String = name.split_whitespace().next().unwrap_or("frame").to_string();
        let extra = name.split_whitespace().nth(1).filter(|s| !s.contains('x')).map(|s| format!("-{s}")).unwrap_or_default();
        p.save_png(d.join(format!("{file}{extra}.png"))).unwrap();
    }
}

/// The two source videos: the given ones, or test patterns made once.
fn media(tools: &Tools) -> (PathBuf, PathBuf) {
    if let Some(dir) = std::env::var_os("KIMCHI_BENCH_MEDIA") {
        let d = Path::new(&dir);
        return (d.join("city.mp4"), d.join("fractal.mp4"));
    }
    let dir = std::env::temp_dir().join("kimchi-bench-media");
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = vec![];
    for (name, src) in [("a.mp4", "testsrc2"), ("b.mp4", "gradients")] {
        let path = dir.join(name);
        if !path.is_file() {
            let status = std::process::Command::new(&tools.ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("{src}=s=1920x1080:r=25"), "-f", "lavfi", "-i", "sine=f=330"])
                .args(["-t", "12", "-c:v", "libx264", "-preset", "veryfast", "-g", "50", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
                .arg(&path)
                .status()
                .expect("ffmpeg");
            assert!(status.success(), "making {name}");
        }
        out.push(path);
    }
    (out[0].clone(), out[1].clone())
}

async fn asset(tools: &Tools, path: &Path) -> Asset {
    let meta = probe(tools, path).await.expect("probe").meta;
    serde_json::from_value(json!({
        "id": kimchi_core::new_id(), "name": path.file_name().unwrap().to_string_lossy(), "kind": "video",
        "path": path.to_string_lossy(), "meta": meta, "origin": {"type": "imported"}, "created_at": "2026-10-05T00:00:00Z"
    }))
    .unwrap_or_else(|e| panic!("asset: {e}"))
}

/// 20 seconds at 1080p25: two videos cut together with a one-second dissolve, the first graded
/// (contrast, saturation, warmth, vignette), and a title over both.
async fn cut_project(tools: &Tools) -> Project {
    let (a, b) = media(tools);
    let (a, b) = (asset(tools, &a).await, asset(tools, &b).await);
    let mut first = Clip::new("a", 0.0, 10.5, ClipContent::Media { asset_id: a.id });
    first.effects = Effects { contrast: 0.2, saturation: 0.3, temperature: 0.2, vignette: 0.5, ..Default::default() };
    let mut second = Clip::new("b", 10.5, 9.5, ClipContent::Media { asset_id: b.id });
    second.transition = Some(kimchi_core::transition::Transition::new(kimchi_core::transition::TransitionKind::Dissolve, 1.0));
    let style = TextStyle { content: "kimchi render bench".into(), font_size: 96.0, ..TextStyle::default() };
    let mut title = Clip::new("title", 0.0, 20.0, ClipContent::Text { style });
    title.transform.y = 360.0;
    let mut p = Project::new("bench", ProjectSettings { width: 1920, height: 1080, fps: 25.0, background: "#101010".into(), sample_rate: 48_000 });
    p.tracks = vec![
        Track { clips: vec![title], ..Track::new(TrackKind::Video, "Title") },
        Track { clips: vec![first, second], ..Track::new(TrackKind::Video, "Video") },
    ];
    p.assets = vec![a, b];
    p
}
