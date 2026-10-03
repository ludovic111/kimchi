//! ffprobe → [`MediaKind`] + [`MediaMeta`].

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::{MediaKind, MediaMeta};
use serde::Deserialize;

use crate::process::Features;
use crate::{MediaError, MediaResult, Tools, process};

pub(crate) const IMAGE_EXTENSIONS: [&str; 13] =
    ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "avif", "jxl", "qoi"];

/// Codecs that only ever hold stills (gif is decided by frame count).
const IMAGE_CODECS: [&str; 16] = [
    "png", "mjpeg", "jpegls", "jpeg2000", "webp", "bmp", "tiff", "jpegxl", "qoi", "ppm", "pgm", "pam", "targa", "sgi",
    "exr", "dpx",
];

/// What ffprobe tells us about a file.
#[derive(Debug, Clone)]
pub struct Probe {
    pub kind: MediaKind,
    pub meta: MediaMeta,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FfProbe {
    #[serde(default)]
    pub streams: Vec<Stream>,
    #[serde(default)]
    pub format: Format,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Format {
    #[serde(default)]
    pub format_name: String,
    pub duration: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Stream {
    #[serde(default)]
    pub codec_type: String,
    #[serde(default)]
    pub codec_name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub avg_frame_rate: Option<String>,
    pub r_frame_rate: Option<String>,
    pub duration: Option<String>,
    pub start_time: Option<String>,
    pub nb_frames: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub color_space: Option<String>,
    pub color_range: Option<String>,
    #[serde(default)]
    pub disposition: HashMap<String, i64>,
    #[serde(default)]
    pub side_data_list: Vec<serde_json::Value>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

impl Stream {
    fn is_picture(&self) -> bool {
        self.codec_type == "video" && self.disposition.get("attached_pic").copied().unwrap_or(0) == 0
    }

    fn rotation(&self) -> i64 {
        let side = self.side_data_list.iter().find_map(|d| d.get("rotation").and_then(|r| r.as_f64()));
        let tag = self.tags.get("rotate").and_then(|r| r.parse::<f64>().ok());
        side.or(tag).map(|r| r.round() as i64).unwrap_or(0)
    }

    fn frames(&self) -> Option<u64> {
        self.nb_frames.as_deref().and_then(|n| n.parse().ok())
    }
}

pub async fn probe(tools: &Tools, path: &Path) -> MediaResult<Probe> {
    let out = process::output(&tools.ffprobe, &streams_args(path))
    .await
    .map_err(|e| match e {
        MediaError::Ffmpeg(msg) => MediaError::Unsupported(format!("{}: {msg}", path.display())),
        e => e,
    })?;
    let info: FfProbe = serde_json::from_slice(&out)
        .map_err(|e| MediaError::Unsupported(format!("{}: unreadable ffprobe output ({e})", path.display())))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    // GIF and APNG don't report a frame count without reading the file; count packets (cheap, no decoding).
    let packets = if matches!(info.format.format_name.as_str(), "gif" | "apng") {
        count_packets(tools, path).await
    } else {
        None
    };
    let kind = classify(&info, &ext, packets)
        .ok_or_else(|| MediaError::Unsupported(format!("{}: no audio or video", path.display())))?;
    let mut meta = describe(&info, kind);
    // Stills: the EXIF orientation, which ffprobe's sizes ignore (see [`Source`]).
    let orientation = match kind {
        MediaKind::Image => process::output(&tools.ffprobe, &orientation_args(path)).await.map(|o| parse_orientation(&o)).unwrap_or(0),
        _ => 0,
    };
    let source = Source::of(&info, orientation);
    (meta.width, meta.height) = (source.width, source.height);
    meta.size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    remember(path, Some(Arc::new(source)));
    Ok(Probe { kind, meta })
}

fn streams_args(path: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "-i"].map(OsString::from).to_vec();
    args.push(input_path(path));
    args
}

/// The first picture's EXIF orientation tag (and nothing else).
fn orientation_args(path: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-v", "error", "-print_format", "json", "-select_streams", "v:0", "-read_intervals", "%+#1"]
        .into_iter()
        .chain(["-show_entries", "frame_tags=Orientation", "-i"])
        .map(OsString::from)
        .collect();
    args.push(input_path(path));
    args
}

/// `{"frames":[{"tags":{"Orientation":"    6"}}]}` → 6; 0 without one.
fn parse_orientation(json: &[u8]) -> u8 {
    let v: serde_json::Value = serde_json::from_slice(json).unwrap_or_default();
    v.pointer("/frames/0/tags/Orientation").and_then(|o| o.as_str()).and_then(|o| o.trim().parse().ok()).filter(|o| (1..=8).contains(o)).unwrap_or(0)
}

/// A file path as an ffmpeg/ffprobe argument: relative paths that look like an option (`-x.mp4`)
/// or a protocol (`a:b.mp4`) get a `./` in front. Non-UTF-8 paths stay as they are.
pub(crate) fn input_path(p: &Path) -> OsString {
    let risky = matches!(p.components().next(), Some(std::path::Component::Normal(first))
        if first.as_encoded_bytes().first() == Some(&b'-') || first.as_encoded_bytes().contains(&b':'));
    if risky { Path::new(".").join(p).into_os_string() } else { p.as_os_str().to_owned() }
}

/// HDR pictures go through `libplacebo` with these options when `zscale` isn't there.
pub(crate) const PLACEBO: &str = "tonemapping=hable:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv:format=yuv420p";

/// What decoding a file's picture takes beyond `-i`, so it comes out upright, with its alpha
/// and in SDR, at the size [`probe`] reports. Found once per file ([`source`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Source {
    /// Upright size of the picture.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// EXIF orientation (2–8) of a still, applied here with `-noautorotate`: ffmpeg builds
    /// disagree on whether they apply it, and ffprobe's size ignores it. 0 when there is none or
    /// the stream has a display matrix (phone videos, HEIC), which ffmpeg applies as usual.
    pub orientation: u8,
    /// `vp8`/`vp9` with an alpha channel (`alpha_mode=1`): only libvpx decodes the alpha.
    pub alpha: Option<&'static str>,
    /// HDR (PQ or HLG): transfer, primaries, matrix, range, as zscale takes them.
    pub hdr: Option<[&'static str; 4]>,
    /// Where the picture stream ends (seconds from the file's start), when the file says;
    /// the sound can go on after it.
    pub video_end: Option<f64>,
}

impl Source {
    pub(crate) fn of(info: &FfProbe, orientation: u8) -> Self {
        let Some(v) = info.streams.iter().find(|s| s.is_picture()) else { return Self::default() };
        let rotated = v.rotation().rem_euclid(180) == 90;
        let orientation = if v.rotation() != 0 || orientation < 2 { 0 } else { orientation };
        let (mut width, mut height) = (v.width, v.height);
        if rotated || orientation >= 5 {
            std::mem::swap(&mut width, &mut height);
        }
        let tag = |k: &str| v.tags.iter().find(|(t, _)| t.eq_ignore_ascii_case(k)).map(|(_, x)| x.trim());
        let alpha = match v.codec_name.as_str() {
            "vp8" if tag("alpha_mode") == Some("1") => Some("vp8"),
            "vp9" if tag("alpha_mode") == Some("1") => Some("vp9"),
            _ => None,
        };
        let pick = |x: &Option<String>, from: &[&'static str], or: &'static str| {
            x.as_deref().and_then(|x| from.iter().copied().find(|f| *f == x)).unwrap_or(or)
        };
        let hdr = matches!(v.color_transfer.as_deref(), Some("smpte2084" | "arib-std-b67")).then(|| {
            [
                pick(&v.color_transfer, &["smpte2084", "arib-std-b67"], "smpte2084"),
                pick(&v.color_primaries, &["bt2020", "bt709", "smpte432", "smpte431"], "bt2020"),
                pick(&v.color_space, &["bt2020nc", "bt2020c", "bt709"], "bt2020nc"),
                pick(&v.color_range, &["tv", "pc"], "tv"),
            ]
        });
        let secs = |s: &str| s.parse::<f64>().ok().filter(|d| d.is_finite() && *d > 0.0);
        // Matroska/WebM keep it in a tag: "00:00:05.000000000".
        let clock = |s: &str| {
            let mut parts = s.rsplit(':');
            let sec: f64 = parts.next()?.parse().ok()?;
            let min: f64 = parts.next().unwrap_or("0").parse().ok()?;
            let hours: f64 = parts.next().unwrap_or("0").parse().ok()?;
            Some(hours * 3600.0 + min * 60.0 + sec).filter(|d| *d > 0.0)
        };
        let length = v.duration.as_deref().and_then(secs).or_else(|| tag("DURATION").and_then(clock));
        let start = v.start_time.as_deref().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let first = info.streams.iter().filter_map(|s| s.start_time.as_deref()?.parse::<f64>().ok()).fold(f64::INFINITY, f64::min);
        let video_end = length.map(|l| (start - if first.is_finite() { first } else { 0.0 }).max(0.0) + l);
        Self { width, height, orientation, alpha, hdr, video_end }
    }

    /// Whether it must be decoded by libvpx (no hardware decoding then).
    pub(crate) fn forces_decoder(&self, f: &Features) -> bool {
        self.decoder(f).is_some()
    }

    fn decoder(&self, f: &Features) -> Option<&'static str> {
        let name = match self.alpha? { "vp9" => "libvpx-vp9", _ => "libvpx" };
        f.decoders.contains(name).then_some(name)
    }

    /// `[-noautorotate] [-c:v libvpx…] -i path` (input seeking goes before).
    pub(crate) fn input(&self, f: &Features, path: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![];
        if self.orientation >= 2 {
            args.push("-noautorotate".into());
        }
        if let Some(d) = self.decoder(f) {
            args.extend(["-c:v".into(), d.into()]);
        }
        args.extend(["-i".into(), input_path(path)]);
        args
    }

    /// Filter turning the decoded picture upright (before any scaling).
    pub(crate) fn upright(&self) -> Option<&'static str> {
        Some(match self.orientation {
            2 => "hflip",
            3 => "hflip,vflip",
            4 => "vflip",
            5 => "transpose=0",
            6 => "transpose=1",
            7 => "transpose=3",
            8 => "transpose=2",
            _ => return None,
        })
    }

    /// Filters tone-mapping HDR to BT.709 SDR (best after scaling down: it works in float).
    pub(crate) fn tonemap(&self, f: &Features) -> Option<String> {
        let [t, p, m, r] = self.hdr?;
        match f.tonemap? {
            "zscale" => Some(format!(
                "zscale=tin={t}:pin={p}:min={m}:rin={r}:t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
                 tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
            )),
            _ => Some(format!("libplacebo={PLACEBO}")),
        }
    }

    /// `upright,scale=…,tonemap,format=rgba`: a picture of exactly `w`×`h` for the compositor.
    pub(crate) fn picture(&self, f: &Features, w: u32, h: u32) -> String {
        let mut v: Vec<String> = self.upright().map(String::from).into_iter().collect();
        v.push(format!("scale={w}:{h}:flags=bicubic,setsar=1"));
        v.extend(self.tonemap(f));
        v.push("format=rgba".into());
        v.join(",")
    }
}

type Sources = Mutex<HashMap<PathBuf, (Option<(u64, std::time::SystemTime)>, Option<Arc<Source>>)>>;

fn sources() -> &'static Sources {
    static S: OnceLock<Sources> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)))
}

fn remember(path: &Path, source: Option<Arc<Source>>) {
    let mut map = sources().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 512 {
        map.clear();
    }
    map.insert(path.to_path_buf(), (stamp(path), source));
}

/// [`Source`] of a file (blocking: ffprobe runs the first time; then cached until it changes).
pub(crate) fn source(tools: &Tools, path: &Path) -> Option<Arc<Source>> {
    if let Some((at, s)) = sources().lock().unwrap_or_else(|e| e.into_inner()).get(path)
        && *at == stamp(path)
    {
        return s.clone();
    }
    let run = |args: Vec<OsString>| {
        process::blocking(&tools.ffprobe).args(args).stderr(std::process::Stdio::null()).output().ok().filter(|o| o.status.success()).map(|o| o.stdout)
    };
    let found = run(streams_args(path)).and_then(|out| serde_json::from_slice::<FfProbe>(&out).ok()).map(|info| {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let orientation = match classify(&info, &ext, None) {
            Some(MediaKind::Image) => run(orientation_args(path)).map(|o| parse_orientation(&o)).unwrap_or(0),
            _ => 0,
        };
        Arc::new(Source::of(&info, orientation))
    });
    remember(path, found.clone());
    found
}

/// [`source`] from async code.
pub(crate) async fn source_async(tools: &Tools, path: &Path) -> Option<Arc<Source>> {
    let (tools, path) = (tools.clone(), path.to_path_buf());
    tokio::task::spawn_blocking(move || source(&tools, &path)).await.ok().flatten()
}

/// [`Features`] from async code.
pub(crate) async fn features_async(tools: &Tools) -> Arc<Features> {
    let tools = tools.clone();
    tokio::task::spawn_blocking(move || Features::get(&tools)).await.unwrap_or_default()
}

async fn count_packets(tools: &Tools, path: &Path) -> Option<u64> {
    let args: [&OsStr; 9] = [
        "-v".as_ref(),
        "error".as_ref(),
        "-count_packets".as_ref(),
        "-select_streams".as_ref(),
        "v:0".as_ref(),
        "-show_entries".as_ref(),
        "stream=nb_read_packets".as_ref(),
        "-of".as_ref(),
        "csv=p=0".as_ref(),
    ];
    let mut args: Vec<OsString> = args.iter().map(|a| a.to_os_string()).collect();
    args.extend(["-i".into(), input_path(path)]);
    let out = process::output(&tools.ffprobe, &args).await.ok()?;
    String::from_utf8_lossy(&out).trim().parse().ok()
}

pub(crate) fn classify(info: &FfProbe, ext: &str, packets: Option<u64>) -> Option<MediaKind> {
    let video = info.streams.iter().find(|s| s.is_picture());
    let has_audio = info.streams.iter().any(|s| s.codec_type == "audio");
    let Some(v) = video else {
        return has_audio.then_some(MediaKind::Audio);
    };
    let format = info.format.format_name.as_str();
    let frames = packets.or(v.frames());
    let single = frames.is_some_and(|n| n <= 1);
    let timeless = info.format.duration.as_deref().and_then(|d| d.parse::<f64>().ok()).is_none_or(|d| d <= 0.05);
    let still = match v.codec_name.as_str() {
        "gif" | "apng" => frames.is_none_or(|n| n <= 1),
        // Motion JPEG & co. in a real container are videos.
        c if IMAGE_CODECS.contains(&c) => {
            format == "image2" || format.ends_with("_pipe") || frames.map_or(timeless, |n| n <= 1)
        }
        // HEIC/AVIF stills live in an ISOBMFF container with a regular video codec.
        _ => !has_audio && (single && IMAGE_EXTENSIONS.contains(&ext) || format.ends_with("_pipe")),
    };
    Some(if still { MediaKind::Image } else { MediaKind::Video })
}

pub(crate) fn describe(info: &FfProbe, kind: MediaKind) -> MediaMeta {
    let video = info.streams.iter().find(|s| s.is_picture());
    let audio = info.streams.iter().find(|s| s.codec_type == "audio");
    let secs =
        |s: &Option<String>| s.as_deref().and_then(|d| d.parse::<f64>().ok()).filter(|d| d.is_finite() && *d > 0.0);
    let duration = match kind {
        MediaKind::Image => None,
        _ => secs(&info.format.duration)
            .or_else(|| info.streams.iter().filter_map(|s| secs(&s.duration)).reduce(f64::max)),
    };
    let (mut width, mut height) = (video.and_then(|v| v.width), video.and_then(|v| v.height));
    if video.is_some_and(|v| v.rotation().rem_euclid(180) == 90) {
        std::mem::swap(&mut width, &mut height);
    }
    let fps = match kind {
        MediaKind::Video => video.and_then(|v| {
            let rate = |r: &Option<String>| r.as_deref().and_then(parse_rate).filter(|f| *f > 0.0 && *f < 1000.0);
            rate(&v.avg_frame_rate).or_else(|| rate(&v.r_frame_rate))
        }),
        _ => None,
    };
    MediaMeta {
        duration,
        width,
        height,
        fps,
        has_video: video.is_some(),
        has_audio: audio.is_some(),
        video_codec: video.map(|v| v.codec_name.clone()),
        audio_codec: audio.map(|a| a.codec_name.clone()),
        size_bytes: 0,
    }
}

fn parse_rate(r: &str) -> Option<f64> {
    match r.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            (d != 0.0).then(|| n / d)
        }
        None => r.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> FfProbe {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn classifies_common_files() {
        let png = parse(
            r#"{"streams":[{"codec_type":"video","codec_name":"png","width":64,"height":32}],"format":{"format_name":"png_pipe"}}"#,
        );
        assert_eq!(classify(&png, "png", None), Some(MediaKind::Image));
        let mp4 = parse(
            r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30000/1001","nb_frames":"300",
                "side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}]},
               {"codec_type":"audio","codec_name":"aac"}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"10.01"}}"#,
        );
        assert_eq!(classify(&mp4, "mp4", None), Some(MediaKind::Video));
        let meta = describe(&mp4, MediaKind::Video);
        assert_eq!((meta.width, meta.height), (Some(1080), Some(1920)));
        assert!((meta.fps.unwrap() - 29.97).abs() < 0.01);
        assert_eq!(meta.duration, Some(10.01));
        assert!(meta.has_audio && meta.has_video);
        let gif = parse(
            r#"{"streams":[{"codec_type":"video","codec_name":"gif"}],"format":{"format_name":"gif","duration":"1.2"}}"#,
        );
        assert_eq!(classify(&gif, "gif", Some(12)), Some(MediaKind::Video));
        assert_eq!(classify(&gif, "gif", Some(1)), Some(MediaKind::Image));
        let mp3 = parse(
            r#"{"streams":[{"codec_type":"audio","codec_name":"mp3"},{"codec_type":"video","codec_name":"mjpeg","disposition":{"attached_pic":1}}],
               "format":{"format_name":"mp3","duration":"3.5"}}"#,
        );
        assert_eq!(classify(&mp3, "mp3", None), Some(MediaKind::Audio));
        assert!(!describe(&mp3, MediaKind::Audio).has_video);
        let heic = parse(
            r#"{"streams":[{"codec_type":"video","codec_name":"hevc","nb_frames":"1"}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2"}}"#,
        );
        assert_eq!(classify(&heic, "heic", None), Some(MediaKind::Image));
        assert_eq!(classify(&parse(r#"{"streams":[]}"#), "txt", None), None);
    }

    #[test]
    fn reads_what_decoding_needs() {
        // A portrait phone JPEG: stored 400×200, EXIF says turn it 90° clockwise.
        let jpeg = parse(r#"{"streams":[{"codec_type":"video","codec_name":"mjpeg","width":400,"height":200}],"format":{"format_name":"image2"}}"#);
        let s = Source::of(&jpeg, 6);
        assert_eq!((s.width, s.height, s.upright()), (Some(200), Some(400), Some("transpose=1")));
        assert_eq!(Source::of(&jpeg, 3).width, Some(400));
        assert_eq!(Source::of(&jpeg, 1), Source { width: Some(400), height: Some(200), ..Source::default() });
        let f = Features { decoders: ["libvpx-vp9".to_string()].into(), tonemap: Some("zscale") };
        let input = Source::of(&jpeg, 8).input(&f, Path::new("-a.jpg"));
        assert_eq!(input, [OsString::from("-noautorotate"), "-i".into(), Path::new(".").join("-a.jpg").into()]);
        // A display matrix (phone video, HEIC) is ffmpeg's to apply; EXIF is ignored then.
        let heic = parse(r#"{"streams":[{"codec_type":"video","codec_name":"hevc","width":400,"height":200,"side_data_list":[{"rotation":-90}]}],"format":{"format_name":"mov"}}"#);
        assert_eq!((Source::of(&heic, 6).orientation, Source::of(&heic, 6).width), (0, Some(200)));

        // VP9 with alpha decodes with libvpx (when the build has it); HDR is tone-mapped.
        let webm = parse(r#"{"streams":[{"codec_type":"video","codec_name":"vp9","width":64,"height":64,"start_time":"0.000","tags":{"alpha_mode":"1","DURATION":"00:00:01.500000000"}},
            {"codec_type":"audio","codec_name":"opus","start_time":"-0.007","tags":{"DURATION":"00:00:04.000000000"}}],"format":{"format_name":"matroska,webm","duration":"4.0"}}"#);
        let s = Source::of(&webm, 0);
        assert_eq!(s.alpha, Some("vp9"));
        assert!((s.video_end.unwrap() - 1.507).abs() < 1e-9, "{s:?}");
        assert_eq!(s.input(&f, Path::new("/a.webm")), ["-c:v", "libvpx-vp9", "-i", "/a.webm"].map(OsString::from));
        assert_eq!(s.input(&Features::default(), Path::new("a.webm")), ["-i", "a.webm"].map(OsString::from));
        let hlg = parse(r#"{"streams":[{"codec_type":"video","codec_name":"hevc","width":64,"height":36,"duration":"2.0","color_transfer":"arib-std-b67","color_primaries":"bt2020","color_space":"bt2020nc","color_range":"tv"}],"format":{"format_name":"mov","duration":"2.0"}}"#);
        let s = Source::of(&hlg, 0);
        assert_eq!(s.video_end, Some(2.0));
        let chain = s.picture(&f, 32, 18);
        assert!(chain.starts_with("scale=32:18:flags=bicubic,setsar=1,zscale=tin=arib-std-b67:pin=bt2020:min=bt2020nc:rin=tv:t=linear"), "{chain}");
        assert!(chain.ends_with("tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p,format=rgba"), "{chain}");
        assert_eq!(s.picture(&Features::default(), 32, 18), "scale=32:18:flags=bicubic,setsar=1,format=rgba");

        assert_eq!(parse_orientation(br#"{"frames":[{"tags":{"Orientation":"    6"}}]}"#), 6);
        assert_eq!(parse_orientation(br#"{"frames":[{}]}"#), 0);
        assert_eq!(input_path(Path::new("a:b.mp4")), Path::new(".").join("a:b.mp4").into_os_string());
        assert_eq!(input_path(Path::new("/x/-a.mp4")), OsString::from("/x/-a.mp4"));
    }
}
