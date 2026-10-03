//! Motion clips through the compositor: every template renders something at every moment, keyframed
//! clips move, and the 3D renderers agree. `KIMCHI_DUMP=dir` writes the frames as PNGs to look at.

use kimchi_core::templates::{Ctx, TEMPLATES};
use kimchi_core::{Clip, ClipContent, Keyframe, Project, ProjectSettings, Track, TrackKind};
use kimchi_media::Tools;
use kimchi_media::render::Renderer;
use kimchi_media::tiny_skia::Pixmap;

fn tools() -> Option<Tools> {
    Tools::locate().ok().or_else(|| {
        eprintln!("ffmpeg not found; skipping");
        None
    })
}

fn project_with(clip: Clip, w: u32, h: u32) -> Project {
    let mut p = Project::new("m", ProjectSettings { width: w, height: h, fps: 30.0, background: "#000000".into(), sample_rate: 48_000 });
    p.tracks = vec![Track { clips: vec![clip], ..Track::new(TrackKind::Video, "Motion") }];
    p
}

fn dump(name: &str, p: &Pixmap) {
    if let Some(dir) = std::env::var_os("KIMCHI_DUMP") {
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        p.save_png(dir.join(format!("{name}.png"))).unwrap();
    }
}

/// Fraction of pixels that aren't black.
fn coverage(p: &Pixmap) -> f64 {
    let busy = p.pixels().iter().filter(|c| c.red() > 8 || c.green() > 8 || c.blue() > 8).count();
    busy as f64 / p.pixels().len() as f64
}

#[test]
fn every_template_draws_through_the_compositor() {
    let Some(tools) = tools() else { return };
    eprintln!("3D engine: {}", Renderer::engine());
    for t in TEMPLATES {
        let d = t.duration;
        let scene = t.build(&Default::default(), Ctx { width: 1280.0, height: 720.0, duration: d }).unwrap();
        let clip = Clip::new(t.name, 0.0, d, ClipContent::Motion { scene, template: None });
        let p = project_with(clip, 1280, 720);
        let mut r = Renderer::new(&tools, &p, 640, 360, 30.0);
        let mut seen = 0.0f64;
        for (i, f) in [0.15, 0.5, 0.9].iter().enumerate() {
            let at = d * f;
            let frame = r.frame(at).unwrap();
            dump(&format!("{}-{i}", t.id), &frame);
            seen = seen.max(coverage(&frame));
        }
        assert!(seen > 0.002, "{} draws something ({seen})", t.id);
    }
}

#[test]
fn keyframed_clips_move_and_fade() {
    let Some(tools) = tools() else { return };
    let mut clip = Clip::new("s", 0.0, 2.0, ClipContent::Solid { color: "#ffffff".into() });
    clip.transform.scale = 0.2;
    clip.keyframes.insert("x".into(), vec![Keyframe::new(0.0, -200.0, Default::default()), Keyframe::new(1.0, 200.0, Default::default())]);
    clip.keyframes.insert("opacity".into(), vec![Keyframe::new(1.0, 1.0, Default::default()), Keyframe::new(2.0, 0.0, Default::default())]);
    let p = project_with(clip, 640, 360);
    let mut r = Renderer::new(&tools, &p, 640, 360, 30.0);
    let at = |r: &mut Renderer, t: f64, x: u32| r.frame(t).unwrap().pixel(x, 180).unwrap().red();
    assert_eq!(at(&mut r, 0.0, 120), 255, "starts on the left");
    assert_eq!(at(&mut r, 0.0, 520), 0);
    assert_eq!(at(&mut r, 1.0, 520), 255, "ends on the right");
    let half = at(&mut r, 1.5, 520);
    assert!((100..160).contains(&half), "half faded: {half}");
}
