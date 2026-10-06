//! Motion clips rendered ahead: the file plays back like the scene drawn live, goes out of date
//! when the scene changes, and covers trimmed clips.

use std::sync::atomic::AtomicBool;

use kimchi_core::{Clip, ClipContent, Project, ProjectSettings, Scene, Track, TrackKind};
use kimchi_media::Tools;
use kimchi_media::render::{Renderer, cache};
use serde_json::json;

fn tools() -> Option<Tools> {
    Tools::locate().ok().or_else(|| {
        eprintln!("ffmpeg not found; skipping");
        None
    })
}

fn project() -> (Project, kimchi_core::Id) {
    let scene = Scene::from_json(&json!({"layers": [
        {"id": "box", "type": "rect", "width": 120, "height": 120, "fill": "#ff5a36", "keyframes": {"x": [[0, -200], [2, 200]]}}
    ]}))
    .unwrap();
    let clip = Clip::new("motion", 0.0, 2.0, ClipContent::Motion { scene, template: None });
    let id = clip.id;
    let mut p = Project::new("r", ProjectSettings { width: 640, height: 360, fps: 30.0, background: "#000000".into(), sample_rate: 48_000 });
    p.tracks = vec![Track { clips: vec![clip], ..Track::new(TrackKind::Video, "Motion") }];
    (p, id)
}

fn red_x(p: &kimchi_media::tiny_skia::Pixmap) -> Option<u32> {
    (0..p.width()).find(|&x| p.pixel(x, 180).is_some_and(|c| c.red() > 200 && c.green() < 150))
}

#[tokio::test]
async fn cache_keys_follow_expression_duration_and_shutter_timing() {
    let (p,id)=project();let original=p.clip(id).unwrap();let base=cache::key(&p,original).unwrap();
    let mut changed=original.clone();changed.start=10.;
    assert_eq!(cache::key(&p,&changed).unwrap(),base,"moving a clip keeps its scene render reusable");
    changed.duration=4.;
    assert_ne!(cache::key(&p,&changed).unwrap(),base,"duration expressions must refresh after trimming");
    changed.speed=0.5;
    assert_ne!(cache::key(&p,&changed).unwrap(),base,"the same source span at another speed changes the motion-blur shutter");
}

#[tokio::test]
async fn renders_ahead_and_plays_the_file() {
    let Some(tools) = tools() else { return };
    let (mut p, id) = project();
    let dir = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let r = cache::render(&tools, &p, id, dir.path(), &|_| {}, &cancel).await.unwrap();
    assert!(std::path::Path::new(&r.file).is_file());
    assert!(r.frames >= 60, "{} frames", r.frames);
    let live = Renderer::new(&tools, &p, 640, 360, 30.0).still(1.0).unwrap();
    p.tracks[0].clips[0].rendered = Some(r.clone());
    assert_eq!(cache::status(&p, p.clip(id).unwrap()), "rendered");
    let played = Renderer::new(&tools, &p, 640, 360, 30.0).still(1.0).unwrap();
    let (a, b) = (red_x(&live).unwrap(), red_x(&played).unwrap());
    assert!(a.abs_diff(b) <= 2, "the file shows the same frame: live {a}, rendered {b}");
    // Streaming playback reads the same file.
    let mut r2 = Renderer::new(&tools, &p, 640, 360, 30.0);
    let mut last = 0;
    for n in 0..30 {
        let f = r2.frame(n as f64 / 30.0).unwrap();
        last = red_x(&f).unwrap();
    }
    assert!(last > 100, "it moved: {last}");
    // A change to the scene makes it out of date: drawn live again.
    if let ClipContent::Motion { scene: Scene::Flat(s), .. } = &mut p.tracks[0].clips[0].content {
        s.layers[0].y = 50.0;
    }
    assert_eq!(cache::status(&p, p.clip(id).unwrap()), "outdated");
}

#[test]
fn a_clip_that_shows_more_than_its_render_is_out_of_date() {
    let (mut p, id) = project();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("render.mkv");
    std::fs::write(&file, b"frames").unwrap();
    let key = cache::key(&p, p.clip(id).unwrap()).unwrap();
    // What `cache::render` stores for this 2 s clip: a frame either side.
    let rendered = kimchi_core::Rendered { file: file.to_string_lossy().into_owned(), key, from: 0.0, fps: 30.0, frames: 62, width: 640, height: 360, engine: "standard".into() };
    p.tracks[0].clips[0].rendered = Some(rendered);
    assert_eq!(cache::status(&p, p.clip(id).unwrap()), "rendered");
    // Lengthened past the render.
    let mut longer = p.clone();
    longer.tracks[0].clips[0].duration = 3.0;
    assert_eq!(cache::status(&longer, longer.clip(id).unwrap()), "outdated");
    // Slipped: the same length, but later in the scene than the file goes.
    let mut slipped = p.clone();
    slipped.tracks[0].clips[0].in_point = 1.0;
    assert_eq!(cache::key(&slipped, slipped.clip(id).unwrap()), cache::key(&p, p.clip(id).unwrap()), "the scene's frames are still right");
    assert!(cache::is_current(&slipped, slipped.clip(id).unwrap()), "so the part the file has still plays from it");
    assert_eq!(cache::status(&slipped, slipped.clip(id).unwrap()), "outdated");
    // Moved along the timeline: still all there.
    p.tracks[0].clips[0].start = 5.0;
    assert_eq!(cache::status(&p, p.clip(id).unwrap()), "rendered");
}
