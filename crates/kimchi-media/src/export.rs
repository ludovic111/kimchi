//! Renders a project to a file by compiling the timeline into one ffmpeg filter graph.
//!
//! [`build`] is pure: it turns a project into a [`Plan`] (inputs, graph, output
//! options). [`export`] runs that plan, reports progress and handles cancellation.
//!
//! Graph shape: a `color` base the size of the output, then every visible clip
//! overlaid bottom-up (last track first, `tracks[0]` last), each one shifted to
//! its timeline position with `setpts`. Audible clips are tempo-adjusted,
//! faded, delayed to their start and mixed with `amix`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kimchi_core::{Clip, ClipContent, Fit, Id, MediaKind, Project, ProjectSettings, TrackKind};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

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
}

/// Pre-rendered transparent PNGs, one per text clip, the size of the canvas
/// ([`crate::text::rasterize_overlays`]), so exports match the preview exactly.
pub type Overlays = HashMap<Id, PathBuf>;

/// Graphs longer than this go through a script file instead of argv.
pub(crate) const INLINE_GRAPH_MAX: usize = 4_000;

/// Sample rate of the preview's PCM ([`Sink::Samples`]).
pub(crate) const PREVIEW_SAMPLE_RATE: u32 = 48_000;

/// What a compiled graph feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sink {
    /// The encoders and muxer of `settings.format`: an export.
    Encode,
    /// Raw RGBA frames on stdout, picture only (the preview).
    Frames,
    /// Raw interleaved f32le stereo PCM at [`PREVIEW_SAMPLE_RATE`] on stdout, sound only (the preview).
    Samples,
}

/// Renders `project`. `progress` receives 0.0–1.0.
pub async fn export(
    tools: &Tools,
    project: &Project,
    overlays: &Overlays,
    settings: &ExportSettings,
    progress: impl Fn(f64) + Send + Sync,
    cancel: CancellationToken,
) -> MediaResult<()> {
    let caps = Caps::detect(tools).await?;
    let plan = build(project, overlays, settings, &caps)?;
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
    let script = (plan.graph.len() > INLINE_GRAPH_MAX)
        .then(|| std::env::temp_dir().join(format!("kimchi-graph-{}.txt", kimchi_core::new_id())));
    if let Some(script) = &script {
        tokio::fs::write(script, &plan.graph).await?;
    }
    progress(0.0);
    let result = run(tools, &plan.args(&part, script.as_deref(), &caps), plan.duration, &progress, &cancel).await;
    if let Some(script) = &script {
        let _ = tokio::fs::remove_file(script).await;
    }
    match result {
        Ok(()) => {
            tokio::fs::rename(&part, &out).await?;
            progress(1.0);
            Ok(())
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

async fn run(
    tools: &Tools,
    args: &[String],
    duration: f64,
    progress: &(impl Fn(f64) + Sync),
    cancel: &CancellationToken,
) -> MediaResult<()> {
    let mut child = process::spawn(&tools.ffmpeg, args, true)?;
    let stderr = process::collect_stderr(&mut child);
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    // `-progress pipe:1` prints key=value blocks; out_time_us is how far the muxer got.
    let watch = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                progress((us / 1e6 / duration.max(1e-6)).clamp(0.0, 0.999));
            }
        }
    };
    let status = tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        status = async { watch.await; child.wait().await } => Some(status?),
    };
    let Some(status) = status else {
        let _ = child.kill().await;
        return Err(MediaError::Cancelled);
    };
    if !status.success() {
        return Err(MediaError::Ffmpeg(process::summarize(&stderr.await.unwrap_or_default(), status)));
    }
    Ok(())
}

/// A compiled export: everything ffmpeg needs except the output path.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Input options: `[-ss …] [-t …] [-loop 1 …] -i path` per input, in input-index order.
    pub inputs: Vec<String>,
    /// The `-filter_complex` graph; produces `[vout]` and/or `[aout]`.
    pub graph: String,
    /// Mapping, codecs and muxer options.
    pub output: Vec<String>,
    /// Rendered length in seconds.
    pub duration: f64,
    /// Every file the inputs read.
    pub sources: Vec<PathBuf>,
}

impl Plan {
    /// Full ffmpeg argv. With `script`, the graph is read from that file (already written).
    pub fn args(&self, out: &Path, script: Option<&Path>, caps: &Caps) -> Vec<String> {
        let mut args: Vec<String> =
            ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats"].map(s).to_vec();
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
/// range (the preview's frames) stays cheap however long the timeline is.
pub fn build(project: &Project, overlays: &Overlays, settings: &ExportSettings, caps: &Caps) -> MediaResult<Plan> {
    compile(project, overlays, settings, caps, Sink::Encode).map(|(plan, _)| plan)
}

/// [`build`] for any [`Sink`]; also returns how many sound chains were mixed (0 = silence).
pub(crate) fn compile(
    project: &Project,
    overlays: &Overlays,
    settings: &ExportSettings,
    caps: &Caps,
    sink: Sink,
) -> MediaResult<(Plan, usize)> {
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
        Sink::Encode => Codecs::pick(format, settings.quality, caps, width, height, fps)?,
        Sink::Frames => Codecs {
            video: ["-f", "rawvideo", "-pix_fmt", "rgba"].map(s).to_vec(),
            audio: vec![],
            muxer: vec![],
            pix_fmt: "rgba",
        },
        Sink::Samples => Codecs {
            video: vec![],
            audio: ["-c:a", "pcm_f32le", "-f", "f32le", "-ac", "2"].map(s).to_vec(),
            muxer: vec![],
            pix_fmt: "rgba",
        },
    };
    let mut g = Graph {
        project,
        from,
        to,
        fps,
        sample_rate: if format == ExportFormat::Webm || sink == Sink::Samples {
            PREVIEW_SAMPLE_RATE
        } else {
            ps.sample_rate.max(8_000)
        },
        sx: width as f64 / ps.width.max(1) as f64,
        sy: height as f64 / ps.height.max(1) as f64,
        width,
        height,
        overlay_format: if format == ExportFormat::Prores { "yuv444" } else { "yuv420" },
        inputs: vec![],
        sources: vec![],
        input_of: HashMap::new(),
        chains: vec![],
    };
    let total = to - from;

    let mut output = vec![];
    let mut audible = 0;
    if format != ExportFormat::Audio && sink != Sink::Samples {
        let base_fmt = if format == ExportFormat::Prores { "yuv444p" } else { "yuv420p" };
        g.chains.push(format!(
            "color=c={}:s={width}x{height}:r={}:d={},format={base_fmt}[b0]",
            color(&ps.background),
            num(fps),
            num(total)
        ));
        let mut layers = 0;
        for track in project.tracks.iter().rev().filter(|t| t.kind == TrackKind::Video && !t.hidden) {
            for clip in sorted(&track.clips) {
                if let Some(label) = g.visual(overlays, clip) {
                    let (start, end) = (clip.start.max(from) - from, clip.end().min(to) - from);
                    let (x, y) = if matches!(clip.content, ClipContent::Text { .. }) {
                        (s("0"), s("0"))
                    } else {
                        (
                            format!("(main_w-overlay_w)/2{}", signed(clip.transform.x * g.sx)),
                            format!("(main_h-overlay_h)/2{}", signed(clip.transform.y * g.sy)),
                        )
                    };
                    // Half a frame of slack so the frame at exactly `start` shows and the one at `end` doesn't.
                    let half = 0.5 / fps;
                    g.chains.push(format!(
                        "[b{layers}][{label}]overlay=x={x}:y={y}:format={}:eof_action=pass:enable='between(t,{},{})'[b{}]",
                        g.overlay_format,
                        num(start - half),
                        num(end - half),
                        layers + 1
                    ));
                    layers += 1;
                }
            }
        }
        let tail = match format {
            ExportFormat::Gif => "split[g0][g1];[g0]palettegen[pal];[g1][pal]paletteuse".into(),
            _ => format!("format={}", codecs.pix_fmt),
        };
        g.chains.push(format!("[b{layers}]{tail}[vout]"));
        output.extend([s("-map"), s("[vout]")]);
        output.extend(codecs.video);
    }
    if format != ExportFormat::Gif && sink != Sink::Frames {
        let mut mixed = vec![];
        for track in project.tracks.iter().filter(|t| !t.muted) {
            for clip in sorted(&track.clips) {
                mixed.extend(g.audible(clip));
            }
        }
        let sr = g.sample_rate;
        audible = mixed.len();
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
    }
    output.extend(codecs.muxer);
    output.extend([s("-t"), num(total)]);
    Ok((Plan { inputs: g.inputs, graph: g.chains.join(";\n"), output, duration: total, sources: g.sources }, audible))
}

fn sorted(clips: &[Clip]) -> Vec<&Clip> {
    let mut clips: Vec<&Clip> = clips.iter().collect();
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
    if st.format == ExportFormat::Gif { st.fps.filter(|f| *f > 0.0).unwrap_or(15.0).min(fps).min(30.0) } else { fps }
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

    fn alpha_fades(&self) -> Vec<String> {
        let mut f = vec![];
        if let Some((st, d)) = self.fade_in {
            f.push(format!("fade=t=in:st={}:d={}:alpha=1", num(st), num(d)));
        }
        if let Some((st, d)) = self.fade_out {
            f.push(format!("fade=t=out:st={}:d={}:alpha=1", num(st), num(d)));
        }
        f
    }
}

struct Graph<'a> {
    project: &'a Project,
    from: f64,
    to: f64,
    fps: f64,
    sample_rate: u32,
    /// Project pixels → output pixels.
    sx: f64,
    sy: f64,
    width: u32,
    height: u32,
    overlay_format: &'static str,
    inputs: Vec<String>,
    sources: Vec<PathBuf>,
    /// Clip → input index, so a video's picture and sound share one decoder.
    input_of: HashMap<Id, usize>,
    chains: Vec<String>,
}

impl Graph<'_> {
    fn input(&mut self, opts: Vec<String>, file: &str) -> usize {
        self.inputs.extend(opts);
        self.inputs.extend([s("-i"), s(file)]);
        self.sources.push(PathBuf::from(file));
        self.sources.len() - 1
    }

    /// Seeked and trimmed input for a clip's media.
    fn media_input(&mut self, clip: &Clip, file: &str, w: &Window) -> usize {
        if let Some(&i) = self.input_of.get(&clip.id) {
            return i;
        }
        let seek = clip.in_point.max(0.0) + w.decode_from * clip.speed;
        let mut opts = vec![];
        if seek > 1e-6 {
            opts.extend([s("-ss"), num(seek)]);
        }
        opts.extend([s("-t"), num(w.decoded() * clip.speed)]);
        let i = self.input(opts, file);
        self.input_of.insert(clip.id, i);
        i
    }

    fn still_input(&mut self, file: &str, w: &Window) -> usize {
        let opts = vec![s("-loop"), s("1"), s("-framerate"), num(self.fps), s("-t"), num(w.decoded())];
        self.input(opts, file)
    }

    /// Adds the picture chain for `clip` and returns its label, if it shows anything.
    fn visual(&mut self, overlays: &Overlays, clip: &Clip) -> Option<String> {
        let t = &clip.transform;
        if t.opacity <= 0.0 || t.scale <= 0.0 {
            return None;
        }
        let w = Window::of(clip, self.from, self.to)?;
        let speed = clip.speed.max(1e-3);
        let (bw, bh) = (
            self.project.settings.width as f64 * t.scale * self.sx,
            self.project.settings.height as f64 * t.scale * self.sy,
        );
        let mut f: Vec<String> = vec![];
        let mut alpha = t.opacity < 1.0 || !w.alpha_fades().is_empty();
        let mut rotate = t.rotation.rem_euclid(360.0).abs() > 1e-3;
        let head = match &clip.content {
            ClipContent::Pending { .. } => return None,
            ClipContent::Solid { color: c } => {
                alpha |= c.trim_start_matches('#').len() == 8;
                format!("color=c={}:s={}x{}:r={}:d={},", color(c), even(bw), even(bh), num(self.fps), num(w.decoded()))
            }
            ClipContent::Text { .. } => {
                let Some(png) = overlays.get(&clip.id) else {
                    tracing::warn!(clip = %clip.id, "no rendered overlay for text clip; skipping");
                    return None;
                };
                let i = self.still_input(&png.to_string_lossy(), &w);
                // Already positioned and rotated by the UI.
                rotate = false;
                if (self.sx - 1.0).abs() > 1e-9 || (self.sy - 1.0).abs() > 1e-9 {
                    f.push(format!("scale={}:{}", self.width, self.height));
                }
                format!("[{i}:v:0]")
            }
            ClipContent::Media { asset_id } => {
                let asset = self.project.asset(*asset_id)?;
                let i = match asset.kind {
                    MediaKind::Image => self.still_input(&asset.path, &w),
                    MediaKind::Video if asset.meta.has_video => {
                        let i = self.media_input(clip, &asset.path, &w);
                        f.push(if (speed - 1.0).abs() > 1e-9 {
                            format!("setpts=(PTS-STARTPTS)/{}", num(speed))
                        } else {
                            s("setpts=PTS-STARTPTS")
                        });
                        // Drop surplus frames early so scaling/rotating only sees what will be shown.
                        if asset.meta.fps.is_none_or(|src| src * speed > self.fps * 1.01) {
                            f.push(format!("fps={}", num(self.fps)));
                        }
                        i
                    }
                    _ => return None,
                };
                f.push(scale(t.fit, bw, bh));
                f.push(s("setsar=1"));
                format!("[{i}:v:0]")
            }
        };
        if alpha || rotate {
            f.push(s("format=rgba"));
        }
        if rotate {
            let a = num(t.rotation.to_radians());
            f.push(format!("rotate=a={a}:ow=rotw({a}):oh=roth({a}):c=none"));
        }
        if t.opacity < 1.0 {
            f.push(format!("colorchannelmixer=aa={}", num(t.opacity)));
        }
        f.extend(w.alpha_fades());
        if w.trim > 1e-6 {
            f.push(format!("trim=start={}", num(w.trim)));
        }
        f.push(format!("setpts=PTS-STARTPTS+{}/TB", num(w.start)));
        let label = format!("v{}", self.chains.len());
        self.chains.push(format!("{head}{}[{label}]", f.join(",")));
        Some(label)
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
        let i = self.media_input(clip, &asset.path, &w);
        let sr = self.sample_rate;
        let mut f = vec![format!("[{i}:a:0]aformat=sample_fmts=fltp:sample_rates={sr}:channel_layouts=stereo")];
        f.extend(atempo(clip.speed).into_iter().map(|t| format!("atempo={}", num(t))));
        // Timestamps from the sample count: robust after atempo and with odd source timestamps.
        f.push(format!("asetpts=N/{sr}/TB"));
        if (clip.volume - 1.0).abs() > 1e-9 {
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
        let label = format!("a{}", self.chains.len());
        self.chains.push(format!("{}[{label}]", f.join(",")));
        Some(label)
    }
}

/// Scales into a `bw`×`bh` box per `fit`, honouring the source's display aspect ratio, even dimensions.
fn scale(fit: Fit, bw: f64, bh: f64) -> String {
    let (bw, bh) = (num(bw.max(2.0)), num(bh.max(2.0)));
    let wide = format!("gt(dar,{bw}/{bh})");
    let ev = |e: String| format!("'max(2,2*trunc(({e})/2))'");
    match fit {
        Fit::Stretch => format!("scale={}:{}", ev(bw.clone()), ev(bh)),
        Fit::Contain => {
            format!("scale=w={}:h={}", ev(format!("if({wide},{bw},{bh}*dar)")), ev(format!("if({wide},{bw}/dar,{bh})")))
        }
        Fit::Cover => {
            format!("scale=w={}:h={}", ev(format!("if({wide},{bh}*dar,{bw})")), ev(format!("if({wide},{bh},{bw}/dar)")))
        }
    }
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
}

impl Codecs {
    fn pick(format: ExportFormat, q: Quality, caps: &Caps, w: u32, h: u32, fps: f64) -> MediaResult<Self> {
        // Bitrate for encoders without a constant-quality mode.
        let bitrate = (w as f64 * h as f64 * fps * by(q, 0.04, 0.08, 0.15)) as u64;
        let aac = || {
            let enc = caps.pick(&["aac", "aac_at"]).unwrap_or("aac");
            vec![s("-c:a"), s(enc), s("-b:a"), s(by(q, "128k", "192k", "256k"))]
        };
        let missing = |what: &str| MediaError::Unsupported(format!("this ffmpeg build has no {what} encoder"));
        let mut c = Codecs { video: vec![], audio: vec![], muxer: vec![], pix_fmt: "yuv420p" };
        match format {
            ExportFormat::Mp4 => {
                c.video = h264_args(caps, by(q, "veryfast", "medium", "slow"), by(q, 28, 22, 18), bitrate)?;
                c.video.extend([s("-pix_fmt"), s("yuv420p")]);
                c.audio = aac();
                c.muxer = vec![s("-movflags"), s("+faststart"), s("-f"), s("mp4")];
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
                c.audio = aac();
                c.muxer = vec![s("-movflags"), s("+faststart"), s("-f"), s("mp4")];
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
                c.audio = vec![s("-c:a"), s("pcm_s16le")];
                c.muxer = vec![s("-f"), s("mov")];
            }
            ExportFormat::Webm => {
                let enc = caps
                    .pick(&["libvpx-vp9", "libvpx", "libsvtav1", "libaom-av1"])
                    .ok_or_else(|| missing("VP9/VP8/AV1"))?;
                c.video = vec![s("-c:v"), s(enc)];
                match enc {
                    "libvpx-vp9" => c.video.extend(
                        ["-crf", &by(q, 40, 33, 28).to_string(), "-b:v", "0", "-deadline", "good"]
                            .into_iter()
                            .chain(["-cpu-used", by(q, "5", "3", "2"), "-row-mt", "1"])
                            .map(s),
                    ),
                    "libvpx" => c.video.extend(
                        ["-crf", &by(q, 30, 20, 10).to_string(), "-b:v", &bitrate.to_string(), "-cpu-used", "4"].map(s),
                    ),
                    "libsvtav1" => c.video.extend(["-crf", &by(q, 45, 35, 28).to_string(), "-preset", "8"].map(s)),
                    _ => c.video.extend(["-crf", &by(q, 45, 35, 28).to_string(), "-b:v", "0", "-cpu-used", "6"].map(s)),
                }
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
                c.muxer = vec![s("-f"), s("gif")];
            }
            ExportFormat::Audio => {
                c.audio = aac();
                c.muxer = vec![s("-movflags"), s("+faststart"), s("-f"), s("ipod")];
            }
        }
        Ok(c)
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

fn signed(x: f64) -> String {
    let n = num(x);
    if n.starts_with('-') { n } else { format!("+{n}") }
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

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
