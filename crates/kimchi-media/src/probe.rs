//! ffprobe → [`MediaKind`] + [`MediaMeta`].

use std::collections::HashMap;
use std::path::Path;

use kimchi_core::{MediaKind, MediaMeta};
use serde::Deserialize;

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
    pub nb_frames: Option<String>,
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
    let out = process::output(
        &tools.ffprobe,
        &[
            "-v".as_ref(),
            "error".as_ref(),
            "-print_format".as_ref(),
            "json".as_ref(),
            "-show_format".as_ref(),
            "-show_streams".as_ref(),
            path.as_os_str(),
        ],
    )
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
    meta.size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(Probe { kind, meta })
}

async fn count_packets(tools: &Tools, path: &Path) -> Option<u64> {
    let args: [&std::ffi::OsStr; 9] = [
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
    let mut args = args.to_vec();
    args.push(path.as_os_str());
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
}
