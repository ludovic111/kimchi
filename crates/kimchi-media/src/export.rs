//! Renders a project to a file: the pictures come from the compositor ([`crate::render`]) as raw
//! frames on ffmpeg's standard input, the sound from an ffmpeg graph over the media files.
//!
//! [`build`] is pure: it turns a project into a [`Plan`] (inputs, graph, output options).
//! [`export`] runs that plan, feeding it frames, reports progress and handles cancellation.
//!
//! Graph shape: input 0 is the rendered picture (`[0:v]`), converted to the encoder's format
//! with BT.709 colours. Audible clips are reversed if they play backwards, tempo-adjusted, faded
//! (and their volume keyframes applied), delayed to their start and mixed with `amix`. Clips on
//! both sides of a transition crossfade over it ([`with_crossfades`]). Each media file is opened
//! once per run of nearby reads and cut per clip ([`Graph::wire`]).

use std::path::{Path, PathBuf};

use kimchi_core::{Clip, ClipContent, MediaKind, Project, ProjectSettings};
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
}

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

/// Sample rate of the preview's PCM ([`Sink::Samples`]).
pub(crate) const PREVIEW_SAMPLE_RATE: u32 = 48_000;

/// What a compiled graph feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sink {
    /// The encoders and muxer of `settings.format`: an export.
    Encode,
    /// Raw interleaved f32le stereo PCM at [`PREVIEW_SAMPLE_RATE`] on stdout, sound only (the preview).
    Samples,
    /// Raw f32le mono PCM at [`SPEECH_SAMPLE_RATE`] on stdout, sound only (speech recognition).
    Speech,
}

/// Sample rate of [`Sink::Speech`] (what Whisper listens to).
pub const SPEECH_SAMPLE_RATE: u32 = 16_000;

/// Renders `project`. `progress` receives 0.0–1.0.
pub async fn export(
    tools: &Tools,
    project: &Project,
    settings: &ExportSettings,
    progress: impl Fn(f64) + Send + Sync,
    cancel: CancellationToken,
) -> MediaResult<Exported> {
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
    let mut used = (plan.encoder.clone(), plan.hardware, false);
    let mut result = attempt(tools, project, &plan, &part, &caps, settings.encoder != EncoderChoice::Software, &progress, &cancel).await;
    // Only a failed hardware encoder is worth redoing on the CPU; a picture that can't be
    // rendered or decoded would fail the same way.
    if let Err(Failure::Encoder(e)) = &result
        && plan.hardware
        && settings.encoder == EncoderChoice::Auto
    {
        tracing::warn!(encoder = ?plan.encoder, error = %e, "hardware encode failed; encoding on the CPU");
        let plan = build_with_hardware(project, settings, &caps, &Hardware::none())?;
        progress(0.0);
        used = (plan.encoder.clone(), false, true);
        result = attempt(tools, project, &plan, &part, &caps, false, &progress, &cancel).await;
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
    compile_with_hardware(project, settings, caps, hw, Sink::Encode).map(|(plan, _)| plan)
}

/// [`build`] for any [`Sink`]; also returns how many sound chains were mixed (0 = silence).
pub(crate) fn compile(project: &Project, settings: &ExportSettings, caps: &Caps, sink: Sink) -> MediaResult<(Plan, usize)> {
    compile_with_hardware(project, settings, caps, &Hardware::none(), sink)
}

fn compile_with_hardware(project: &Project, settings: &ExportSettings, caps: &Caps, hw: &Hardware, sink: Sink) -> MediaResult<(Plan, usize)> {
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
    let codecs = match sink {
        Sink::Encode => Codecs::pick(format, settings.quality, caps, hw, settings.encoder, width, height, fps)?,
        Sink::Samples => Codecs {
            audio: ["-c:a", "pcm_f32le", "-f", "f32le", "-ac", "2"].map(s).to_vec(),
            ..Codecs::new("rgba")
        },
        Sink::Speech => Codecs {
            audio: ["-c:a", "pcm_f32le", "-f", "f32le", "-ac", "1"].map(s).to_vec(),
            ..Codecs::new("rgba")
        },
    };
    let total = to - from;
    let picture = !format.is_audio_only() && sink == Sink::Encode;
    let mut g = Graph {
        project,
        from,
        to,
        sample_rate: if sink == Sink::Speech {
            SPEECH_SAMPLE_RATE
        } else if format == ExportFormat::Webm || sink == Sink::Samples {
            PREVIEW_SAMPLE_RATE
        } else {
            ps.sample_rate.max(8_000)
        },
        inputs: codecs.device.clone(),
        count: 0,
        sources: vec![],
        sounds: vec![],
        chains: vec![],
    };

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
    if format != ExportFormat::Gif {
        let mut mixed = vec![];
        for track in project.tracks.iter().filter(|t| !t.muted) {
            for clip in with_crossfades(project, track) {
                mixed.extend(g.audible(&clip));
            }
        }
        g.wire();
        let sr = g.sample_rate;
        let audible = mixed.len();
        g.chains.push(if mixed.is_empty() {
            // Silence rather than no track: players and muxers cope better.
            format!("anullsrc=r={sr}:cl=stereo,atrim=end={}[aout]", num(total))
        } else {
            let labels: String = mixed.iter().map(|l| format!("[{l}]")).collect();
            // amix loses its timestamps once its first input ends (NOPTS frames), which
            // breaks atrim and A/V sync: rebuild them from the sample count, then pad/cut
            // to the exact length.
            format!(
                "{labels}amix=inputs={}:normalize=0:dropout_transition=0,asetpts=N/{sr}/TB,apad=whole_dur={t},atrim=end={t}[aout]",
                mixed.len(),
                t = num(total)
            )
        });
        output.extend([s("-map"), s("[aout]")]);
        output.extend(codecs.audio);
        output.extend([s("-ar"), sr.to_string()]);
        output.extend(codecs.muxer);
        output.extend([s("-t"), num(total)]);
        return Ok((Plan { inputs: g.inputs, graph: g.chains.join(";\n"), output, duration: total, sources: g.sources, encoder: codecs.encoder, hardware: codecs.hardware && picture, video }, audible));
    }
    output.extend(codecs.muxer);
    output.extend([s("-t"), num(total)]);
    Ok((Plan { inputs: g.inputs, graph: g.chains.join(";\n"), output, duration: total, sources: g.sources, encoder: codecs.encoder, hardware: codecs.hardware && picture, video }, 0))
}

/// A track's clips as their sound plays: around each transition on a cut the outgoing clip
/// plays on and the incoming one starts early (as far as their media allow), fading out and in
/// over the transition; a transition with no clip before it fades the sound in.
fn with_crossfades(project: &Project, track: &kimchi_core::Track) -> Vec<Clip> {
    let mut clips = track.clips.clone();
    let source_len = |c: &Clip| c.asset_id().and_then(|id| project.asset(id)).and_then(|a| a.duration()).unwrap_or(0.0);
    for span in kimchi_core::transition::spans(track) {
        let (len, half) = (span.duration(), span.duration() / 2.0);
        let Some(from) = span.from else {
            let to = &mut clips[span.to];
            to.fade_in = to.fade_in.max(len);
            continue;
        };
        let after = clips[from].room(source_len(&clips[from])).1.min(half);
        let a = &mut clips[from];
        a.duration += after;
        if a.reverse {
            a.in_point -= after * a.speed;
        }
        a.fade_out = after + half;
        let before = clips[span.to].room(source_len(&clips[span.to])).0.min(half);
        let b = &mut clips[span.to];
        b.start -= before;
        b.duration += before;
        if !b.reverse {
            b.in_point -= before * b.speed;
        }
        kimchi_core::anim::shift(&mut b.keyframes, before);
        b.fade_in = before + half;
    }
    clips.sort_by(|a, b| a.start.total_cmp(&b.start));
    clips
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

/// Which part of a clip lands in the export window, and how to decode it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Window {
    /// Output time where the clip's first rendered frame goes.
    start: f64,
    /// Rendered length.
    len: f64,
    /// Clip-local time where decoding starts (the input is seeked there).
    decode_from: f64,
    /// Seconds dropped after the fades (when a fade must be computed from an earlier point).
    trim: f64,
    /// `(start, duration)` in decoded-local time.
    fade_in: Option<(f64, f64)>,
    fade_out: Option<(f64, f64)>,
}

impl Window {
    fn of(clip: &Clip, from: f64, to: f64) -> Option<Self> {
        let (a, b) = (clip.start.max(from), clip.end().min(to));
        if b - a < 1e-6 || clip.duration <= 0.0 {
            return None;
        }
        let head = a - clip.start;
        let d = clip.duration;
        let (fi, fo) = (clip.fade_in.clamp(0.0, d), clip.fade_out.clamp(0.0, d));
        let fade_in = fi > 1e-6 && head < fi;
        let fade_out = fo > 1e-6 && d - fo < head + (b - a);
        // `fade`/`afade` can't start in the past, so decode from early enough
        // that every fade still needed begins at or after local zero.
        let decode_from = if fade_in {
            0.0
        } else if fade_out {
            head.min(d - fo)
        } else {
            head
        };
        Some(Self {
            start: a - from,
            len: b - a,
            decode_from,
            trim: head - decode_from,
            fade_in: fade_in.then_some((0.0, fi)),
            fade_out: fade_out.then_some((d - fo - decode_from, fo)),
        })
    }

    /// Decoded length in clip-local (timeline) seconds.
    fn decoded(&self) -> f64 {
        self.trim + self.len
    }
}

struct Graph<'a> {
    project: &'a Project,
    from: f64,
    to: f64,
    sample_rate: u32,
    inputs: Vec<String>,
    /// Inputs so far (the picture pipe counts).
    count: usize,
    sources: Vec<PathBuf>,
    /// Clips' sounds, given inputs by [`Graph::wire`].
    sounds: Vec<Sound>,
    chains: Vec<String>,
}

/// One clip's sound: `len` seconds of `file` read from `from`, then `filters`, as `[label]`.
struct Sound {
    file: String,
    from: f64,
    len: f64,
    filters: String,
    label: String,
}

/// Reads closer than this (seconds of source) share an input.
const SHARE_GAP: f64 = 1.0;
/// Most media inputs one graph opens. Each is an open file and a few hundred characters of
/// command line (Windows stops at 32 767; macOS apps get 256 files by default), so beyond this
/// the closest reads of a file share inputs even when far apart.
const MAX_INPUTS: usize = 64;

impl Graph<'_> {
    fn input(&mut self, opts: Vec<String>, file: &str) -> usize {
        self.inputs.extend(opts);
        self.inputs.extend([s("-i"), path(Path::new(file))]);
        if !self.sources.iter().any(|p| p == Path::new(file)) {
            self.sources.push(PathBuf::from(file));
        }
        self.count += 1;
        self.count - 1
    }

    /// Gives the sounds their inputs: each file is opened once per run of nearby reads (seeked
    /// to the first, cut for each sound with `asplit` + `atrim`), not once per clip, so hundreds
    /// of clips cut from one recording stay a handful of inputs.
    fn wire(&mut self) {
        // Runs of reads per file: (file, from, to, sounds), files in order of first use.
        let mut runs: Vec<(String, f64, f64, Vec<usize>)> = vec![];
        let mut files: Vec<&str> = vec![];
        for snd in &self.sounds {
            if !files.contains(&snd.file.as_str()) {
                files.push(&snd.file);
            }
        }
        for file in files {
            let mut mine: Vec<usize> = (0..self.sounds.len()).filter(|&i| self.sounds[i].file == file).collect();
            mine.sort_by(|&a, &b| self.sounds[a].from.total_cmp(&self.sounds[b].from));
            for i in mine {
                let snd = &self.sounds[i];
                match runs.last_mut() {
                    Some((f, _, to, members)) if f == file && snd.from <= *to + SHARE_GAP => {
                        *to = to.max(snd.from + snd.len);
                        members.push(i);
                    }
                    _ => runs.push((file.to_string(), snd.from, snd.from + snd.len, vec![i])),
                }
            }
        }
        // Too many inputs: merge the neighbouring runs (same file) with the smallest gap.
        while runs.len() > MAX_INPUTS {
            let closest = runs.windows(2).enumerate().filter(|(_, w)| w[0].0 == w[1].0).min_by(|(_, a), (_, b)| (a[1].1 - a[0].2).total_cmp(&(b[1].1 - b[0].2)));
            let Some((i, _)) = closest else { break }; // all different files
            let next = runs.remove(i + 1);
            let run = &mut runs[i];
            run.2 = run.2.max(next.2);
            run.3.extend(next.3);
        }
        // Inputs in the order the clips came (stable input numbers for a timeline).
        runs.sort_by_key(|r| r.3.iter().copied().min());
        for (file, from, to, mut members) in runs {
            let mut opts = vec![];
            if from > 1e-6 {
                opts.extend([s("-ss"), num(from)]);
            }
            opts.extend([s("-t"), num(to - from)]);
            let i = self.input(opts, &file);
            if let [only] = members[..] {
                let snd = &self.sounds[only];
                self.chains.push(format!("[{i}:a:0]{}[{}]", snd.filters, snd.label));
                continue;
            }
            members.sort_unstable();
            let outs: String = (0..members.len()).map(|k| format!("[s{i}_{k}]")).collect();
            self.chains.push(format!("[{i}:a:0]asplit={}{outs}", members.len()));
            for (k, m) in members.into_iter().enumerate() {
                let snd = &self.sounds[m];
                let (a, b) = (snd.from - from, snd.from - from + snd.len);
                self.chains.push(format!("[s{i}_{k}]atrim=start={}:end={},asetpts=PTS-STARTPTS,{}[{}]", num(a), num(b), snd.filters, snd.label));
            }
        }
    }

    /// Adds the sound chain for `clip` and returns its label, if it makes any sound.
    fn audible(&mut self, clip: &Clip) -> Option<String> {
        let ClipContent::Media { asset_id } = &clip.content else {
            return None;
        };
        let asset = self.project.asset(*asset_id)?;
        let has_sound = match asset.kind {
            MediaKind::Audio => true,
            MediaKind::Video => asset.meta.has_audio,
            MediaKind::Image => false,
        };
        if !has_sound || clip.volume <= 0.0 {
            return None;
        }
        let w = Window::of(clip, self.from, self.to)?;
        // A reversed clip shows its source backwards: the decoded window is read forwards from
        // the matching source time, then reversed.
        let from = if clip.reverse { clip.duration - w.decoded() - w.decode_from } else { w.decode_from };
        let seek = clip.in_point.max(0.0) + from.max(0.0) * clip.speed;
        let file = asset.path.clone();
        let sr = self.sample_rate;
        let mut f = vec![format!("aformat=sample_fmts=fltp:sample_rates={sr}:channel_layouts=stereo")];
        if clip.reverse {
            f.push(s("areverse"));
        }
        f.extend(atempo(clip.speed).into_iter().map(|t| format!("atempo={}", num(t))));
        // Timestamps from the sample count: robust after atempo and with odd source timestamps.
        f.push(format!("asetpts=N/{sr}/TB"));
        if clip.keyframes.contains_key("volume") {
            f.push(format!("volume=eval=frame:volume='{}'", volume_curve(clip, w.decode_from)));
        } else if (clip.volume - 1.0).abs() > 1e-9 {
            f.push(format!("volume={}", num(clip.volume)));
        }
        if let Some((st, d)) = w.fade_in {
            f.push(format!("afade=t=in:st={}:d={}", num(st), num(d)));
        }
        if let Some((st, d)) = w.fade_out {
            f.push(format!("afade=t=out:st={}:d={}", num(st), num(d)));
        }
        f.push(if w.trim > 1e-6 {
            format!("atrim=start={}:end={},asetpts=PTS-STARTPTS", num(w.trim), num(w.decoded()))
        } else {
            format!("atrim=end={}", num(w.decoded()))
        });
        let delay = (w.start * sr as f64).round() as u64;
        if delay > 0 {
            f.push(format!("adelay=delays={delay}S:all=1"));
        }
        let label = format!("a{}", self.sounds.len());
        self.sounds.push(Sound { file, from: seek, len: w.decoded() * clip.speed, filters: f.join(","), label: label.clone() });
        Some(label)
    }
}

/// A clip's volume keyframes as an ffmpeg expression of `t` (seconds from `decode_from` into the
/// clip): straight segments every 50 ms or so, nested `if`s.
fn volume_curve(clip: &Clip, decode_from: f64) -> String {
    let (a, b) = (decode_from, clip.duration);
    let n = (((b - a) / 0.05).ceil() as usize).clamp(1, 400);
    let pts: Vec<(f64, f64)> = (0..=n).map(|i| {
        let local = a + (b - a) * i as f64 / n as f64;
        (local - a, clip.volume_at(clip.start + local))
    }).collect();
    // Build from the last segment outwards: if(lt(t,t1), seg0, if(lt(t,t2), seg1, …)).
    let mut expr = num(pts.last().map_or(1.0, |p| p.1));
    for w in pts.windows(2).rev() {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        let k = if t1 - t0 > 1e-9 { (v1 - v0) / (t1 - t0) } else { 0.0 };
        expr = format!("if(lt(t,{}),{}+({})*(t-{}),{expr})", num(t1), num(v0), num(k), num(t0));
    }
    expr
}

/// `atempo` only takes 0.5–2.0 per stage, so bigger changes are chained.
pub fn atempo(speed: f64) -> Vec<f64> {
    let mut rest = speed.clamp(0.01, 100.0);
    let mut stages = vec![];
    while rest > 2.0 + 1e-9 {
        stages.push(2.0);
        rest /= 2.0;
    }
    while rest < 0.5 - 1e-9 {
        stages.push(0.5);
        rest /= 0.5;
    }
    if (rest - 1.0).abs() > 1e-9 {
        stages.push(rest);
    }
    stages
}

/// Encoder options for one export.
struct Codecs {
    video: Vec<String>,
    audio: Vec<String>,
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
        Codecs { video: vec![], audio: vec![], muxer: vec![], pix_fmt, encoder: None, hardware: false, device: vec![], upload: false }
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
        let aac = || {
            let enc = caps.pick(&["aac", "aac_at"]).unwrap_or("aac");
            vec![s("-c:a"), s(enc), s("-b:a"), s(by(q, "128k", "192k", "256k"))]
        };
        let missing = |what: &str| MediaError::Unsupported(format!("this ffmpeg build has no {what} encoder"));
        let mut c = Codecs::new("yuv420p");

        // The container and the sound.
        match format {
            ExportFormat::Mp4 | ExportFormat::Hevc => {
                c.audio = aac();
                c.muxer = vec![s("-movflags"), s("+faststart"), s("-f"), s("mp4")];
            }
            ExportFormat::Prores => {
                c.audio = vec![s("-c:a"), s("pcm_s16le")];
                c.muxer = vec![s("-f"), s("mov")];
            }
            ExportFormat::Webm => {
                c.audio = match caps.pick(&["libopus", "libvorbis", "opus"]).ok_or_else(|| missing("Opus/Vorbis"))? {
                    "libvorbis" => vec![s("-c:a"), s("libvorbis"), s("-q:a"), s(by(q, "4", "5", "7"))],
                    // The native Opus encoder is still flagged experimental.
                    enc => {
                        let mut a = vec![s("-c:a"), s(enc), s("-b:a"), s(by(q, "96k", "128k", "160k"))];
                        if enc == "opus" {
                            a.extend([s("-strict"), s("-2")]);
                        }
                        a
                    }
                };
                c.muxer = vec![s("-f"), s("webm")];
            }
            ExportFormat::Gif => {
                c.video = vec![s("-c:v"), s("gif"), s("-loop"), s("0")];
                c.encoder = Some(s("gif"));
                c.muxer = vec![s("-f"), s("gif")];
                return Ok(c);
            }
            ExportFormat::Audio => {
                c.audio = aac();
                c.muxer = vec![s("-movflags"), s("+faststart"), s("-f"), s("ipod")];
                return Ok(c);
            }
            ExportFormat::Wav => {
                c.audio = vec![s("-c:a"), s("pcm_s24le")];
                c.muxer = vec![s("-f"), s("wav")];
                return Ok(c);
            }
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
