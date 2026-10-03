use chrono::Utc;
use kimchi_core::{Asset, AssetOrigin, Keyframe, MediaMeta, ProjectSettings, TextStyle, Track, TrackKind, new_id};

use super::*;

fn caps() -> Caps {
    Caps::new(9, ["libx264", "libx265", "prores_ks", "libvpx-vp9", "libopus", "aac", "gif"])
}

fn asset(kind: MediaKind, path: &str, has_audio: bool) -> Asset {
    Asset {
        id: new_id(),
        name: path.into(),
        kind,
        path: path.into(),
        meta: MediaMeta {
            duration: (kind != MediaKind::Image).then_some(10.0),
            width: Some(1920),
            height: Some(1080),
            fps: (kind == MediaKind::Video).then_some(30.0),
            has_video: kind != MediaKind::Audio,
            has_audio,
            ..Default::default()
        },
        origin: AssetOrigin::Imported,
        created_at: Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
    }
}

fn media(a: &Asset, start: f64, duration: f64) -> Clip {
    Clip::new(&a.name, start, duration, ClipContent::Media { asset_id: a.id })
}

fn project(tracks: Vec<(TrackKind, Vec<Clip>)>, assets: Vec<Asset>) -> Project {
    let mut p = Project::new("t", ProjectSettings { width: 640, height: 360, fps: 25.0, ..Default::default() });
    p.assets = assets;
    p.tracks = tracks.into_iter().map(|(kind, clips)| Track { clips, ..Track::new(kind, "t") }).collect();
    p
}

fn settings(format: ExportFormat) -> ExportSettings {
    ExportSettings {
        path: "/tmp/out".into(),
        format,
        quality: Quality::Standard,
        width: None,
        height: None,
        fps: None,
        range: None,
        encoder: Default::default(),
    }
}

fn plan(p: &Project, format: ExportFormat) -> Plan {
    build(p, &settings(format), &caps()).unwrap()
}

fn input_files(plan: &Plan) -> Vec<&str> {
    plan.inputs.windows(2).filter(|w| w[0] == "-i").map(|w| w[1].as_str()).collect()
}

#[test]
fn pictures_come_from_the_compositor_through_a_pipe() {
    let (top, bottom) = (asset(MediaKind::Image, "top.png", false), asset(MediaKind::Video, "bottom.mp4", true));
    let p = project(
        vec![(TrackKind::Video, vec![media(&top, 1.5, 2.0)]), (TrackKind::Video, vec![media(&bottom, 0.0, 4.0)])],
        vec![top, bottom],
    );
    let plan = plan(&p, ExportFormat::Mp4);
    // Input 0 is the rendered picture; media files are only read for their sound.
    assert_eq!(&plan.inputs[..10], ["-f", "rawvideo", "-pix_fmt", "rgba", "-s", "640x360", "-r", "25.0", "-i", "pipe:0"]);
    assert_eq!(input_files(&plan), ["pipe:0", "bottom.mp4"]);
    assert_eq!(plan.video, Some(VideoFeed { width: 640, height: 360, fps: 25.0, from: 0.0, frames: 100 }));
    assert_eq!(plan.duration, 4.0);
    let g = &plan.graph;
    assert!(g.starts_with("[0:v]scale=out_color_matrix=bt709:out_range=tv,format=yuv420p[vout]"), "{g}");
    assert!(g.contains("[1:a:0]aformat="), "{g}");
    let out = plan.output.join(" ");
    assert!(out.contains("-c:v libx264") && out.contains("-colorspace bt709"), "{out}");
    // The picture comes on stdin, so ffmpeg must read it.
    let args = plan.args(Path::new("o.mp4"), None, &caps());
    assert!(!args.contains(&"-nostdin".to_string()));
}

#[test]
fn speed_trims_source_and_chains_atempo() {
    assert_eq!(atempo(3.0), [2.0, 1.5]);
    assert_eq!(atempo(0.25), [0.5, 0.5]);
    assert!(atempo(1.0).is_empty());

    let v = asset(MediaKind::Video, "v.mp4", true);
    let mut clip = media(&v, 2.0, 1.0);
    clip.speed = 3.0;
    clip.in_point = 1.0;
    let p = project(vec![(TrackKind::Video, vec![clip])], vec![v]);
    let plan = plan(&p, ExportFormat::Mp4);
    // 1 s on the timeline at 3x = 3 s of source starting at the in point (input 1, after the picture).
    assert_eq!(&plan.inputs[10..], ["-ss", "1.0", "-t", "3.0", "-i", "v.mp4"]);
    let g = &plan.graph;
    assert!(g.contains("[1:a:0]aformat="), "{g}");
    assert!(g.contains("atempo=2.0,atempo=1.5,asetpts=N/48000/TB"), "{g}");
    assert!(g.contains("adelay=delays=96000S:all=1[a"), "{g}");
    assert!(
        g.contains(
            "amix=inputs=1:normalize=0:dropout_transition=0,asetpts=N/48000/TB,apad=whole_dur=3.0,atrim=end=3.0[aout]"
        ),
        "{g}"
    );
}

#[test]
fn skips_hidden_muted_and_pending() {
    let (img, snd) = (asset(MediaKind::Image, "hidden.png", false), asset(MediaKind::Audio, "muted.wav", true));
    let pending = Clip::new(
        "p",
        0.0,
        2.0,
        ClipContent::Pending { job_id: "j".into(), kind: MediaKind::Video, prompt: "x".into(), model_name: "m".into() },
    );
    let mut p = project(
        vec![
            (TrackKind::Video, vec![media(&img, 0.0, 2.0)]),
            (TrackKind::Video, vec![pending]),
            (TrackKind::Audio, vec![media(&snd, 0.0, 2.0)]),
        ],
        vec![img, snd],
    );
    p.tracks[0].hidden = true;
    p.tracks[2].muted = true;
    let plan = plan(&p, ExportFormat::Mp4);
    assert_eq!(input_files(&plan), ["pipe:0"], "{:?}", plan.inputs);
    assert!(plan.graph.contains("anullsrc=r=48000:cl=stereo,atrim=end=2.0[aout]"));
}

#[test]
fn volume_keyframes_become_an_expression() {
    let snd = asset(MediaKind::Audio, "a.wav", true);
    let mut clip = media(&snd, 0.0, 2.0);
    clip.keyframes.insert("volume".into(), vec![Keyframe::new(0.0, 0.0, Default::default()), Keyframe::new(1.0, 1.0, Default::default())]);
    let p = project(vec![(TrackKind::Audio, vec![clip.clone()])], vec![snd]);
    let g = plan(&p, ExportFormat::Audio).graph;
    assert!(g.contains("volume=eval=frame:volume='if(lt(t,0.05),0.0+(1.0)*(t-0.0),if("), "{g}");
    // The curve holds the last value past its last keyframe.
    assert!(volume_curve(&clip, 0.0).trim_end_matches(')').ends_with(",1.0"), "{g}");
    let text = Clip::new("t", 0.0, 1.0, ClipContent::Text { style: TextStyle::default() });
    let p = project(vec![(TrackKind::Video, vec![text])], vec![]);
    assert_eq!(plan(&p, ExportFormat::Mp4).video.unwrap().frames, 25);
}

#[test]
fn range_shifts_and_cuts_clips() {
    let v = asset(MediaKind::Video, "v.mp4", true);
    let mut a = media(&v, 0.0, 4.0);
    a.fade_in = 3.5;
    let mut b = media(&v, 4.0, 4.0);
    b.in_point = 1.0;
    let late = media(&v, 9.0, 1.0);
    let p = project(vec![(TrackKind::Video, vec![a, b, late])], vec![v]);
    let mut st = settings(ExportFormat::Mp4);
    st.range = Some((3.0, 6.0));
    let plan = build(&p, &st, &caps()).unwrap();
    assert_eq!(plan.duration, 3.0);
    assert_eq!(plan.video.unwrap().from, 3.0);
    // `a` is still fading in at 3 s: decode from its start, fade, then drop the first 3 s.
    // `b` starts inside the window; `late` is outside and ignored.
    assert_eq!(&plan.inputs[10..], ["-t", "4.0", "-i", "v.mp4", "-ss", "1.0", "-t", "2.0", "-i", "v.mp4"]);
    let g = &plan.graph;
    assert!(g.contains("afade=t=in:st=0.0:d=3.5"), "{g}");
    assert!(g.contains("atrim=start=3.0:end=4.0,asetpts=PTS-STARTPTS"), "{g}");
    assert!(g.contains("adelay=delays=48000S:all=1"), "{g}");

    // Without a fade the input is simply seeked further.
    let w = Window::of(&media(&asset(MediaKind::Video, "x", false), 0.0, 4.0), 3.0, 6.0).unwrap();
    assert_eq!((w.start, w.len, w.decode_from, w.trim), (0.0, 1.0, 3.0, 0.0));
}

#[test]
fn formats_and_fallbacks() {
    let snd = asset(MediaKind::Audio, "a.wav", true);
    let p = project(vec![(TrackKind::Audio, vec![media(&snd, 0.0, 2.0)])], vec![snd]);

    let gif = plan(&p, ExportFormat::Gif);
    assert!(!gif.graph.contains("aout") && input_files(&gif) == ["pipe:0"]);
    assert!(gif.graph.contains("palettegen") && gif.output.join(" ").contains("-c:v gif"));
    // GIF fps is capped.
    assert_eq!(gif.video.unwrap().fps, 15.0);

    let audio = plan(&p, ExportFormat::Audio);
    assert!(!audio.graph.contains("vout") && audio.output.join(" ").contains("-c:a aac"));
    assert!(audio.video.is_none() && audio.args(Path::new("o.m4a"), None, &caps()).contains(&"-nostdin".to_string()));
    assert!(audio.output.join(" ").contains("-f ipod"));

    let webm = plan(&p, ExportFormat::Webm);
    assert!(webm.output.join(" ").contains("-c:v libvpx-vp9") && webm.output.join(" ").contains("-ar 48000"));

    let prores = plan(&p, ExportFormat::Prores);
    assert!(prores.graph.contains("format=yuv422p10le[vout]") && prores.output.join(" ").contains("pcm_s16le"));

    let vt = Caps::new(9, ["h264_videotoolbox", "aac"]);
    let mp4 = build(&p, &settings(ExportFormat::Mp4), &vt).unwrap();
    assert!(mp4.output.join(" ").contains("-c:v h264_videotoolbox -b:v"));
    assert!(matches!(build(&p, &settings(ExportFormat::Hevc), &vt), Err(MediaError::Unsupported(_))));

    let hevc = plan(&p, ExportFormat::Hevc);
    assert!(hevc.output.join(" ").contains("-tag:v hvc1"));

    // Long graphs go through a file; the option name depends on the ffmpeg version.
    let args = audio.args(Path::new("o.m4a"), Some(Path::new("g.txt")), &caps());
    assert!(args.windows(2).any(|w| w == ["-/filter_complex", "g.txt"]));
    let args = audio.args(Path::new("o.m4a"), Some(Path::new("g.txt")), &Caps::new(6, ["aac"]));
    assert!(args.windows(2).any(|w| w == ["-filter_complex_script", "g.txt"]));
    assert_eq!(args.last().unwrap(), "o.m4a");
}

#[test]
fn output_size_and_empty_projects() {
    let v = asset(MediaKind::Video, "v.mp4", false);
    let mut clip = media(&v, 0.0, 1.0);
    clip.transform.x = 100.0;
    let p = project(vec![(TrackKind::Video, vec![clip])], vec![v]);
    let mut st = settings(ExportFormat::Mp4);
    st.width = Some(1281);
    let plan = build(&p, &st, &caps()).unwrap();
    // Even dimensions.
    assert!(plan.inputs.contains(&"1280x720".to_string()), "{:?}", plan.inputs);
    assert_eq!((plan.video.unwrap().width, plan.video.unwrap().height), (1280, 720));

    let empty = project(vec![(TrackKind::Video, vec![])], vec![]);
    assert!(matches!(build(&empty, &st, &caps()), Err(MediaError::Unsupported(_))));
    st.range = Some((5.0, 6.0));
    assert!(matches!(build(&p, &st, &caps()), Err(MediaError::Unsupported(_))));
}

#[test]
fn numbers_and_colors() {
    assert_eq!(num(2.0), "2.0");
    assert_eq!(num(1.0 / 3.0), "0.333333");
    assert_eq!(num(-0.0000001), "0.0");
    assert_eq!(color("#abc"), "0xaabbcc");
    assert_eq!(color("#11223344"), "0x11223344");
    assert_eq!(color("nope"), "black");
}

#[test]
fn hardware_encodes_the_compositor_pipe_and_uploads_vaapi_frames() {
    use crate::accel::{CANDIDATES, Verified};
    let p = project(vec![(TrackKind::Video, vec![Clip::new("solid", 0.0, 1.0, ClipContent::Solid { color: "#ff0000".into() })])], vec![]);
    let v = Verified { encoder: *CANDIDATES.iter().find(|c| c.name == "h264_vaapi").unwrap(), constant_quality: false };
    let hw = Hardware { encoders: vec![v], vaapi_device: Some("/dev/dri/renderD128".into()) };
    let plan = build_with_hardware(&p, &settings(ExportFormat::Mp4), &caps(), &hw).unwrap();
    assert!(plan.hardware);
    assert_eq!(plan.encoder.as_deref(), Some("h264_vaapi"));
    assert!(plan.video.is_some());
    assert_eq!(input_files(&plan), ["pipe:0"]);
    assert!(plan.graph.contains("format=nv12,hwupload[vout]"), "{}", plan.graph);
    assert!(plan.inputs.iter().any(|a| a == "-init_hw_device"));
    let mut st = settings(ExportFormat::Mp4);
    st.encoder = EncoderChoice::Software;
    let cpu = build_with_hardware(&p, &st, &caps(), &hw).unwrap();
    assert!(!cpu.hardware);
    assert_eq!(cpu.encoder.as_deref(), Some("libx264"));
}
