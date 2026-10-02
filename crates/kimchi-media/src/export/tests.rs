use chrono::Utc;
use kimchi_core::{Asset, AssetOrigin, MediaMeta, ProjectSettings, TextStyle, Track, Transform, new_id};

use super::*;
use crate::accel::{EncoderChoice, Hardware, Verified};

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
    build(p, &Overlays::new(), &settings(format), &caps(), &Hardware::none()).unwrap()
}

fn input_files(plan: &Plan) -> Vec<&str> {
    plan.inputs.windows(2).filter(|w| w[0] == "-i").map(|w| w[1].as_str()).collect()
}

#[test]
fn layers_bottom_track_first_and_offsets_clips() {
    let (top, bottom) = (asset(MediaKind::Image, "top.png", false), asset(MediaKind::Video, "bottom.mp4", false));
    let p = project(
        vec![(TrackKind::Video, vec![media(&top, 1.5, 2.0)]), (TrackKind::Video, vec![media(&bottom, 0.0, 4.0)])],
        vec![top, bottom],
    );
    let plan = plan(&p, ExportFormat::Mp4);
    assert_eq!(input_files(&plan), ["bottom.mp4", "top.png"]);
    assert_eq!(plan.duration, 4.0);
    let g = &plan.graph;
    assert!(g.starts_with("color=c=0x000000:s=640x360:r=25.0:d=4.0,format=yuv420p[b0]"), "{g}");
    // Bottom clip goes onto the base first, the top one onto the result.
    let first = g.find("[b0][v1]overlay").expect(g);
    let second = g.find("[b1][v3]overlay").expect(g);
    assert!(first < second);
    assert!(g.contains("[1:v:0]scale=w=") && g.contains("setpts=PTS-STARTPTS+1.5/TB[v3]"), "{g}");
    assert!(g.contains("enable='between(t,1.48,3.48)'"), "{g}");
    assert!(g.contains("[b2]format=yuv420p[vout]"));
    // 30 fps source into 25 fps: surplus frames dropped before scaling.
    assert!(g.contains("[0:v:0]setpts=PTS-STARTPTS,fps=25.0,scale="), "{g}");
    // Image input loops for the clip length.
    assert!(plan.inputs.join(" ").contains("-loop 1 -framerate 25.0 -t 2.0 -i top.png"));
    assert!(plan.output.join(" ").contains("-c:v libx264"));
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
    // 1 s on the timeline at 3x = 3 s of source starting at the in point; one input for picture and sound.
    assert_eq!(plan.inputs, ["-ss", "1.0", "-t", "3.0", "-i", "v.mp4"]);
    let g = &plan.graph;
    assert!(g.contains("[0:v:0]setpts=(PTS-STARTPTS)/3.0,"), "{g}");
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
    assert!(plan.inputs.is_empty(), "{:?}", plan.inputs);
    assert!(!plan.graph.contains("overlay"));
    assert!(plan.graph.contains("anullsrc=r=48000:cl=stereo,atrim=end=2.0[aout]"));
}

#[test]
fn transforms_solids_text_and_fades() {
    let mut solid = Clip::new("s", 0.0, 2.0, ClipContent::Solid { color: "#f00".into() });
    solid.transform = Transform { x: 10.0, y: -20.0, scale: 0.5, rotation: 15.0, opacity: 0.5, ..Default::default() };
    solid.fade_in = 0.5;
    solid.fade_out = 0.25;
    let text = Clip::new("t", 1.0, 1.0, ClipContent::Text { style: TextStyle::default() });
    let overlays = Overlays::from([(text.id, PathBuf::from("text.png"))]);
    let p = project(vec![(TrackKind::Video, vec![text]), (TrackKind::Video, vec![solid])], vec![]);
    let plan = build(&p, &overlays, &settings(ExportFormat::Mp4), &caps(), &Hardware::none()).unwrap();
    let g = &plan.graph;
    assert!(
        g.contains(
            "color=c=0xff0000:s=320x180:r=25.0:d=2.0,format=rgba,rotate=a=0.261799:ow=rotw(0.261799):oh=roth(0.261799):c=none,\
             colorchannelmixer=aa=0.5,fade=t=in:st=0.0:d=0.5:alpha=1,fade=t=out:st=1.75:d=0.25:alpha=1,setpts=PTS-STARTPTS+0.0/TB"
        ),
        "{g}"
    );
    assert!(g.contains("overlay=x=(main_w-overlay_w)/2+10.0:y=(main_h-overlay_h)/2-20.0:"), "{g}");
    // Text: the PNG is already laid out, only timing applies.
    assert!(g.contains("[0:v:0]setpts=PTS-STARTPTS+1.0/TB[v"), "{g}");
    assert!(g.contains("overlay=x=0:y=0:"), "{g}");
    // A text clip without a rendered overlay is skipped rather than failing the export.
    let plan = build(&p, &Overlays::new(), &settings(ExportFormat::Mp4), &caps(), &Hardware::none()).unwrap();
    assert!(plan.inputs.is_empty());
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
    let plan = build(&p, &Overlays::new(), &st, &caps(), &Hardware::none()).unwrap();
    assert_eq!(plan.duration, 3.0);
    // `a` is still fading in at 3 s: decode from its start, fade, then drop the first 3 s.
    // `b` starts inside the window; `late` is outside and ignored.
    assert_eq!(plan.inputs, ["-t", "4.0", "-i", "v.mp4", "-ss", "1.0", "-t", "2.0", "-i", "v.mp4"]);
    let g = &plan.graph;
    assert!(g.contains("fade=t=in:st=0.0:d=3.5:alpha=1,trim=start=3.0,setpts=PTS-STARTPTS+0.0/TB"), "{g}");
    assert!(g.contains("atrim=start=3.0:end=4.0,asetpts=PTS-STARTPTS"), "{g}");
    assert!(g.contains("setpts=PTS-STARTPTS+1.0/TB"), "{g}");
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
    assert!(!gif.graph.contains("aout") && gif.inputs.is_empty());
    assert!(gif.graph.contains("palettegen") && gif.output.join(" ").contains("-c:v gif"));
    // GIF fps is capped.
    assert!(gif.graph.contains(":r=15.0:"));

    let audio = plan(&p, ExportFormat::Audio);
    assert!(!audio.graph.contains("vout") && audio.output.join(" ").contains("-c:a aac"));
    assert!(audio.output.join(" ").contains("-f ipod"));

    let webm = plan(&p, ExportFormat::Webm);
    assert!(webm.output.join(" ").contains("-c:v libvpx-vp9") && webm.output.join(" ").contains("-ar 48000"));

    let prores = plan(&p, ExportFormat::Prores);
    assert!(prores.graph.contains("format=yuv422p10le[vout]") && prores.output.join(" ").contains("pcm_s16le"));

    let vt = Caps::new(9, ["h264_videotoolbox", "aac"]);
    let mp4 = build(&p, &Overlays::new(), &settings(ExportFormat::Mp4), &vt, &Hardware::none()).unwrap();
    assert!(mp4.output.join(" ").contains("-c:v h264_videotoolbox -b:v"));
    assert!(matches!(build(&p, &Overlays::new(), &settings(ExportFormat::Hevc), &vt, &Hardware::none()), Err(MediaError::Unsupported(_))));

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
    let plan = build(&p, &Overlays::new(), &st, &caps(), &Hardware::none()).unwrap();
    // Even dimensions, offsets scaled from project pixels to output pixels.
    assert!(plan.graph.contains("s=1280x720:"), "{}", plan.graph);
    assert!(plan.graph.contains("(main_w-overlay_w)/2+200.0"), "{}", plan.graph);

    let empty = project(vec![(TrackKind::Video, vec![])], vec![]);
    assert!(matches!(build(&empty, &Overlays::new(), &st, &caps(), &Hardware::none()), Err(MediaError::Unsupported(_))));
    st.range = Some((5.0, 6.0));
    assert!(matches!(build(&p, &Overlays::new(), &st, &caps(), &Hardware::none()), Err(MediaError::Unsupported(_))));
}

#[test]
fn numbers_and_colors() {
    assert_eq!(num(2.0), "2.0");
    assert_eq!(num(1.0 / 3.0), "0.333333");
    assert_eq!(num(-0.0000001), "0.0");
    assert_eq!(signed(-3.5), "-3.5");
    assert_eq!(color("#abc"), "0xaabbcc");
    assert_eq!(color("#11223344"), "0x11223344");
    assert_eq!(color("nope"), "black");
}

fn verified(name: &str) -> Verified {
    Verified { encoder: *accel::CANDIDATES.iter().find(|c| c.name == name).unwrap(), constant_quality: true }
}

#[test]
fn hardware_encoders_first_cpu_on_request() {
    let v = asset(MediaKind::Video, "v.mp4", true);
    let p = project(vec![(TrackKind::Video, vec![media(&v, 0.0, 2.0)])], vec![v]);
    let hw = Hardware {
        encoders: vec![verified("h264_nvenc"), verified("hevc_vaapi"), verified("av1_nvenc")],
        vaapi_device: Some("/dev/dri/renderD129".into()),
    };
    let with = |format, choice, width: Option<u32>| {
        let mut st = settings(format);
        st.encoder = choice;
        st.width = width;
        build(&p, &Overlays::new(), &st, &caps(), &hw)
    };

    let mp4 = with(ExportFormat::Mp4, EncoderChoice::Auto, None).unwrap();
    assert!(mp4.output.join(" ").contains("-c:v h264_nvenc -preset p5 -tune hq -rc vbr -cq 24 -b:v 0 -pix_fmt nv12"));
    assert!(mp4.graph.contains("format=nv12[vout]") && mp4.output.join(" ").contains("-c:a aac"));
    assert_eq!((mp4.encoder.as_deref(), mp4.hardware), (Some("h264_nvenc"), true));

    // VA-API: the device is opened before the inputs and the frames go up at the end of the graph.
    let hevc = with(ExportFormat::Hevc, EncoderChoice::Auto, None).unwrap();
    assert_eq!(hevc.inputs[..4].join(" "), "-init_hw_device vaapi=kva:/dev/dri/renderD129 -filter_hw_device kva");
    assert!(hevc.graph.contains("format=nv12,hwupload[vout]"));
    assert!(hevc.output.join(" ").contains("-tag:v hvc1") && !hevc.output.contains(&s("-pix_fmt")));

    let cpu = with(ExportFormat::Mp4, EncoderChoice::Software, None).unwrap();
    assert_eq!((cpu.encoder.as_deref(), cpu.hardware), (Some("libx264"), false));
    assert!(cpu.graph.contains("format=yuv420p[vout]"));

    // H.264 hardware stops at 4096 wide: Auto goes to the CPU, Hardware says why.
    let big = with(ExportFormat::Mp4, EncoderChoice::Auto, Some(7680)).unwrap();
    assert_eq!(big.encoder.as_deref(), Some("libx264"));
    match with(ExportFormat::Mp4, EncoderChoice::Hardware, Some(7680)) {
        Err(MediaError::Unsupported(msg)) => assert!(msg.contains("too big"), "{msg}"),
        other => panic!("{other:?}"),
    }

    // WebM: VP9 on the CPU unless hardware is asked for, which may give AV1.
    assert_eq!(with(ExportFormat::Webm, EncoderChoice::Auto, None).unwrap().encoder.as_deref(), Some("libvpx-vp9"));
    assert_eq!(with(ExportFormat::Webm, EncoderChoice::Hardware, None).unwrap().encoder.as_deref(), Some("av1_nvenc"));
    assert!(matches!(with(ExportFormat::Prores, EncoderChoice::Hardware, None), Err(MediaError::Unsupported(_))));

    // Sound only and GIF never touch the GPU.
    let gif = with(ExportFormat::Gif, EncoderChoice::Auto, None).unwrap();
    assert_eq!((gif.encoder.as_deref(), gif.hardware), (Some("gif"), false));
    let audio = with(ExportFormat::Audio, EncoderChoice::Hardware, None).unwrap();
    assert_eq!((audio.encoder, audio.hardware), (None, false));
}

#[test]
fn heavy_sources_decode_in_hardware_up_to_a_limit() {
    let with_hw = caps().with_hwaccels(["videotoolbox", "cuda", "vaapi"]);
    let mut assets = vec![];
    let mut clips = vec![];
    for i in 0..6 {
        let mut a = asset(MediaKind::Video, &format!("uhd{i}.mov"), false);
        (a.meta.width, a.meta.height, a.meta.video_codec) = (Some(3840), Some(2160), Some("hevc".into()));
        clips.push(media(&a, i as f64, 1.0));
        assets.push(a);
    }
    let light = asset(MediaKind::Video, "hd.mp4", false);
    clips.push(media(&light, 0.0, 1.0));
    assets.push(light);
    let p = project(vec![(TrackKind::Video, clips)], assets);
    let plan = build(&p, &Overlays::new(), &settings(ExportFormat::Mp4), &with_hw, &Hardware::none()).unwrap();
    let hw = plan.inputs.iter().filter(|a| *a == "-hwaccel").count();
    assert_eq!(hw, accel::MAX_HW_DECODERS);
    // The 1080p H.264 source decodes in software, right before its -i.
    let i = plan.inputs.iter().position(|a| a == "hd.mp4").unwrap();
    assert_ne!(plan.inputs[i - 3], "-hwaccel");
    // A build without hardware decoders gets none.
    let plan = build(&p, &Overlays::new(), &settings(ExportFormat::Mp4), &caps(), &Hardware::none()).unwrap();
    assert!(!plan.inputs.contains(&s("-hwaccel")));
}
