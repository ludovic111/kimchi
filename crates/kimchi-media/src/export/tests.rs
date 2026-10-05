use chrono::Utc;
use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, MediaKind, MediaMeta, ProjectSettings, TextStyle, Track, TrackKind, new_id};

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
        beats: None,
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
        audio: Default::default(),
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
    // Input 0 is the rendered picture, input 1 the mixer's sound; the media files are read by
    // the compositor and the mixer, not by this ffmpeg.
    assert_eq!(&plan.inputs[..10], ["-f", "rawvideo", "-pix_fmt", "rgba", "-s", "640x360", "-r", "25.0", "-i", "pipe:0"]);
    assert_eq!(&plan.inputs[10..17], ["-f", "f32le", "-ar", "48000", "-ac", "2", "-i"]);
    let mix = plan.mix.clone().unwrap();
    assert_eq!(input_files(&plan), ["pipe:0", mix.path.to_str().unwrap()]);
    assert_eq!((mix.rate, mix.from, mix.to), (48_000, 0.0, 4.0));
    assert_eq!(plan.sources, [PathBuf::from("bottom.mp4")]);
    assert_eq!(plan.video, Some(VideoFeed { width: 640, height: 360, fps: 25.0, from: 0.0, frames: 100 }));
    assert_eq!(plan.duration, 4.0);
    let g = &plan.graph;
    assert_eq!(g, "[0:v]scale=out_color_matrix=bt709:out_range=tv,format=yuv420p[vout]");
    let out = plan.output.join(" ");
    assert!(out.contains("-c:v libx264") && out.contains("-colorspace bt709"), "{out}");
    assert!(out.contains("-map 1:a -c:a aac -b:a 192k -ar 48000"), "{out}");
    // The picture comes on stdin, so ffmpeg must read it.
    let args = plan.args(Path::new("o.mp4"), None, &caps());
    assert!(!args.contains(&"-nostdin".to_string()));
}

#[test]
fn sound_formats_rates_and_depths() {
    assert_eq!(atempo(3.0), [2.0, 1.5]);
    let snd = asset(MediaKind::Audio, "a.wav", true);
    let p = project(vec![(TrackKind::Audio, vec![media(&snd, 0.0, 2.0)])], vec![snd]);
    let caps = Caps::new(9, ["libx264", "prores_ks", "libvpx-vp9", "aac", "libmp3lame", "libopus", "libvorbis", "flac"]);
    let out = |audio: AudioOptions, format: ExportFormat| build(&p, &ExportSettings { audio, ..settings(format) }, &caps).map(|plan| (plan.output.join(" "), plan.mix.unwrap().rate));
    // Defaults: AAC in M4A, 24-bit WAV, at the project's rate.
    assert!(out(AudioOptions::default(), ExportFormat::Audio).unwrap().0.contains("-c:a aac -b:a 192k -ar 48000 -movflags +faststart -f ipod"));
    assert!(out(AudioOptions::default(), ExportFormat::Wav).unwrap().0.contains("-c:a pcm_s24le -ar 48000 -rf64 auto -f wav"));
    let with = |format: AudioFormat| AudioOptions { format: Some(format), ..Default::default() };
    let (o, _) = out(AudioOptions { bit_depth: Some(16), sample_rate: Some(44_100), ..with(AudioFormat::Wav) }, ExportFormat::Audio).unwrap();
    assert!(o.contains("-c:a pcm_s16le -af aresample=dither_method=triangular,aformat=sample_fmts=s16 -ar 44100"), "{o}");
    assert!(out(AudioOptions { bit_depth: Some(32), ..with(AudioFormat::Wav) }, ExportFormat::Audio).unwrap().0.contains("pcm_f32le"));
    assert!(out(with(AudioFormat::Aiff), ExportFormat::Audio).unwrap().0.contains("-c:a pcm_s24be -ar 48000 -f aiff"));
    assert!(out(AudioOptions { bit_depth: Some(32), ..with(AudioFormat::Aiff) }, ExportFormat::Audio).is_err());
    assert!(out(with(AudioFormat::Flac), ExportFormat::Audio).unwrap().0.contains("-c:a flac -sample_fmt s32 -bits_per_raw_sample 24"));
    let (o, rate) = out(AudioOptions { bitrate_kbps: Some(320), sample_rate: Some(96_000), ..with(AudioFormat::Mp3) }, ExportFormat::Audio).unwrap();
    assert!(o.contains("-c:a libmp3lame -b:a 320k -ar 48000 -f mp3") && rate == 48_000, "{o}");
    let (o, rate) = out(AudioOptions { sample_rate: Some(44_100), ..with(AudioFormat::Opus) }, ExportFormat::Audio).unwrap();
    assert!(o.contains("-c:a libopus -b:a 128k -ar 48000 -f ogg") && rate == 48_000, "{o}");
    assert!(out(with(AudioFormat::Vorbis), ExportFormat::Audio).unwrap().0.contains("-c:a libvorbis -q:a 6"));
    assert!(out(AudioOptions { sample_rate: Some(22_050), ..Default::default() }, ExportFormat::Audio).is_err());
    assert!(out(AudioOptions { bit_depth: Some(20), ..Default::default() }, ExportFormat::Wav).is_err());
    // An ffmpeg without LAME says so.
    let lame = build(&p, &ExportSettings { audio: with(AudioFormat::Mp3), ..settings(ExportFormat::Audio) }, &Caps::new(9, ["aac"]));
    assert!(matches!(lame, Err(MediaError::Unsupported(m)) if m.contains("libmp3lame")));
    // Video exports keep their codec; ProRes takes the bit depth; WebM's Opus runs at 48 kHz.
    assert!(out(AudioOptions { bit_depth: Some(24), ..Default::default() }, ExportFormat::Prores).unwrap().0.contains("pcm_s24le"));
    assert_eq!(out(AudioOptions { sample_rate: Some(96_000), ..Default::default() }, ExportFormat::Webm).unwrap().1, 48_000);
    assert_eq!(out(AudioOptions { sample_rate: Some(96_000), ..Default::default() }, ExportFormat::Mp4).unwrap().1, 96_000);
    // Old settings without `audio` still read, and the options read back.
    let old: ExportSettings = serde_json::from_str(r#"{"path":"a.mp4","format":"mp4","quality":"draft","width":null,"height":null,"fps":null,"range":null}"#).unwrap();
    assert_eq!(old.audio, AudioOptions::default());
    let o: AudioOptions = serde_json::from_str(r#"{"format":"m4a","sampleRate":44100,"bitDepth":16,"stems":true}"#).unwrap();
    assert_eq!((o.format, o.sample_rate, o.bit_depth, o.stems), (Some(AudioFormat::Aac), Some(44_100), Some(16), true));
    assert_eq!(AudioFormat::parse("ogg"), Ok(AudioFormat::Vorbis));
    assert!(AudioFormat::parse("mp4").unwrap_err().contains("flac"));
    let mut used = vec![];
    assert_eq!(stem_name(1, "Dialogue / VO", "wav", &mut used), "01-Dialogue _ VO.wav");
    assert_eq!(stem_name(1, "Dialogue / VO", "wav", &mut used), "01-Dialogue _ VO 2.wav");
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
    // No sound file is read; the mix (silence) is still there for players.
    assert!(plan.sources.is_empty(), "{:?}", plan.sources);
    assert!(plan.mix.is_some());
}

#[test]
fn text_clips_last_their_frames() {
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
    // The mix covers the same window; `late` is outside and no file of it is checked twice.
    let mix = plan.mix.unwrap();
    assert_eq!((mix.from, mix.to), (3.0, 6.0));
    assert_eq!(plan.sources, [PathBuf::from("v.mp4")]);
}

#[test]
fn formats_and_fallbacks() {
    let snd = asset(MediaKind::Audio, "a.wav", true);
    let p = project(vec![(TrackKind::Audio, vec![media(&snd, 0.0, 2.0)])], vec![snd]);

    let gif = plan(&p, ExportFormat::Gif);
    assert!(gif.mix.is_none() && input_files(&gif) == ["pipe:0"]);
    assert!(gif.graph.contains("palettegen") && gif.output.join(" ").contains("-c:v gif"));
    // GIF fps is capped.
    assert_eq!(gif.video.unwrap().fps, 15.0);

    let audio = plan(&p, ExportFormat::Audio);
    assert!(audio.graph.is_empty() && audio.output.join(" ").contains("-c:a aac"));
    // Sound only: no filter graph at all, the mix mapped as it is.
    let args = audio.args(Path::new("o.m4a"), None, &caps());
    assert!(!args.iter().any(|a| a.contains("filter_complex")) && args.windows(2).any(|w| w == ["-map", "0:a"]), "{args:?}");
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
    assert_eq!(input_files(&plan)[0], "pipe:0");
    assert!(plan.graph.contains("format=nv12,hwupload[vout]"), "{}", plan.graph);
    assert!(plan.inputs.iter().any(|a| a == "-init_hw_device"));
    let mut st = settings(ExportFormat::Mp4);
    st.encoder = EncoderChoice::Software;
    let cpu = build_with_hardware(&p, &st, &caps(), &hw).unwrap();
    assert!(!cpu.hardware);
    assert_eq!(cpu.encoder.as_deref(), Some("libx264"));
}

#[test]
fn many_clips_from_one_file_are_one_source() {
    let mut v = asset(MediaKind::Video, "-interview.mp4", true);
    let other = asset(MediaKind::Audio, "music.wav", true);
    v.meta.duration = Some(3600.0);
    let clips: Vec<Clip> = (0..300).map(|n| Clip { in_point: ((n * 7919) % 3500) as f64, ..media(&v, n as f64, 1.0) }).collect();
    let p = project(vec![(TrackKind::Video, clips), (TrackKind::Audio, vec![media(&other, 0.0, 2.0)])], vec![v, other]);
    let plan = plan_of(&p);
    assert_eq!(plan.sources, [PathBuf::from("-interview.mp4"), PathBuf::from("music.wav")]);
    // ffmpeg reads the picture pipe and the mix, nothing else.
    assert_eq!(input_files(&plan).len(), 2);
}

fn plan_of(p: &Project) -> Plan {
    plan(p, ExportFormat::Mp4)
}

#[test]
fn ntsc_rates_are_exact_fractions() {
    let text = Clip::new("t", 0.0, 1.0, ClipContent::Text { style: TextStyle::default() });
    let mut p = project(vec![(TrackKind::Video, vec![text])], vec![]);
    p.settings.fps = 29.97;
    let plan = plan(&p, ExportFormat::Mp4);
    assert!(plan.inputs.windows(2).any(|w| w == ["-r", "30000/1001"]), "{:?}", plan.inputs);
    assert_eq!(plan.video.unwrap().fps, 30000.0 / 1001.0);
    assert_eq!(crate::rate(23.976), "24000/1001");
    assert_eq!(crate::rate(59.94), "60000/1001");
    assert_eq!(crate::rate(25.0), "25");
    assert_eq!(crate::snap_fps(24.0), 24.0);
}
