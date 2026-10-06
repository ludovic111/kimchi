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
fn space_expressions_use_clip_context_in_studio_exports_and_refining_views() {
    use kimchi_media::render::space::viewport::{ViewCamera,ViewOptions};
    let Some(tools)=tools() else {return};
    let scene=kimchi_core::Scene::from_json(&serde_json::json!({"type":"3d","background":"#000000",
        "camera":{"position":[0,0,8],"target":[0,0,0],"projection":"orthographic","orthoSize":6},
        "objects":[{"id":"panel","type":"mesh","vertices":[[-0.5,-0.5,0],[0.5,-0.5,0],[0.5,0.5,0],[-0.5,0.5,0]],"faces":[[0,1,2,3]],
            "expressions":{"position.x":"duration * 0.25","position.y":"fps / 60 - 1"},"material":{"color":"#ff0000","unlit":true}}]
    })).unwrap();
    let mut clip=Clip::new("space",0.,4.,ClipContent::Motion {scene,template:None});clip.speed=2.;let id=clip.id;
    let mut project=project_with(clip,160,90);project.settings.fps=60.;
    let view=ViewCamera {position:[0.,0.,8.],target:[0.;3],ortho:true,ortho_size:6.,..Default::default()};
    let mut renderer=Renderer::new(&tools,&project,160,90,60.);
    let check=|image:&Pixmap,label:&str| {
        let p=image.pixel(110,45).unwrap();
        assert!(p.red()>200 && p.green()<20 && p.blue()<20,"{label}: duration=8 and fps=60 place the red panel at [2,0,0], got {p:?}");
        assert!(image.pixel(80,45).unwrap().red()<20,"{label}: no panel at the default expression position");
    };
    check(&renderer.scene_view(id,0.,Some(&view),&ViewOptions::default(),None).unwrap(),"Studio");
    check(&renderer.still(0.).unwrap(),"timeline preview");
    check(&Renderer::for_export(&tools,&project,160,90,60.).still(0.).unwrap(),"export");
    let selected=renderer.scene_view(id,0.,Some(&view),&ViewOptions {edit:Some("panel".into()),edit_faces:vec![0],..Default::default()},None).unwrap();
    let p=selected.pixel(110,45).unwrap();
    assert!(p.green()>30 && p.blue()>5,"edit overlay follows the same evaluated panel: {p:?}");
    if let ClipContent::Motion {scene:kimchi_core::Scene::Space(s),..}=&mut project.tracks[0].clips[0].content {
        s.render.engine="path".into();s.render.samples=4.;s.render.denoise=false;
    }
    let mut renderer=Renderer::new(&tools,&project,160,90,60.);
    let mut progressive=renderer.refining_view(id,0.,Some(&view)).unwrap().unwrap();progressive.add(4);
    check(&progressive.picture(),"refining Studio view");
    let opts=ViewOptions {edit:Some("panel".into()),edit_faces:vec![0],through_camera:true,..Default::default()};
    // A foreign editor camera must not override an explicit scene-camera view.
    let foreign=ViewCamera {position:[100.,0.,8.],target:[100.,0.,0.],..view};
    let mut editing=renderer.refining_studio_view(id,0.,Some(&foreign),&opts).unwrap().unwrap();
    for samples in [1,3] {
        editing.add(samples);
        let image=editing.picture();let p=image.pixel(110,45).unwrap();
        assert!(p.red()>200 && p.green()>30 && p.blue()>5,"the evaluated face stays highlighted through the scene camera after {} samples: {p:?}",editing.samples());
        assert!(image.pixel(80,45).unwrap().red()<20,"overlay does not fall back to the default expression position");
    }
    assert!(editing.done());

}

#[test]
fn a_composition_is_viewed_on_its_own_canvas() {
    // An 800×200 composition with a background, seen alone (the Studio's composition view,
    // motion.view composition): its background fills its frame only, centred like its layers.
    let Some(tools) = tools() else { return };
    let scene = kimchi_core::Scene::from_json(&serde_json::json!({"type": "2d", "background": "#0000ff",
        "compositions": [{"id": "card", "width": 800, "height": 200, "background": "#ff0000", "layers": [
            {"id": "dot", "type": "ellipse", "width": 100, "height": 100, "fill": "#00ff00", "keyframes": {"x": [[0, -300], [1, 300]]}}]}],
        "layers": [{"id": "c", "type": "comp", "comp": "card"}]}))
    .unwrap();
    let clip = Clip::new("m", 0.0, 2.0, ClipContent::Motion { scene, template: None });
    let id = clip.id;
    let p = project_with(clip, 1280, 720);
    let mut r = Renderer::new(&tools, &p, 640, 360, 30.0);
    let pic = r.scene_view(id, 1.0, None, &Default::default(), Some("card")).unwrap();
    dump("composition-view", &pic);
    let at = |x: u32, y: u32| {
        let c = pic.pixel(x, y).unwrap().demultiply();
        (c.red(), c.green(), c.blue(), c.alpha())
    };
    assert_eq!(at(320, 40).3, 0, "nothing above the composition's frame");
    assert_eq!(at(140, 180), (255, 0, 0, 255), "its background inside");
    assert_eq!(at(470, 180).1, 255, "the dot at the composition's time");
    assert_eq!(at(630, 180).3, 0, "nothing beside the frame");
}

#[test]
fn canvas_views_keep_zoomed_vectors_sharp_and_preserve_project_and_composition_coordinates() {
    use kimchi_media::render::CanvasView;
    let Some(tools)=tools() else {return};
    let scene=kimchi_core::Scene::from_json(&serde_json::json!({"background":"#123456","layers":[
        {"id":"card","type":"rect","width":12,"height":8,"rotation":13,"y":-8,"fill":"#4fba91","keyframes":{"x":[[0,10],[1,20]]}},
        {"id":"hairline","type":"rect","width":0.25,"height":10,"x":23,"y":-8,"fill":"#ffffff"}],
        "compositions":[{"id":"large","width":4000,"height":1000,"duration":2,"background":"#234567","layers":[
            {"id":"comp-line","type":"rect","width":0.25,"height":10,"x":1503,"y":-20,"fill":"#ffffff"}]}]
    })).unwrap();
    let clip=Clip::new("canvas",0.,4.,ClipContent::Motion {scene,template:None});let id=clip.id;
    let project=project_with(clip,128,72);
    let mut full=Renderer::new(&tools,&project,256,144,30.);
    let original=full.scene_view(id,1.,None,&Default::default(),None).unwrap();
    let fitted=full.scene_canvas(id,1.,None,&CanvasView {centre:[0.,0.],scale:[2.,2.]}).unwrap();
    assert_eq!(fitted.data(),original.data(),"an uncropped view matches the existing shared compositor exactly");
    let mut full=Renderer::new(&tools,&project,1024,576,30.);
    let reference=full.scene_view(id,1.,None,&Default::default(),None).unwrap();
    let mut crop=Renderer::new(&tools,&project,160,128,30.);
    let view=CanvasView {centre:[20.,-8.],scale:[8.,8.]};
    let zoomed=crop.scene_canvas(id,1.,None,&view).unwrap();
    for y in 0..128 {for x in 0..160 {
        assert_eq!(zoomed.pixel(x,y),reference.pixel(x+592,y+160),"crop pixel ({x},{y}) must retain full-resolution vector coverage");
    }}
    assert_eq!(zoomed.pixel(104,64).unwrap().red(),255,"a quarter-pixel source line is sharp at 800% zoom");
    assert_ne!(zoomed.pixel(102,64).unwrap().red(),255);
    dump("canvas-crop-800",&zoomed);
    let edge=crop.scene_canvas(id,1.,None,&CanvasView {centre:[64.,0.],scale:[2.,2.]}).unwrap();
    assert_eq!(edge.pixel(79,64).unwrap().alpha(),255);
    assert_eq!(edge.pixel(81,64).unwrap().alpha(),0,"scene background stops at the actual project boundary");
    let comp_view=CanvasView {centre:[1500.,-20.],scale:[8.,8.]};
    let comp=crop.scene_canvas(id,1.,Some("large"),&comp_view).unwrap();
    assert_eq!(comp.pixel(104,64).unwrap().red(),255,"an opened composition renders directly, even far outside the smaller project canvas");
    assert_eq!(comp.pixel(102,64).unwrap().red(),0x23,"composition backgrounds and coordinates remain unchanged");
    assert!(crop.scene_canvas(id,3.,Some("large"),&comp_view).unwrap().pixels().iter().all(|p|p.alpha()==0),"an ended composition has no content or background");
    dump("composition-crop-800",&comp);
    for scale in [[0.,1.],[f64::NAN,1.],[f64::INFINITY,1.],[1e100,1.]] {
        assert!(crop.scene_canvas(id,1.,None,&CanvasView {scale,..view}).is_err());
    }
    // A mirror samples the opposite side of the scene, well outside this viewport crop.
    let scene=kimchi_core::Scene::from_json(&serde_json::json!({"layers":[
        {"id":"reflected","type":"rect","width":8,"height":8,"x":30,"fill":"#ffffff","effects":[{"type":"mirror"}]}]
    })).unwrap();
    let clip=Clip::new("mirror",0.,4.,ClipContent::Motion {scene,template:None});let id=clip.id;let project=project_with(clip,128,72);
    let reference=Renderer::new(&tools,&project,1024,576,30.).scene_view(id,1.,None,&Default::default(),None).unwrap();
    let reflected=Renderer::new(&tools,&project,80,80,30.).scene_canvas(id,1.,None,&CanvasView {centre:[-30.,0.],scale:[8.,8.]}).unwrap();
    assert_eq!(reflected.pixel(40,40).unwrap().red(),255,"effects keep the offscreen source pixels they need");
    for y in 0..80 {for x in 0..80 {assert_eq!(reflected.pixel(x,y),reference.pixel(x+232,y+248));}}
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
