//! The live preview: frames from the same compositor as the export ([`crate::render`]), so what
//! the window shows is exactly what renders, only smaller.
//!
//! [`render_frame`] draws one frame for scrubbing; [`PreviewStream`] plays from a point: the
//! compositor runs ahead of the clock on a worker thread (each playing video decoded by its own
//! ffmpeg), and the sound is mixed by one ffmpeg graph, both with small bounded buffers.
//! Speakers are the caller's business: the stream only hands out PCM.
//!
//! Sources are read through each asset's `proxy` when it exists on disk, and clips whose media
//! is missing are left out rather than failing the preview (an export reports them instead).
//!
//! Costs: one [`render_frame`] decodes every visible video at that time, in parallel (40–120 ms
//! depending on how far a source must decode from its previous keyframe), so scrubbing should
//! drop requests while one is in flight. Stills, titles and 3D devices are kept between frames.
//! When a stream falls behind (many 4K layers without proxies, heavy 3D on the CPU) frames
//! arrive late; their `pts` says when they belong.

use std::path::{Path, PathBuf};

use kimchi_core::Project;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::export::{self, ExportFormat, ExportSettings, PREVIEW_SAMPLE_RATE, Quality, Sink};
use crate::render::Renderer;
use crate::{Caps, MediaError, MediaResult, Tools, process};

/// Sample rate of [`AudioChunk`]s.
pub const SAMPLE_RATE: u32 = PREVIEW_SAMPLE_RATE;
/// Channels of [`AudioChunk`]s (interleaved stereo).
pub const CHANNELS: u16 = 2;

/// One composited picture: `width`×`height`, 4 bytes per pixel in **RGBA** order, straight
/// (not premultiplied) and opaque, rows top to bottom with no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

impl Frame {
    /// A frame of one colour (`#rrggbb`, like the project background).
    pub fn solid(width: u32, height: u32, color: &str) -> Self {
        let [r, g, b] = rgb(color);
        Self { width, height, rgba: [r, g, b, 255].repeat(width as usize * height as usize) }
    }

    /// RGBA at (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    /// Swaps red and blue in place, for consumers that want BGRA (GPUI's `RenderImage`).
    pub fn into_bgra(mut self) -> Vec<u8> {
        for px in self.rgba.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        self.rgba
    }
}

/// Interleaved f32 stereo samples at [`SAMPLE_RATE`], starting `start` seconds into the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    pub start: f64,
    pub samples: Vec<f32>,
}

/// The frame of the timeline shown at `time`, `width`×`height` (rounded down to even numbers;
/// pass the project's aspect ratio). `time` is snapped to the project's frame grid, so this is the
/// frame an export would contain. Past the end, or on an empty timeline, returns the background
/// colour without decoding anything.
pub async fn render_frame(tools: &Tools, project: &Project, time: f64, width: u32, height: u32) -> MediaResult<Frame> {
    let (width, height) = (even(width), even(height));
    let fps = project.settings.fps.clamp(1.0, 240.0);
    let end = project.duration();
    let time = snap(time.max(0.0), fps);
    if time >= end || end <= 1e-6 {
        return Ok(Frame::solid(width, height, &project.settings.background));
    }
    let (tools, project) = (tools.clone(), project.clone());
    tokio::task::spawn_blocking(move || {
        let mut r = Renderer::new(&tools, &project, width, height, fps);
        let p = r.still(time)?;
        Ok(Frame { width, height, rgba: crate::render::to_rgba(p) })
    })
    .await
    .map_err(|e| MediaError::Io(std::io::Error::other(e)))?
}

/// Playback from a point to the end of the timeline: frames in order, and the mixed sound.
///
/// Dropping it stops (kills) the ffmpeg processes.
pub struct PreviewStream {
    frames: mpsc::Receiver<MediaResult<(f64, Frame)>>,
    audio: Option<mpsc::Receiver<AudioChunk>>,
    tasks: Vec<JoinHandle<()>>,
    from: f64,
    fps: f64,
    duration: f64,
    size: (u32, u32),
}

/// Frames decoded ahead of the consumer (≈ 0.2 s at 30 fps).
const FRAMES_AHEAD: usize = 6;
/// Audio frames per [`AudioChunk`] (≈ 21 ms).
const CHUNK_FRAMES: usize = 1024;
/// Chunks decoded ahead (≈ 5 s): plenty for the device buffer, small enough to stop quickly.
const CHUNKS_AHEAD: usize = 256;

impl PreviewStream {
    /// Starts rendering from `from` (snapped to the frame grid at `fps`) to the end of the
    /// timeline at `width`×`height` (rounded down to even numbers) and `fps`. An empty timeline,
    /// or `from` past the end, gives a stream that is already finished.
    pub async fn start(
        tools: &Tools,
        project: &Project,
        from: f64,
        width: u32,
        height: u32,
        fps: f64,
    ) -> MediaResult<Self> {
        let (width, height) = (even(width), even(height));
        let fps = if fps.is_finite() && fps > 0.0 { fps.clamp(1.0, 240.0) } else { project.settings.fps.clamp(1.0, 240.0) };
        let end = project.duration();
        let from = snap(from.max(0.0), fps);
        let mut stream = Self {
            frames: mpsc::channel(1).1,
            audio: None,
            tasks: vec![],
            from,
            fps,
            duration: (end - from).max(0.0),
            size: (width, height),
        };
        if end - from < 1e-3 {
            stream.duration = 0.0;
            return Ok(stream);
        }
        let project = crate::render::playable(tools, project);
        let caps = Caps::detect(tools).await?;
        let st = settings(width, height, fps, (from, end));

        let (tx, rx) = mpsc::channel(FRAMES_AHEAD);
        stream.frames = rx;
        let (t2, p2) = (tools.clone(), project.clone());
        let decode_caps = caps.clone();
        stream.tasks.push(tokio::task::spawn_blocking(move || {
            let mut r = Renderer::new(&t2, &p2, width, height, fps).with_hardware_decoding(decode_caps);
            let frames = ((end - from) * fps - 1e-6).ceil().max(0.0) as u64;
            for n in 0..frames {
                let pts = from + n as f64 / fps;
                let frame = r.frame(pts).map(|p| (pts, Frame { width, height, rgba: crate::render::to_rgba(p) }));
                let failed = frame.is_err();
                if tx.blocking_send(frame).is_err() || failed {
                    return; // the stream was dropped
                }
            }
        }));

        let (plan, audible) = export::compile(&project, &st, &caps, Sink::Samples)?;
        if audible > 0 {
            let script = Script::write(&plan.graph).await?;
            let mut args = head();
            args.extend(plan.body(script.path(), &caps));
            args.push("-".into());
            let mut child = process::spawn(&tools.ffmpeg, &args, true)?;
            let (tx, rx) = mpsc::channel(CHUNKS_AHEAD);
            stream.audio = Some(rx);
            stream.tasks.push(tokio::spawn(async move {
                let _script = script;
                let stderr = process::collect_stderr(&mut child);
                let mut stdout = child.stdout.take().expect("piped stdout");
                let bytes = CHUNK_FRAMES * CHANNELS as usize * 4;
                let mut sent = 0u64; // samples per channel so far
                loop {
                    let mut buf = vec![0u8; bytes];
                    let mut filled = 0;
                    while filled < bytes {
                        match stdout.read(&mut buf[filled..]).await {
                            Ok(0) => break,
                            Ok(k) => filled += k,
                            Err(_) => return,
                        }
                    }
                    // Whole stereo frames only (a short read only happens at the very end).
                    let usable = filled - filled % (CHANNELS as usize * 4);
                    if usable > 0 {
                        let samples: Vec<f32> =
                            buf[..usable].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
                        let start = from + sent as f64 / SAMPLE_RATE as f64;
                        sent += (samples.len() / CHANNELS as usize) as u64;
                        if tx.send(AudioChunk { start, samples }).await.is_err() {
                            return;
                        }
                    }
                    if filled < bytes {
                        break;
                    }
                }
                if let Err(e) = finish(child, stderr).await {
                    tracing::warn!("preview audio: {e}");
                }
            }));
        }
        Ok(stream)
    }

    /// The next frame and its timeline time, in order; `Ok(None)` once the end is reached.
    pub async fn next_frame(&mut self) -> MediaResult<Option<(f64, Frame)>> {
        self.frames.recv().await.transpose()
    }

    /// The next frame if one is ready, without waiting; `None` when none is (yet).
    pub fn try_next_frame(&mut self) -> Option<MediaResult<(f64, Frame)>> {
        self.frames.try_recv().ok()
    }

    /// The sound, once: `None` for a silent range (or after the first call). Chunks are
    /// contiguous from [`Self::from`]; `try_recv`/`blocking_recv` work from an audio thread.
    pub fn audio(&mut self) -> Option<mpsc::Receiver<AudioChunk>> {
        self.audio.take()
    }

    /// Where playback starts, snapped to the frame grid.
    pub fn from(&self) -> f64 {
        self.from
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// Seconds from [`Self::from`] to the end of the timeline.
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Frame size (even numbers).
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

impl Drop for PreviewStream {
    fn drop(&mut self) {
        // Aborting drops each task's `Child`, which kills its ffmpeg (`kill_on_drop`).
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Waits for ffmpeg after its output ended; its stderr when it failed.
async fn finish(mut child: tokio::process::Child, stderr: JoinHandle<String>) -> MediaResult<()> {
    let status = child.wait().await?;
    if status.success() {
        return Ok(());
    }
    Err(MediaError::Ffmpeg(process::summarize(&stderr.await.unwrap_or_default(), status)))
}

fn head() -> Vec<String> {
    ["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats"].map(String::from).to_vec()
}

fn settings(width: u32, height: u32, fps: f64, range: (f64, f64)) -> ExportSettings {
    ExportSettings {
        path: String::new(),
        format: ExportFormat::Mp4,
        quality: Quality::Draft,
        width: Some(width),
        height: Some(height),
        fps: Some(fps),
        range: Some(range),
        encoder: Default::default(),
    }
}

/// Long graphs go through a temporary file, removed on drop.
struct Script(Option<PathBuf>);

impl Script {
    async fn write(graph: &str) -> MediaResult<Self> {
        if graph.len() <= export::INLINE_GRAPH_MAX {
            return Ok(Self(None));
        }
        let path = export::script_path("preview")?;
        tokio::fs::write(&path, graph).await?;
        Ok(Self(Some(path)))
    }

    fn path(&self) -> Option<&Path> {
        self.0.as_deref()
    }
}

impl Drop for Script {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The start of the frame containing `t`.
fn snap(t: f64, fps: f64) -> f64 {
    (t * fps + 1e-6).floor() / fps
}

fn even(x: u32) -> u32 {
    (x.max(2) / 2) * 2
}

/// `#rgb`/`#rrggbb`(`aa`) → RGB; black otherwise (as the export does).
fn rgb(c: &str) -> [u8; 3] {
    let hex = export::color(c);
    let hex = hex.trim_start_matches("0x");
    if hex.len() < 6 {
        return [0, 0, 0];
    }
    std::array::from_fn(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_and_colours() {
        assert_eq!(snap(1.0, 25.0), 1.0);
        assert_eq!(snap(1.039, 25.0), 1.0);
        assert_eq!(snap(1.04, 25.0), 1.04);
        assert_eq!(even(641), 640);
        assert_eq!(rgb("#203040"), [0x20, 0x30, 0x40]);
        assert_eq!(rgb("#abc"), [0xaa, 0xbb, 0xcc]);
        assert_eq!(rgb("nope"), [0, 0, 0]);
        let f = Frame::solid(4, 2, "#102030");
        assert_eq!(f.rgba.len(), 32);
        assert_eq!(f.pixel(3, 1), [0x10, 0x20, 0x30, 255]);
        assert_eq!(&f.into_bgra()[..4], [0x30, 0x20, 0x10, 255]);
    }
}
