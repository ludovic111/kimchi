//! Renders a project to a file: the pictures come from the compositor ([`crate::render`]) as raw
//! frames on ffmpeg's standard input, the sound from kimchi-audio's mixer ([`crate::audio`]),
//! mixed first to a temporary file (much faster than real time) that ffmpeg encodes beside the
//! pictures. ffmpeg decodes each clip's sound for the mixer and encodes; it mixes nothing.
//!
//! [`build`] is pure: it turns a project into a [`Plan`] (inputs, graph, output options).
//! [`export`] mixes the sound, runs that plan feeding it frames, reports progress and handles
//! cancellation. Sound-only exports take their file type, sample rate, bit depth and bitrate
//! from [`AudioOptions`]; with `stems` they write one file per track and bus into a folder.
//!
//! Graph shape: input 0 is the rendered picture (`[0:v]`), converted to the encoder's format
//! with BT.709 colours; the mix is the last input, raw f32 stereo, mapped as it is.

use std::path::{Path, PathBuf};

use kimchi_audio::mixer::Selection;
use kimchi_core::{Project, ProjectSettings};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::accel::{self, Codec, EncoderChoice, Hardware, Verified};
use crate::{Caps, MediaError, MediaResult, Tools, process};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// H.264 + AAC in MP4. Plays everywhere.
    Mp4,
    /// HEVC + AAC in MP4. Smaller files.
    Hevc,
    /// ProRes 422 HQ + PCM in MOV, for further editing.
    Prores,
    /// VP9 + Opus in WebM.
    Webm,
    /// Animated GIF (no audio).
    Gif,
    /// Audio only (AAC in M4A).
    Audio,
    /// Audio only, uncompressed (24-bit PCM WAV), e.g. to score the cut in ryolune.
    Wav,
}

impl ExportFormat {
    /// Formats with no picture.
    pub fn is_audio_only(self) -> bool {
        matches!(self, ExportFormat::Audio | ExportFormat::Wav)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Draft,
    Standard,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportSettings {
    pub path: String,
    pub format: ExportFormat,
    pub quality: Quality,
    /// Output size; defaults to the project size.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// Only render this range (seconds).
    pub range: Option<(f64, f64)>,
    #[serde(default)]
    pub encoder: EncoderChoice,
    /// The sound: file type of sound-only exports, sample rate, bit depth, bitrate, stems.
    #[serde(default)]
    pub audio: AudioOptions,
}

/// How an export's sound is written. Every field has a default, so older settings read as
/// before: video exports keep their codec's sound (AAC, PCM, Opus) at the project's rate;
/// `audio` exports AAC in M4A and `wav` 24-bit WAV.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioOptions {
    /// The file type of sound-only exports (`format: audio` or `wav`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<AudioFormat>,
    /// 44100, 48000 or 96000 Hz; the project's rate by default (Opus is always 48 kHz).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// 16 or 24-bit integers, or 32-bit float (WAV only), for WAV, AIFF and FLAC (and the PCM of
    /// ProRes exports). 24 by default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u32>,
    /// kbit/s of lossy sound (MP3, AAC, Opus, Vorbis); by quality otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    /// One file per track with sound and per bus, into the folder `path` names (sound-only
    /// exports).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stems: bool,
    /// Stems go through the master's effects, fader and limiter too (by default they don't).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stems_master: bool,
    /// Loudness target for this export (LUFS), instead of the master's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loudness: Option<f64>,
}

/// File types of sound-only exports.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    /// Uncompressed PCM (16/24-bit or 32-bit float).
    Wav,
    /// Uncompressed PCM, big-endian (16/24-bit).
    Aiff,
    /// Lossless, about half the size (16/24-bit).
    Flac,
    /// MP3 (needs an ffmpeg with libmp3lame).
    Mp3,
    /// AAC in M4A.
    #[serde(alias = "m4a")]
    Aac,
    /// Opus in Ogg (48 kHz).
    Opus,
    /// Vorbis in Ogg.
    #[serde(alias = "ogg")]
    Vorbis,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 7] = [Self::Wav, Self::Aiff, Self::Flac, Self::Mp3, Self::Aac, Self::Opus, Self::Vorbis];

    /// The file extension it is written with.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Aac => "m4a",
            Self::Opus => "opus",
            Self::Vorbis => "ogg",
        }
    }

    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Mp3 | Self::Aac | Self::Opus | Self::Vorbis)
    }

    /// Reads `wav`, `aiff`, `flac`, `mp3`, `aac` / `m4a`, `opus`, `vorbis` / `ogg`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wav" | "wave" => Ok(Self::Wav),
            "aiff" | "aif" => Ok(Self::Aiff),
            "flac" => Ok(Self::Flac),
            "mp3" => Ok(Self::Mp3),
            "aac" | "m4a" => Ok(Self::Aac),
            "opus" => Ok(Self::Opus),
            "vorbis" | "ogg" => Ok(Self::Vorbis),
            other => Err(format!("audio formats are wav, aiff, flac, mp3, aac (m4a), opus and vorbis (ogg), not \"{other}\"")),
        }
    }
}

/// Sample rates sound can be exported at.
pub const AUDIO_RATES: [u32; 3] = [44_100, 48_000, 96_000];

/// How an export was encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Exported {
    /// The ffmpeg video encoder (`h264_videotoolbox`, `libx264`), `None` for sound only.
    pub encoder: Option<String>,
    /// It ran on the GPU or media engine.
    pub hardware: bool,
    /// The hardware encode failed and the export was redone on the CPU.
    pub fell_back: bool,
}

/// Graphs longer than this go through a script file instead of argv.
pub(crate) const INLINE_GRAPH_MAX: usize = 4_000;

/// Sample rate of the sound speech recognition hears (what Whisper listens to).
pub const SPEECH_SAMPLE_RATE: u32 = 16_000;

/// The clip-speed stages of ffmpeg's `atempo` (kept here for callers of the old export graph).
pub use crate::audio::atempo;

/// Renders `project`. `progress` receives 0.0–1.0.
pub async fn export(
    tools: &Tools,
    project: &Project,
    settings: &ExportSettings,
    progress: impl Fn(f64) + Send + Sync,
    cancel: CancellationToken,
) -> MediaResult<Exported> {
    if settings.audio.stems {
        return export_stems(tools, project, settings, &progress, &cancel).await;
    }
    let caps = Caps::detect(tools).await?;
    let hw = hardware_for(tools, &caps, settings).await;
    let plan = build_with_hardware(project, settings, &caps, &hw)?;
    if let Some(missing) = plan.sources.iter().find(|p| !p.exists()) {
        return Err(MediaError::Unsupported(format!("missing media file {}", missing.display())));
    }
    let out = PathBuf::from(&settings.path);
    let name = out.file_name().ok_or_else(|| MediaError::Unsupported(format!("bad output path {}", settings.path)))?;
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(dir).await?;
    }
    // Render next to the target and rename at the end, so a failed or cancelled
    // export never leaves a truncated file under the real name.
    let part = out.with_file_name(format!(".{}.part", name.to_string_lossy()));
    progress(0.0);
    // The sound first: the mixer renders it to a temporary file, then ffmpeg encodes it with
    // the pictures. Mixing takes a small share of a video export's time, most of a sound-only one.
    let share = match (&plan.mix, plan.video.is_some()) {
        (None, _) => 0.0,
        (Some(_), true) => 0.1,
        (Some(_), false) => 0.7,
    };
    if let Some(mix) = &plan.mix {
        let mixed = mix_for(tools, project, settings, mix, Selection::default(), &|p| progress(p * share), &cancel).await;
        if let Err(e) = mixed {
            let _ = tokio::fs::remove_file(&mix.path).await;
            return Err(e);
        }
    }
    let progress = |p: f64| progress(share + (1.0 - share) * p);
    let mut used = (plan.encoder.clone(), plan.hardware, false);
    let mut result = attempt(tools, project, &plan, &part, &caps, settings.encoder != EncoderChoice::Software, &progress, &cancel).await;
    // Only a failed hardware encoder is worth redoing on the CPU; a picture that can't be
    // rendered or decoded would fail the same way.
    if let Err(Failure::Encoder(e)) = &result
        && plan.hardware
        && settings.encoder == EncoderChoice::Auto
    {
        tracing::warn!(encoder = ?plan.encoder, error = %e, "hardware encode failed; encoding on the CPU");
        let mut cpu = build_with_hardware(project, settings, &caps, &Hardware::none())?;
        // Same sound: the mix already made is used again.
        if let (Some(again), Some(made)) = (&mut cpu.mix, &plan.mix) {
            cpu.inputs.iter_mut().filter(|a| **a == path(&again.path)).for_each(|a| *a = path(&made.path));
            again.path = made.path.clone();
        }
        progress(0.0);
        used = (cpu.encoder.clone(), false, true);
        result = attempt(tools, project, &cpu, &part, &caps, false, &progress, &cancel).await;
    }
    if let Some(mix) = &plan.mix {
        let _ = tokio::fs::remove_file(&mix.path).await;
    }
    let done = match result {
        Ok(()) => tokio::fs::rename(&part, &out).await.map_err(MediaError::from),
        Err(Failure::Encoder(e) | Failure::Other(e)) => Err(e),
    };
    if let Err(e) = done {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    progress(1.0);
    Ok(Exported { encoder: used.0, hardware: used.1, fell_back: used.2 })
}

/// Why an export attempt failed.
enum Failure {
    /// ffmpeg (the encoder and muxer) gave up.
    Encoder(MediaError),
    /// Anything else: rendering, decoding, cancelling, the file system.
    Other(MediaError),
}

impl From<MediaError> for Failure {
    fn from(e: MediaError) -> Self {
        Failure::Other(e)
    }
}

/// A temporary file for a long filter graph. ffmpeg reads its path from its arguments, which
/// are UTF-8 here: a temporary folder whose path isn't is reported rather than mangled.
pub(crate) fn script_path(what: &str) -> MediaResult<PathBuf> {
    let path = std::env::temp_dir().join(format!("kimchi-{what}-{}.txt", kimchi_core::new_id()));
    if path.to_str().is_none() {
        return Err(MediaError::Unsupported(format!(
            "the temporary folder's path ({}) isn't valid UTF-8; point TMPDIR (TEMP on Windows) at another folder",
            path.display()
        )));
    }
    Ok(path)
}

/// The video encoder an export of `project` with `settings` would start with (`None` for sound
/// only), as [`export`] picks it: for showing before or while it runs.
pub async fn planned_encoder(tools: &Tools, project: &Project, settings: &ExportSettings) -> MediaResult<Option<String>> {
    let caps = Caps::detect(tools).await?;
    let hw = hardware_for(tools, &caps, settings).await;
    let (w, h) = output_size(&project.settings, settings);
    let codecs = Codecs::pick(settings.format, settings.quality, &caps, &hw, settings.encoder, w, h, output_fps(&project.settings, settings))?;
    Ok(codecs.encoder)
}

/// What each format would be encoded with here, per [`EncoderChoice`] (at 1080p30).
pub async fn encoders(tools: &Tools) -> MediaResult<(Hardware, Vec<FormatEncoders>)> {
    let caps = Caps::detect(tools).await?;
    let hw = Hardware::detect(tools, &caps).await;
    let formats = [ExportFormat::Mp4, ExportFormat::Hevc, ExportFormat::Prores, ExportFormat::Webm, ExportFormat::Gif]
        .into_iter()
        .map(|format| {
            let with = |choice| {
                Codecs::pick(format, Quality::Standard, &caps, &hw, choice, 1920, 1080, 30.0).ok().and_then(|c| c.encoder)
            };
            FormatEncoders {
                format,
                auto: with(EncoderChoice::Auto),
                hardware: with(EncoderChoice::Hardware),
                software: with(EncoderChoice::Software),
            }
        })
        .collect();
    Ok((hw, formats))
}

/// Video encoders for one format; `None` where there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FormatEncoders {
    pub format: ExportFormat,
    pub auto: Option<String>,
    pub hardware: Option<String>,
    pub software: Option<String>,
}

/// The hardware an export may use: none for sound, or when the CPU was asked for.
async fn hardware_for(tools: &Tools, caps: &Caps, settings: &ExportSettings) -> Hardware {
    if settings.encoder == EncoderChoice::Software || settings.format.is_audio_only() || settings.format == ExportFormat::Gif {
        return Hardware::none();
    }
    Hardware::detect(tools, caps).await
}

#[allow(clippy::too_many_arguments)]
async fn attempt(
    tools: &Tools, project: &Project, plan: &Plan, part: &Path, caps: &Caps, decode_hardware: bool,
    progress: &(impl Fn(f64) + Sync), cancel: &CancellationToken,
) -> Result<(), Failure> {
    let script = (plan.graph.len() > INLINE_GRAPH_MAX).then(|| script_path("graph")).transpose()?;
    if let Some(script) = &script {
        tokio::fs::write(script, &plan.graph).await.map_err(MediaError::from)?;
    }
    let result = run(tools, project, plan, &plan.args(part, script.as_deref(), caps), decode_hardware.then_some(caps), progress, cancel).await;
    if let Some(script) = &script {
        let _ = tokio::fs::remove_file(script).await;
    }
    result
}

async fn run(
    tools: &Tools,
    project: &Project,
    plan: &Plan,
    args: &[String],
    decode_caps: Option<&Caps>,
    progress: &(impl Fn(f64) + Sync),
    cancel: &CancellationToken,
) -> Result<(), Failure> {
    let mut child = process::spawn_with_stdin(&tools.ffmpeg, args, true, plan.video.is_some())?;
    let stderr = process::collect_stderr(&mut child);
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    let stdin = child.stdin.take();
    let duration = plan.duration;
    // `-progress pipe:1` prints key=value blocks; out_time_us is how far the muxer got.
    let watch = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                progress((us / 1e6 / duration.max(1e-6)).clamp(0.0, 0.999));
            }
        }
    };
    // The picture: frames from the compositor, written to ffmpeg's stdin.
    let feed = async {
        let (Some(video), Some(mut stdin)) = (plan.video, stdin) else { return Ok(()) };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<MediaResult<Vec<u8>>>(3);
        let (tools, project) = (tools.clone(), project.clone());
        let decode_caps = decode_caps.cloned();
        let render = tokio::task::spawn_blocking(move || {
            let mut r = crate::render::Renderer::for_export(&tools, &project, video.width, video.height, video.fps);
            if let Some(caps) = decode_caps { r = r.with_hardware_decoding(caps); }
            for n in 0..video.frames {
                let t = video.from + n as f64 / video.fps;
                let frame = r.frame(t).map(crate::render::to_rgba);
                if tx.blocking_send(frame).is_err() {
                    return; // cancelled or ffmpeg gone
                }
            }
        });
        let mut result = Ok(());
        while let Some(frame) = rx.recv().await {
            match frame {
                Ok(bytes) => {
                    if stdin.write_all(&bytes).await.is_err() {
                        break; // ffmpeg stopped reading: its exit status says why
                    }
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        // Close both ends so the renderer (blocked on a full channel) and ffmpeg finish.
        drop(rx);
        drop(stdin);
        let _ = render.await;
        result
    };
    let status = tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        r = async {
            let (_, fed) = tokio::join!(watch, feed);
            fed?;
            Ok::<_, MediaError>(child.wait().await?)
        } => Some(r?),
    };
    let Some(status) = status else {
        let _ = child.kill().await;
        return Err(Failure::Other(MediaError::Cancelled));
    };
    if !status.success() {
        return Err(Failure::Encoder(MediaError::Ffmpeg(process::summarize(&stderr.await.unwrap_or_default(), status))));
    }
    Ok(())
}

/// The rendered picture fed to ffmpeg.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoFeed {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Timeline time of the first frame.
    pub from: f64,
    pub frames: u64,
}

/// A compiled export: everything ffmpeg needs except the output path.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Input options: the picture pipe first (when there is a picture), then
    /// `[-ss …] [-t …] -i path` per media input (a file, or a stretch of one several clips
    /// read), in input-index order.
    pub inputs: Vec<String>,
    /// The `-filter_complex` graph; produces `[vout]` and/or `[aout]`.
    pub graph: String,
    /// Mapping, codecs and muxer options.
    pub output: Vec<String>,
    /// Rendered length in seconds.
    pub duration: f64,
    /// Every file the inputs read.
    pub sources: Vec<PathBuf>,
    pub encoder: Option<String>,
    pub hardware: bool,
    /// The frames to render and pipe in, for formats with a picture.
    pub video: Option<VideoFeed>,
    /// The mix ffmpeg reads (made by [`export`] before it runs the plan), for formats with sound.
    pub mix: Option<MixInput>,
}

/// The sound of an export: the mixer's output for `from..to`, raw f32le stereo at `rate`,
/// written to `path` before ffmpeg starts.
#[derive(Debug, Clone, PartialEq)]
pub struct MixInput {
    pub path: PathBuf,
    pub rate: u32,
    pub from: f64,
    pub to: f64,
}

impl Plan {
    /// Full ffmpeg argv. With `script`, the graph is read from that file (already written).
    pub fn args(&self, out: &Path, script: Option<&Path>, caps: &Caps) -> Vec<String> {
        let mut args: Vec<String> =
            ["-hide_banner", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats"].map(s).to_vec();
        if self.video.is_none() {
            args.insert(1, s("-nostdin"));
        }
        args.extend(self.body(script, caps));
        args.push(path(out));
        args
    }

    /// Inputs, graph and output options, without the global options and the output path.
    pub(crate) fn body(&self, script: Option<&Path>, caps: &Caps) -> Vec<String> {
        let mut args = self.inputs.clone();
        match script {
            // ffmpeg 7 replaced -filter_complex_script with the generic `-/option file` syntax.
            Some(file) if caps.version == 0 || caps.version >= 7 => args.extend([s("-/filter_complex"), path(file)]),
            Some(file) => args.extend([s("-filter_complex_script"), path(file)]),
            // Sound only: the mix is mapped as it is.
            None if self.graph.is_empty() => {}
            None => args.extend([s("-filter_complex"), self.graph.clone()]),
        }
        args.extend(self.output.iter().cloned());
        args
    }
}

/// Compiles `project` into a [`Plan`]. Pure: doesn't touch the file system.
///
/// Only clips that intersect the rendered range get an input and a chain, so rendering a short
/// range stays cheap however long the timeline is.
pub fn build(project: &Project, settings: &ExportSettings, caps: &Caps) -> MediaResult<Plan> {
    build_with_hardware(project, settings, caps, &Hardware::none())
}

pub fn build_with_hardware(project: &Project, settings: &ExportSettings, caps: &Caps, hw: &Hardware) -> MediaResult<Plan> {
    let ps = &project.settings;
    let end = project.duration();
    if end <= 1e-6 {
        return Err(MediaError::Unsupported("the timeline is empty".into()));
    }
    let (from, to) = settings.range.map_or((0.0, end), |(a, b)| (a.max(0.0), b.min(end)));
    if to - from < 1e-3 {
        return Err(MediaError::Unsupported("the export range is empty".into()));
    }
    let format = settings.format;
    let (width, height) = output_size(ps, settings);
    let fps = output_fps(ps, settings);
    let mut codecs = Codecs::pick(format, settings.quality, caps, hw, settings.encoder, width, height, fps)?;
    let total = to - from;
    let picture = !format.is_audio_only();
    let mut g = Graph { inputs: codecs.device.clone(), count: 0, sources: vec![], chains: vec![] };

    let mut output = vec![];
    let mut video = None;
    if picture {
        let frames = ((total * fps) - 1e-6).ceil().max(1.0) as u64;
        video = Some(VideoFeed { width, height, fps, from, frames });
        g.inputs.extend(["-f", "rawvideo", "-pix_fmt", "rgba", "-s"].map(s));
        g.inputs.extend([format!("{width}x{height}"), s("-r"), crate::ntsc(fps).map_or_else(|| num(fps), s), s("-i"), s("pipe:0")]);
        g.count = 1;
        g.chains.push(match format {
            ExportFormat::Gif => s("[0:v]split[g0][g1];[g0]palettegen[pal];[g1][pal]paletteuse[vout]"),
            _ => format!("[0:v]scale=out_color_matrix=bt709:out_range=tv,format={}", codecs.pix_fmt) + if codecs.upload { ",hwupload[vout]" } else { "[vout]" },
        });
        output.extend([s("-map"), s("[vout]")]);
        output.extend(codecs.video);
        if format != ExportFormat::Gif {
            output.extend(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"].map(s));
        }
    }
    let mut mix = None;
    if format != ExportFormat::Gif {
        // The sound: the mixer's output, the last input, encoded as the format wants.
        let sound = Sound::pick(format, settings, ps, caps)?;
        if format.is_audio_only() {
            codecs.muxer = sound.muxer.clone();
        }
        let file = mix_path()?;
        g.sources = sound_files(project, from, to);
        g.inputs.extend(["-f", "f32le", "-ar"].map(s));
        g.inputs.extend([sound.rate.to_string(), s("-ac"), s("2"), s("-i"), path(&file)]);
        output.extend([s("-map"), format!("{}:a", g.count)]);
        g.count += 1;
        output.extend(sound.args);
        output.extend([s("-ar"), sound.rate.to_string()]);
        mix = Some(MixInput { path: file, rate: sound.rate, from, to });
    }
    output.extend(codecs.muxer);
    output.extend([s("-t"), num(total)]);
    Ok(Plan { inputs: g.inputs, graph: g.chains.join(";\n"), output, duration: total, sources: g.sources, encoder: codecs.encoder, hardware: codecs.hardware && picture, video, mix })
}

/// A temporary file for an export's mix (raw f32le), in a folder whose path is valid UTF-8.
fn mix_path() -> MediaResult<PathBuf> {
    Ok(script_path("mix")?.with_extension("f32"))
}

/// The media files whose sound plays between `from` and `to` (to check they are all there).
fn sound_files(project: &Project, from: f64, to: f64) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = vec![];
    for track in project.tracks.iter().filter(|t| !t.muted) {
        for h in kimchi_audio::mixer::heard(project, track) {
            if h.clip.end() <= from || h.clip.start >= to {
                continue;
            }
            if let Some(a) = kimchi_audio::mixer::sound_of(project, &h.clip) {
                let file = PathBuf::from(&a.path);
                // A file gone but with its proxy still there plays from the proxy.
                let file = if !file.exists() && let Some(p) = a.proxy.as_ref().map(PathBuf::from).filter(|p| p.exists()) { p } else { file };
                if !files.contains(&file) {
                    files.push(file);
                }
            }
        }
    }
    files
}

/// How an export's sound is encoded.
#[derive(Debug, Clone, PartialEq)]
struct Sound {
    /// Codec options (`-c:a …`, bitrate, sample format).
    args: Vec<String>,
    /// The container, for sound-only exports.
    muxer: Vec<String>,
    rate: u32,
}

impl Sound {
    fn pick(format: ExportFormat, st: &ExportSettings, ps: &ProjectSettings, caps: &Caps) -> MediaResult<Self> {
        let o = &st.audio;
        let q = st.quality;
        let bad = |m: String| MediaError::Unsupported(m);
        let missing = |what: &str, lib: &str| bad(format!("this ffmpeg build has no {what} encoder ({lib}); choose another audio format"));
        let mut rate = match o.sample_rate {
            Some(r) if AUDIO_RATES.contains(&r) => r,
            Some(r) => return Err(bad(format!("audio is exported at 44100, 48000 or 96000 Hz, not {r}"))),
            None => ps.sample_rate.clamp(8_000, 192_000),
        };
        let kbps = |draft: u32, standard: u32, high: u32| format!("{}k", o.bitrate_kbps.unwrap_or(by(q, draft, standard, high)).clamp(32, 512));
        let depth = o.bit_depth.unwrap_or(24);
        if ![16, 24, 32].contains(&depth) {
            return Err(bad(format!("bit depth is 16, 24 or 32 (float), not {depth}")));
        }
        let pcm = |le: bool| -> MediaResult<Vec<String>> {
            let codec = match (depth, le) {
                (16, true) => "pcm_s16le",
                (24, true) => "pcm_s24le",
                (32, true) => "pcm_f32le",
                (16, false) => "pcm_s16be",
                (24, false) => "pcm_s24be",
                _ => return Err(bad("AIFF holds 16 or 24-bit sound; export 32-bit float as WAV".into())),
            };
            let mut a = vec![s("-c:a"), s(codec)];
            if depth == 16 {
                // Triangular dither on the way down to 16 bits.
                a.extend([s("-af"), s("aresample=dither_method=triangular,aformat=sample_fmts=s16")]);
            }
            Ok(a)
        };
        let aac = |kbps: String| {
            let enc = caps.pick(&["aac", "aac_at"]).unwrap_or("aac");
            vec![s("-c:a"), s(enc), s("-b:a"), kbps]
        };
        let opus = |kbps: String| -> MediaResult<Vec<String>> {
            let enc = caps.pick(&["libopus", "opus"]).ok_or_else(|| missing("Opus", "libopus"))?;
            let mut a = vec![s("-c:a"), s(enc), s("-b:a"), kbps];
            if enc == "opus" {
                // The native Opus encoder is still flagged experimental.
                a.extend([s("-strict"), s("-2")]);
            }
            Ok(a)
        };
        let video_sound = |args: Vec<String>, rate: u32| Ok(Sound { args, muxer: vec![], rate });
        match format {
            ExportFormat::Gif => Err(bad("GIFs have no sound".into())),
            ExportFormat::Mp4 | ExportFormat::Hevc => video_sound(aac(kbps(128, 192, 256)), rate),
            ExportFormat::Prores => {
                // 16-bit unless asked: what ProRes exports always had.
                let depth = o.bit_depth.unwrap_or(16).min(24);
                video_sound(vec![s("-c:a"), s(if depth == 16 { "pcm_s16le" } else { "pcm_s24le" })], rate)
            }
            ExportFormat::Webm => {
                let args = match caps.pick(&["libopus", "libvorbis", "opus"]).ok_or_else(|| missing("Opus/Vorbis", "libopus"))? {
                    "libvorbis" => vec![s("-c:a"), s("libvorbis"), s("-q:a"), s(by(q, "4", "5", "7"))],
                    _ => opus(kbps(96, 128, 160))?,
                };
                video_sound(args, 48_000)
            }
            ExportFormat::Audio | ExportFormat::Wav => {
                let kind = o.format.unwrap_or(if format == ExportFormat::Wav { AudioFormat::Wav } else { AudioFormat::Aac });
                let (args, muxer) = match kind {
                    AudioFormat::Wav => (pcm(true)?, vec![s("-rf64"), s("auto"), s("-f"), s("wav")]),
                    AudioFormat::Aiff => (pcm(false)?, vec![s("-f"), s("aiff")]),
                    AudioFormat::Flac => {
                        let fmt = match depth {
                            16 => "s16",
                            24 => "s32",
                            _ => return Err(bad("FLAC holds 16 or 24-bit sound; export 32-bit float as WAV".into())),
                        };
                        let mut a = vec![s("-c:a"), s("flac"), s("-sample_fmt"), s(fmt)];
                        if depth == 24 {
                            a.extend([s("-bits_per_raw_sample"), s("24")]);
                        }
                        (a, vec![s("-f"), s("flac")])
                    }
                    AudioFormat::Mp3 => {
                        let enc = caps.pick(&["libmp3lame"]).ok_or_else(|| missing("MP3", "libmp3lame"))?;
                        if rate > 48_000 {
                            rate = 48_000;
                        }
                        (vec![s("-c:a"), s(enc), s("-b:a"), kbps(128, 192, 320)], vec![s("-f"), s("mp3")])
                    }
                    AudioFormat::Aac => (aac(kbps(128, 192, 256)), vec![s("-movflags"), s("+faststart"), s("-f"), s("ipod")]),
                    AudioFormat::Opus => {
                        rate = 48_000;
                        (opus(kbps(96, 128, 192))?, vec![s("-f"), s("ogg")])
                    }
                    AudioFormat::Vorbis => {
                        let enc = caps.pick(&["libvorbis", "vorbis"]).ok_or_else(|| missing("Vorbis", "libvorbis"))?;
                        let mut a = vec![s("-c:a"), s(enc)];
                        match o.bitrate_kbps {
                            Some(_) => a.extend([s("-b:a"), kbps(128, 192, 256)]),
                            None => a.extend([s("-q:a"), s(by(q, "4", "6", "8"))]),
                        }
                        if enc == "vorbis" {
                            a.extend([s("-strict"), s("-2")]);
                        }
                        (a, vec![s("-f"), s("ogg")])
                    }
                };
                Ok(Sound { args, muxer, rate })
            }
        }
    }
}

/// Mixes the sound of `mix` for an export (its loudness target applied).
async fn mix_for(
    tools: &Tools, project: &Project, settings: &ExportSettings, mix: &MixInput, selection: Selection,
    progress: &(dyn Fn(f64) + Sync), cancel: &CancellationToken,
) -> MediaResult<()> {
    let mut project = project.clone();
    if let Some(target) = settings.audio.loudness {
        project.mixer.master.loudness = Some(target);
    }
    let made = crate::audio::mixdown(tools, &project, (mix.from, mix.to), mix.rate, selection, &mix.path, progress, cancel).await?;
    tracing::info!(frames = made.frames, lufs = made.loudness.integrated, true_peak = made.loudness.true_peak, "mixed the export's sound");
    Ok(())
}

/// A sound-only export as stems: one file per track with sound and per bus, written into a new
/// folder at `settings.path` (published only once every file is there).
async fn export_stems(
    tools: &Tools, project: &Project, settings: &ExportSettings, progress: &(impl Fn(f64) + Send + Sync), cancel: &CancellationToken,
) -> MediaResult<Exported> {
    if !settings.format.is_audio_only() {
        return Err(MediaError::Unsupported("stems are sound-only exports: choose the audio or wav format".into()));
    }
    let dir = PathBuf::from(&settings.path);
    if dir.exists() {
        return Err(MediaError::Unsupported(format!("{} already exists; stems go into a new folder", dir.display())));
    }
    let caps = Caps::detect(tools).await?;
    // Every stem has the same range and encoding as a plain export would.
    let mut one = settings.clone();
    one.audio.stems = false;
    let template = build(project, &one, &caps)?;
    if let Some(missing) = template.sources.iter().find(|p| !p.exists()) {
        return Err(MediaError::Unsupported(format!("missing media file {}", missing.display())));
    }
    let mix = template.mix.clone().ok_or_else(|| MediaError::Unsupported("there is no sound to export".into()))?;
    let ext = settings.audio.format.unwrap_or(if settings.format == ExportFormat::Wav { AudioFormat::Wav } else { AudioFormat::Aac }).extension();
    // What to write: tracks with sound in the range (not muted), then buses something feeds.
    let mut stems: Vec<(String, Selection)> = vec![];
    for t in project.tracks.iter().filter(|t| !t.muted) {
        if kimchi_audio::mixer::heard(project, t).iter().any(|h| h.clip.end() > mix.from && h.clip.start < mix.to) {
            stems.push((t.name.clone(), Selection { tracks: Some(vec![t.id]), skip_master: !settings.audio.stems_master, bus: None }));
        }
    }
    for b in project.mixer.buses.iter().filter(|b| !b.muted) {
        if project.tracks.iter().any(|t| !t.muted && (t.mix.output == Some(b.id) || t.mix.sends.iter().any(|s| s.bus == b.id))) {
            stems.push((b.name.clone(), Selection { tracks: None, skip_master: !settings.audio.stems_master, bus: Some(b.id) }));
        }
    }
    if stems.is_empty() {
        return Err(MediaError::Unsupported("no track has sound in the range".into()));
    }
    let parent = dir.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    tokio::fs::create_dir_all(&parent).await?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "stems".into());
    let staging = parent.join(format!(".{name}.part"));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging).await?;
    let count = stems.len() as f64;
    let result = async {
        let mut used = vec![];
        for (i, (label, selection)) in stems.into_iter().enumerate() {
            let base = i as f64 / count;
            let file = staging.join(stem_name(i + 1, &label, ext, &mut used));
            let raw = mix_path()?;
            let stem_progress = |p: f64| progress(base + p * 0.8 / count);
            let r = mix_for(tools, project, &one, &MixInput { path: raw.clone(), ..mix.clone() }, selection, &stem_progress, cancel).await;
            let r = match r {
                Ok(()) => encode_raw(tools, &template, &mix.path, &raw, &file, cancel).await,
                Err(e) => Err(e),
            };
            let _ = tokio::fs::remove_file(&raw).await;
            r?;
            progress((i + 1) as f64 / count);
        }
        Ok::<_, MediaError>(())
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }
    if dir.exists() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(MediaError::Unsupported(format!("{} appeared during the export; nothing was replaced", dir.display())));
    }
    tokio::fs::rename(&staging, &dir).await?;
    progress(1.0);
    Ok(Exported { encoder: None, hardware: false, fell_back: false })
}

/// `NN-Name.ext`, unique in the folder, safe on every file system.
fn stem_name(n: usize, label: &str, ext: &str, used: &mut Vec<String>) -> String {
    let clean: String = label.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).take(80).collect();
    let clean = clean.trim();
    let base = format!("{n:02}-{}", if clean.is_empty() { "Track" } else { clean });
    let mut name = format!("{base}.{ext}");
    let mut k = 2;
    while used.contains(&name) {
        name = format!("{base} {k}.{ext}");
        k += 1;
    }
    used.push(name.clone());
    name
}

/// Encodes a raw mix `raw` to `out` with the sound options of `plan` (whose input reads `planned`).
async fn encode_raw(tools: &Tools, plan: &Plan, planned: &Path, raw: &Path, out: &Path, cancel: &CancellationToken) -> MediaResult<()> {
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-y", "-loglevel", "error"].map(s).to_vec();
    args.extend(plan.inputs.iter().map(|a| if *a == path(planned) { path(raw) } else { a.clone() }));
    args.extend(plan.output.iter().cloned());
    args.push(path(out));
    let mut child = process::spawn(&tools.ffmpeg, &args, false)?;
    let stderr = process::collect_stderr(&mut child);
    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(MediaError::Cancelled);
        }
        st = child.wait() => st?,
    };
    if !status.success() {
        return Err(MediaError::Ffmpeg(process::summarize(&stderr.await.unwrap_or_default(), status)));
    }
    Ok(())
}

pub(crate) fn output_size(ps: &ProjectSettings, st: &ExportSettings) -> (u32, u32) {
    let aspect = ps.width.max(1) as f64 / ps.height.max(1) as f64;
    let (w, h) = match (st.width, st.height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, (w as f64 / aspect).round() as u32),
        (None, Some(h)) => ((h as f64 * aspect).round() as u32, h),
        // GIFs at full HD are huge; default to something shareable.
        (None, None) if st.format == ExportFormat::Gif && ps.width > 720 => (720, (720.0 / aspect).round() as u32),
        (None, None) => (ps.width, ps.height),
    };
    (even(w as f64), even(h as f64))
}

fn output_fps(ps: &ProjectSettings, st: &ExportSettings) -> f64 {
    let fps = st.fps.filter(|f| *f > 0.0).unwrap_or(ps.fps).clamp(1.0, 240.0);
    let fps = if st.format == ExportFormat::Gif { st.fps.filter(|f| *f > 0.0).unwrap_or(15.0).min(fps).min(30.0) } else { fps };
    crate::snap_fps(fps)
}

/// The ffmpeg inputs and filter chains of a plan being built.
struct Graph {
    inputs: Vec<String>,
    /// Inputs so far (the picture pipe counts).
    count: usize,
    /// Media files the export reads.
    sources: Vec<PathBuf>,
    chains: Vec<String>,
}

/// Encoder options for one export.
struct Codecs {
    video: Vec<String>,
    muxer: Vec<String>,
    pix_fmt: &'static str,
    /// The video encoder.
    encoder: Option<String>,
    /// It is a hardware encoder.
    hardware: bool,
    /// Global options opening its device, before the inputs.
    device: Vec<String>,
    /// Frames go up to the device at the end of the graph.
    upload: bool,
}

impl Codecs {
    fn new(pix_fmt: &'static str) -> Self {
        Codecs { video: vec![], muxer: vec![], pix_fmt, encoder: None, hardware: false, device: vec![], upload: false }
    }

    #[allow(clippy::too_many_arguments)]
    fn pick(
        format: ExportFormat,
        q: Quality,
        caps: &Caps,
        hw: &Hardware,
        choice: EncoderChoice,
        w: u32,
        h: u32,
        fps: f64,
    ) -> MediaResult<Self> {
        // Bitrate for encoders without a constant-quality mode.
        let bitrate = (w as f64 * h as f64 * fps * by(q, 0.04, 0.08, 0.15)) as u64;
        let missing = |what: &str| MediaError::Unsupported(format!("this ffmpeg build has no {what} encoder"));
        let mut c = Codecs::new("yuv420p");

        // The container (the sound's codec is [`Sound::pick`]'s).
        match format {
            ExportFormat::Mp4 | ExportFormat::Hevc => {
                c.muxer = vec![s("-movflags"), s("+faststart"), s("-f"), s("mp4")];
            }
            ExportFormat::Prores => {
                c.muxer = vec![s("-f"), s("mov")];
            }
            ExportFormat::Webm => {
                c.muxer = vec![s("-f"), s("webm")];
            }
            ExportFormat::Gif => {
                c.video = vec![s("-c:v"), s("gif"), s("-loop"), s("0")];
                c.encoder = Some(s("gif"));
                c.muxer = vec![s("-f"), s("gif")];
                return Ok(c);
            }
            ExportFormat::Audio | ExportFormat::Wav => return Ok(c),
        }

        // The picture: the GPU or media engine first, unless the CPU was asked for.
        let codecs: &[Codec] = match (format, choice) {
            (ExportFormat::Mp4, _) => &[Codec::H264],
            (ExportFormat::Hevc, _) => &[Codec::Hevc],
            (ExportFormat::Prores, _) => &[Codec::Prores],
            // AV1 in WebM only when hardware was asked for: older players lack it.
            (ExportFormat::Webm, EncoderChoice::Hardware) => &[Codec::Vp9, Codec::Av1],
            (ExportFormat::Webm, _) => &[Codec::Vp9],
            _ => &[],
        };
        let found = codecs.iter().filter(|codec| accel::fits(**codec, w, h)).find_map(|codec| hw.best(*codec));
        match (found, choice) {
            (Some(v), EncoderChoice::Auto | EncoderChoice::Hardware) => {
                // Hardware encoders need a little more than x264 for the same picture.
                let rate = if v.encoder.codec == Codec::H264 { bitrate * 5 / 4 } else { bitrate * 4 / 5 };
                c.use_hardware(v, q, rate, hw.vaapi_device.as_deref());
                if format == ExportFormat::Hevc {
                    c.video.extend([s("-tag:v"), s("hvc1")]);
                }
                return Ok(c);
            }
            (None, EncoderChoice::Hardware) => {
                let too_big = codecs.iter().any(|codec| !accel::fits(*codec, w, h) && hw.best(*codec).is_some());
                return Err(MediaError::Unsupported(if too_big {
                    format!("{w}×{h} is too big for this computer's hardware {} encoder; use the CPU", name(format))
                } else {
                    format!("this computer has no hardware {} encoder; use the CPU (or Auto)", name(format))
                }));
            }
            _ => {}
        }

        match format {
            ExportFormat::Mp4 => {
                c.video = h264_args(caps, by(q, "veryfast", "medium", "slow"), by(q, 28, 22, 18), bitrate)?;
                c.video.extend([s("-pix_fmt"), s("yuv420p")]);
            }
            ExportFormat::Hevc => {
                let enc = caps.pick(&["libx265", "hevc_videotoolbox", "hevc_mf"]).ok_or_else(|| missing("HEVC"))?;
                c.video = vec![s("-c:v"), s(enc)];
                if enc == "libx265" {
                    let crf = by(q, 30, 26, 22).to_string();
                    c.video.extend(
                        [
                            "-preset",
                            by(q, "veryfast", "fast", "medium"),
                            "-crf",
                            &crf,
                            "-x265-params",
                            "log-level=error",
                        ]
                        .map(s),
                    );
                } else {
                    c.video.extend([s("-b:v"), (bitrate * 7 / 10).to_string()]);
                    if enc == "hevc_videotoolbox" {
                        c.video.extend([s("-allow_sw"), s("1")]);
                    }
                }
                // hvc1 is what Apple players require.
                c.video.extend(["-tag:v", "hvc1", "-pix_fmt", "yuv420p"].map(s));
            }
            ExportFormat::Prores => {
                let enc =
                    caps.pick(&["prores_ks", "prores", "prores_videotoolbox"]).ok_or_else(|| missing("ProRes"))?;
                c.video = vec![s("-c:v"), s(enc)];
                if enc == "prores_videotoolbox" {
                    c.video.extend([s("-profile:v"), s("hq")]);
                    c.pix_fmt = "p210le";
                } else {
                    c.video.extend([s("-profile:v"), s("3"), s("-vendor"), s("apl0")]);
                    c.pix_fmt = "yuv422p10le";
                }
            }
            ExportFormat::Webm => {
                let enc = caps
                    .pick(&["libvpx-vp9", "libvpx", "libsvtav1", "libaom-av1"])
                    .ok_or_else(|| missing("VP9/VP8/AV1"))?;
                c.video = vec![s("-c:v"), s(enc)];
                match enc {
                    // Tiles and row threading let VP9 use every core.
                    "libvpx-vp9" => c.video.extend(
                        ["-crf", &by(q, 40, 33, 28).to_string(), "-b:v", "0", "-deadline", "good"]
                            .into_iter()
                            .chain(["-cpu-used", by(q, "5", "3", "2"), "-row-mt", "1", "-tile-columns", vp9_tiles(w)])
                            .map(s),
                    ),
                    "libvpx" => c.video.extend(
                        ["-crf", &by(q, 30, 20, 10).to_string(), "-b:v", &bitrate.to_string(), "-cpu-used", "4"].map(s),
                    ),
                    "libsvtav1" => c.video.extend(["-crf", &by(q, 45, 35, 28).to_string(), "-preset", "8"].map(s)),
                    _ => c.video.extend(
                        ["-crf", &by(q, 45, 35, 28).to_string(), "-b:v", "0", "-cpu-used", "6", "-row-mt", "1"].map(s),
                    ),
                }
            }
            _ => unreachable!("sound-only formats returned above"),
        }
        c.encoder = c.video.windows(2).find(|w| w[0] == "-c:v").map(|w| w[1].clone());
        Ok(c)
    }

    fn use_hardware(&mut self, v: &Verified, q: Quality, bitrate: u64, vaapi_device: Option<&str>) {
        let a = accel::args(v, q, bitrate, vaapi_device);
        self.video = a.video;
        if !a.upload {
            self.video.extend([s("-pix_fmt"), s(a.pix_fmt)]);
        }
        self.pix_fmt = a.pix_fmt;
        self.device = a.device;
        self.upload = a.upload;
        self.encoder = Some(s(v.encoder.name));
        self.hardware = true;
    }
}

/// log2 of the VP9 tile columns for a picture `w` wide (tiles are at least 256 px).
fn vp9_tiles(w: u32) -> &'static str {
    match w {
        ..1024 => "1",
        1024..2048 => "2",
        2048..4096 => "3",
        _ => "4",
    }
}

fn name(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Mp4 => "H.264",
        ExportFormat::Hevc => "HEVC",
        ExportFormat::Prores => "ProRes",
        ExportFormat::Webm => "VP9/AV1",
        ExportFormat::Gif => "GIF",
        ExportFormat::Audio | ExportFormat::Wav => "audio",
    }
}

/// H.264 encoder options, falling back from libx264 to platform encoders
/// (LGPL builds don't ship x264), and to MPEG-4 Part 2 as a last resort.
pub(crate) fn h264_args(caps: &Caps, preset: &str, crf: u32, bitrate: u64) -> MediaResult<Vec<String>> {
    let enc = caps
        .pick(&["libx264", "h264_videotoolbox", "h264_mf", "libopenh264", "mpeg4"])
        .ok_or_else(|| MediaError::Unsupported("this ffmpeg build has no H.264 encoder".into()))?;
    let mut args = vec![s("-c:v"), s(enc)];
    match enc {
        "libx264" => args.extend([s("-preset"), s(preset), s("-crf"), crf.to_string()]),
        "mpeg4" => {
            tracing::warn!("no H.264 encoder available, falling back to MPEG-4 Part 2");
            args.extend([s("-q:v"), s("3")]);
        }
        _ => {
            args.extend([s("-b:v"), bitrate.to_string()]);
            if enc == "h264_videotoolbox" {
                args.extend([s("-allow_sw"), s("1")]);
            }
        }
    }
    Ok(args)
}

/// Hardware H.264 options for a proxy or similar: `(video args, pix_fmt, device args, upload)`.
pub(crate) fn h264_hardware(v: &Verified, bitrate: u64, vaapi_device: Option<&str>) -> (Vec<String>, &'static str, Vec<String>, bool) {
    let mut c = Codecs::new("nv12");
    c.use_hardware(v, Quality::Standard, bitrate, vaapi_device);
    (c.video, c.pix_fmt, c.device, c.upload)
}

/// `#rrggbb`/`#rgb`/`#rrggbbaa` → ffmpeg colour syntax.
pub(crate) fn color(c: &str) -> String {
    let hex = c.trim().trim_start_matches('#');
    let hex = match hex.len() {
        3 => hex.chars().flat_map(|ch| [ch, ch]).collect(),
        _ => hex.to_string(),
    };
    if matches!(hex.len(), 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        format!("0x{hex}")
    } else {
        tracing::warn!(color = c, "unrecognised colour; using black");
        s("black")
    }
}

/// Compact decimal for filter arguments: always has a fractional part (`2.0`, `0.333333`).
fn num(x: f64) -> String {
    let x = if x.abs() < 5e-7 { 0.0 } else { x };
    let t = format!("{x:.6}");
    let t = t.trim_end_matches('0');
    if t.ends_with('.') { format!("{t}0") } else { t.to_string() }
}

fn by<T>(q: Quality, draft: T, standard: T, high: T) -> T {
    match q {
        Quality::Draft => draft,
        Quality::Standard => standard,
        Quality::High => high,
    }
}

fn even(x: f64) -> u32 {
    ((x.round().max(2.0) as u32) / 2) * 2
}

fn s(x: &str) -> String {
    x.to_string()
}

/// A path for the command line (paths here come from UTF-8 settings and projects).
fn path(p: &Path) -> String {
    crate::probe::input_path(p).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
