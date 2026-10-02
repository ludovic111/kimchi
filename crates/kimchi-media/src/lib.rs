//! kimchi-media: everything that touches media files, via ffmpeg/ffprobe.

pub mod export;
pub mod text;
mod probe;
mod process;

use std::path::{Path, PathBuf};

use kimchi_core::{Filmstrip, MediaKind, MediaMeta, Waveform};
use thiserror::Error;
use tokio::io::AsyncReadExt;

pub use probe::{Probe, probe};
pub use process::Caps;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("ffmpeg wasn't found. Install it (e.g. `brew install ffmpeg`) or set KIMCHI_FFMPEG.")]
    ToolsMissing,
    #[error("unsupported file: {0}")]
    Unsupported(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type MediaResult<T> = Result<T, MediaError>;

/// Paths to the ffmpeg binaries.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    /// Finds ffmpeg/ffprobe: `KIMCHI_FFMPEG`/`KIMCHI_FFPROBE`, next to the
    /// executable (bundled sidecars), then `PATH` and common install locations.
    pub fn locate() -> MediaResult<Self> {
        let ffmpeg = find_tool("ffmpeg", "KIMCHI_FFMPEG").ok_or(MediaError::ToolsMissing)?;
        let ffprobe = find_tool("ffprobe", "KIMCHI_FFPROBE").ok_or(MediaError::ToolsMissing)?;
        Ok(Self { ffmpeg, ffprobe })
    }
}

fn find_tool(name: &str, env: &str) -> Option<PathBuf> {
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let mut candidates: Vec<PathBuf> = std::env::var_os(env).map(PathBuf::from).into_iter().collect();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        // The app bundles its own copy as `kimchi-ffmpeg` so Linux packages don't clash with a
        // system ffmpeg; Tauri keeps a target-triple suffix in dev builds (`kimchi-ffmpeg-aarch64-apple-darwin`).
        for prefix in [format!("kimchi-{name}"), name.to_string()] {
            candidates.push(dir.join(format!("{prefix}{}", std::env::consts::EXE_SUFFIX)));
            if let Ok(entries) = std::fs::read_dir(&dir) {
                let mut sidecars: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{prefix}-"))))
                    .collect();
                sidecars.sort();
                candidates.extend(sidecars);
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(&exe)));
    }
    candidates.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(|d| Path::new(d).join(&exe)));
    candidates.into_iter().find(|p| p.is_file() && runs(p))
}

fn runs(path: &Path) -> bool {
    let mut cmd = std::process::Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash on Windows
    }
    cmd.arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `ffmpeg -y -v error <args>` and nothing else.
async fn ffmpeg(tools: &Tools, args: &[String]) -> MediaResult<Vec<u8>> {
    let mut all = vec!["-hide_banner".into(), "-nostdin".into(), "-y".into(), "-v".into(), "error".into()];
    all.extend_from_slice(args);
    process::output(&tools.ffmpeg, &all).await
}

fn s(x: impl ToString) -> String {
    x.to_string()
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn made(out: &Path) -> bool {
    std::fs::metadata(out).is_ok_and(|m| m.len() > 0)
}

/// Poster frame (JPEG) at most `max_width` wide. For audio, writes nothing and returns false.
pub async fn thumbnail(tools: &Tools, path: &Path, kind: MediaKind, out: &Path, max_width: u32) -> MediaResult<bool> {
    if kind == MediaKind::Audio {
        return Ok(false);
    }
    let at = match kind {
        MediaKind::Video => probe(tools, path).await?.meta.duration.unwrap_or(0.0) * 0.1,
        _ => 0.0,
    };
    let filter = format!("scale=w='min(iw,{})':h=-2,format=yuvj420p", max_width.max(2));
    let attempt = |seek: Option<f64>| {
        let mut args = vec![];
        if let Some(t) = seek {
            args.extend([s("-ss"), format!("{t:.3}")]);
        }
        args.extend([s("-i"), path_arg(path), s("-frames:v"), s("1"), s("-vf"), filter.clone()]);
        args.extend([s("-q:v"), s("4"), s("-update"), s("1"), s("-f"), s("image2"), path_arg(out)]);
        args
    };
    let _ = std::fs::remove_file(out);
    ffmpeg(tools, &attempt((at > 0.0).then_some(at))).await?;
    if !made(out) && at > 0.0 {
        ffmpeg(tools, &attempt(None)).await?;
    }
    if !made(out) {
        return Err(MediaError::Unsupported(format!("no picture in {}", path.display())));
    }
    Ok(true)
}

/// Horizontal strip of frames (JPEG) `height` px tall, for drawing clips on the timeline.
pub async fn filmstrip(tools: &Tools, path: &Path, duration: f64, out: &Path, height: u32) -> MediaResult<Filmstrip> {
    let meta = probe(tools, path).await?.meta;
    let (w, h) = (meta.width.unwrap_or(16), meta.height.unwrap_or(9).max(1));
    let frame_height = height.max(2);
    let frame_width = even((frame_height as f64 * w as f64 / h as f64).round() as u32);
    let duration = if duration > 0.0 { duration } else { meta.duration.unwrap_or(1.0) };
    let interval = (duration / 120.0).max(0.5);
    let frames = ((duration / interval).ceil() as u32).clamp(1, 120);
    let mut args = vec![];
    if interval >= 4.0 {
        // Long clip: decoding only keyframes is much faster and precise enough at this zoom.
        args.extend([s("-skip_frame"), s("nokey")]);
    }
    args.extend([s("-an"), s("-sn"), s("-i"), path_arg(path)]);
    let vf = format!(
        "fps=1/{interval:.4}:round=down,scale={frame_width}:{frame_height},setsar=1,tile={frames}x1,format=yuvj420p"
    );
    args.extend([
        s("-vf"),
        vf,
        s("-frames:v"),
        s("1"),
        s("-q:v"),
        s("5"),
        s("-update"),
        s("1"),
        s("-f"),
        s("image2"),
        path_arg(out),
    ]);
    let _ = std::fs::remove_file(out);
    ffmpeg(tools, &args).await?;
    if !made(out) {
        return Err(MediaError::Unsupported(format!("no picture in {}", path.display())));
    }
    Ok(Filmstrip { path: path_arg(out), frames, frame_width, frame_height, interval })
}

/// Audio peaks (little-endian f32 in 0..1) at `peaks_per_second`.
pub async fn waveform(tools: &Tools, path: &Path, out: &Path, peaks_per_second: u32) -> MediaResult<Waveform> {
    let pps = peaks_per_second.max(1);
    // A low decode rate keeps this fast; enough samples per bucket to find the peak.
    let rate = (pps * 64).clamp(4_000, 16_000);
    let args = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-i",
        &path_arg(path),
        "-map",
        "0:a:0",
        "-ac",
        "1",
        "-ar",
        &rate.to_string(),
        "-f",
        "f32le",
        "-",
    ];
    let mut child = process::spawn(&tools.ffmpeg, &args, true)?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let stderr = process::collect_stderr(&mut child);

    let mut peaks: Vec<f32> = vec![];
    let (mut buf, mut carry, mut index) = (vec![0u8; 64 * 1024], Vec::<u8>::with_capacity(4), 0u64);
    loop {
        let n = stdout.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        // Reads don't respect sample boundaries: finish the sample split across two reads first.
        let mut chunk = &buf[..n];
        if !carry.is_empty() {
            let take = (4 - carry.len()).min(chunk.len());
            carry.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
            let Ok(sample) = <[u8; 4]>::try_from(carry.as_slice()) else {
                continue;
            };
            push_peak(&mut peaks, index, pps, rate, f32::from_le_bytes(sample));
            index += 1;
            carry.clear();
        }
        let (samples, rest) = chunk.as_chunks::<4>();
        for sample in samples {
            push_peak(&mut peaks, index, pps, rate, f32::from_le_bytes(*sample));
            index += 1;
        }
        carry.extend_from_slice(rest);
    }
    let status = child.wait().await?;
    let stderr = stderr.await.unwrap_or_default();
    if !status.success() {
        return Err(MediaError::Ffmpeg(process::summarize(&stderr, status)));
    }
    if index == 0 {
        return Err(MediaError::Unsupported(format!("no audio in {}", path.display())));
    }
    let max = peaks.iter().copied().fold(0.0f32, f32::max);
    let bytes: Vec<u8> =
        peaks.iter().flat_map(|p| if max > 0.0 { (p / max).min(1.0) } else { 0.0 }.to_le_bytes()).collect();
    tokio::fs::write(out, bytes).await?;
    Ok(Waveform { path: path_arg(out), peaks_per_second: pps })
}

fn push_peak(peaks: &mut Vec<f32>, index: u64, pps: u32, rate: u32, sample: f32) {
    let bucket = (index * pps as u64 / rate as u64) as usize;
    if peaks.len() <= bucket {
        peaks.resize(bucket + 1, 0.0);
    }
    let v = if sample.is_finite() { sample.abs() } else { 0.0 };
    peaks[bucket] = peaks[bucket].max(v);
}

/// Whether the webview can't play this file directly and needs a proxy.
///
/// Conservative: anything not known to play in WebKit (macOS/Linux) and
/// WebView2 (Windows) `<video>`/`<audio>` gets a proxy.
pub fn needs_proxy(meta: &MediaMeta, path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if probe::IMAGE_EXTENSIONS.contains(&ext.as_str()) && ext != "gif" {
        return false;
    }
    let mac = cfg!(target_os = "macos");
    let vcodec = meta.video_codec.as_deref().unwrap_or("");
    let acodec = meta.audio_codec.as_deref().unwrap_or("");
    if meta.has_video {
        if meta.duration.is_none() && vcodec != "gif" {
            return false; // a still
        }
        let (container_ok, video_ok) = match ext.as_str() {
            "mp4" | "m4v" | "mov" => (true, vcodec == "h264" || (mac && vcodec == "hevc")),
            // WebKit on macOS is unreliable with WebM; Chromium/GStreamer handle it.
            "webm" => (!mac, matches!(vcodec, "vp8" | "vp9" | "av1")),
            _ => (false, false),
        };
        let audio_ok = !meta.has_audio
            || match acodec {
                "aac" | "mp3" => ext != "webm",
                "opus" | "vorbis" => ext == "webm",
                "alac" => mac && ext != "webm",
                c if c.starts_with("pcm_") => mac && ext == "mov",
                _ => false,
            };
        return !(container_ok && video_ok && audio_ok);
    }
    if !meta.has_audio {
        return false;
    }
    let ok = match (ext.as_str(), acodec) {
        ("mp3", "mp3") | ("m4a" | "mp4" | "aac", "aac") | ("flac", "flac") => true,
        ("wav", "pcm_s16le" | "pcm_u8" | "pcm_s24le" | "pcm_f32le") => true,
        ("ogg" | "oga" | "opus" | "webm", "opus" | "vorbis") => !mac,
        ("m4a" | "caf", "alac") | ("aif" | "aiff", _) => mac,
        _ => false,
    };
    !ok
}

/// H.264/AAC MP4 proxy for preview playback.
pub async fn proxy(tools: &Tools, path: &Path, out: &Path) -> MediaResult<()> {
    let meta = probe(tools, path).await?.meta;
    let caps = Caps::detect(tools).await?;
    let mut args = vec![s("-i"), path_arg(path), s("-sn"), s("-dn")];
    if meta.has_video {
        // Fit within 1920x1080 (1080x1920 for portrait) without upscaling.
        let vf = "scale=w='if(gte(iw,ih),min(1920,iw),min(1080,iw))':h='if(gte(iw,ih),min(1080,ih),min(1920,ih))':\
                  force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1,format=yuv420p";
        args.extend([s("-map"), s("0:v:0"), s("-vf"), s(vf)]);
        args.extend(export::h264_args(&caps, "veryfast", 23, 5_000_000)?);
    }
    if meta.has_audio {
        args.extend([s("-map"), s("0:a:0"), s("-c:a"), s("aac"), s("-b:a"), s("160k"), s("-ac"), s("2")]);
    }
    if !meta.has_video && !meta.has_audio {
        return Err(MediaError::Unsupported(format!("nothing to play in {}", path.display())));
    }
    args.extend([s("-movflags"), s("+faststart"), s("-f"), s("mp4"), path_arg(out)]);
    if let Err(e) = ffmpeg(tools, &args).await {
        let _ = std::fs::remove_file(out);
        return Err(e);
    }
    Ok(())
}

/// Full-resolution PNG of the frame at `time` seconds.
pub async fn grab_frame(tools: &Tools, path: &Path, time: f64, out: &Path) -> MediaResult<()> {
    // `-update 1` keeps overwriting the file, so without `-frames:v 1` the last decoded frame wins.
    let attempt = |seek: &[&str], first_only: bool| {
        let mut args: Vec<String> = seek.iter().map(s).collect();
        args.extend([s("-i"), path_arg(path)]);
        if first_only {
            args.extend([s("-frames:v"), s("1")]);
        }
        args.extend([s("-update"), s("1"), s("-f"), s("image2"), s("-c:v"), s("png"), path_arg(out)]);
        args
    };
    let _ = std::fs::remove_file(out);
    if time > 0.0 {
        // Input-side -ss is frame-accurate when transcoding.
        ffmpeg(tools, &attempt(&["-ss", &format!("{time:.6}")], true)).await?;
        if !made(out) {
            // Past the end: take the last frame instead.
            let _ = ffmpeg(tools, &attempt(&["-sseof", "-0.5"], false)).await;
        }
    }
    if !made(out) {
        ffmpeg(tools, &attempt(&[], true)).await?;
    }
    if !made(out) {
        return Err(MediaError::Unsupported(format!("no picture in {}", path.display())));
    }
    Ok(())
}

fn even(x: u32) -> u32 {
    (x.max(2) / 2) * 2
}
