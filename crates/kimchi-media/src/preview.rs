//! The live preview: frames from the same compositor as the export ([`crate::render`]), so what
//! the window shows is exactly what renders, only smaller.
//!
//! [`render_frame`] draws one frame for scrubbing; [`PreviewStream`] plays from a point: the
//! compositor runs ahead of the clock on a worker thread (each playing video decoded by its own
//! ffmpeg), and the sound comes from kimchi-audio's mixer in real time on its own thread, a
//! little over 100 ms ahead of the speakers, following changes to the mix while it plays
//! ([`PreviewStream::update`]) and feeding the meters ([`PreviewStream::meters`]). Speakers are
//! the caller's business: the stream only hands out PCM.
//!
//! Sources are read through each asset's `proxy` when it exists on disk, and clips whose media
//! is missing are left out rather than failing the preview (an export reports them instead).
//!
//! Costs: one [`render_frame`] decodes every visible video at that time, in parallel (40–120 ms
//! depending on how far a source must decode from its previous keyframe), so scrubbing should
//! drop requests while one is in flight. Stills, titles and 3D devices are kept between frames.
//! When a stream falls behind (many 4K layers without proxies, heavy 3D on the CPU) frames
//! arrive late; their `pts` says when they belong.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kimchi_audio::meter::Meters;
use kimchi_audio::mixer::{Mixer, Mode, SourceOpener};
use kimchi_core::Project;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::export;
use crate::render::Renderer;
use crate::{Caps, MediaError, MediaResult, Tools};

/// Sample rate of [`AudioChunk`]s.
pub const SAMPLE_RATE: u32 = 48_000;
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
    /// The mixer thread's inbox (a newer project) and its stop flag.
    mixing: Option<Arc<Live>>,
    meters: Arc<Meters>,
    from: f64,
    fps: f64,
    duration: f64,
    size: (u32, u32),
}

/// Frames decoded ahead of the consumer (≈ 0.2 s at 30 fps).
const FRAMES_AHEAD: usize = 6;
/// Audio frames per [`AudioChunk`] (≈ 10.7 ms).
const CHUNK_FRAMES: usize = 512;
/// Chunks mixed ahead (≈ 85 ms): with the device's own buffer, the sound leaves the mixer a
/// little over 100 ms before it is heard, so a change to the mix is heard that soon.
const CHUNKS_AHEAD: usize = 8;

/// What the mixer thread and the stream share.
struct Live {
    project: Mutex<Option<Arc<Project>>>,
    stop: AtomicBool,
}

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
            mixing: None,
            meters: Arc::new(Meters::default()),
            from,
            fps,
            duration: (end - from).max(0.0),
            size: (width, height),
        };
        if end - from < 1e-3 {
            stream.duration = 0.0;
            return Ok(stream);
        }
        let sound = project.clone();
        let project = crate::render::playable(tools, project);
        let caps = Caps::detect(tools).await?;

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

        if sound.tracks.iter().any(|t| !kimchi_audio::mixer::heard(&sound, t).is_empty()) {
            let (tx, rx) = mpsc::channel(CHUNKS_AHEAD);
            stream.audio = Some(rx);
            let live = Arc::new(Live { project: Mutex::new(None), stop: AtomicBool::new(false) });
            stream.mixing = Some(live.clone());
            let opener: Arc<dyn SourceOpener> = crate::audio::FfmpegOpener::new(tools.clone());
            let meters = stream.meters.clone();
            let sound = Arc::new(sound);
            std::thread::Builder::new()
                .name("kimchi-mixer".into())
                .spawn(move || {
                    let mut mixer = match Mixer::new(sound, opener, SAMPLE_RATE, Mode::Realtime) {
                        Ok(m) => m.with_meters(meters),
                        Err(e) => {
                            tracing::warn!("preview sound: {e}");
                            return;
                        }
                    };
                    mixer.seek(from);
                    mixer.prime(std::time::Duration::from_millis(400));
                    let mut block = vec![[0.0f32; 2]; CHUNK_FRAMES];
                    let mut sent = 0u64;
                    let frames = ((end - from) * SAMPLE_RATE as f64).ceil() as u64;
                    while sent < frames && !live.stop.load(Ordering::Relaxed) {
                        if let Some(p) = live.project.lock().unwrap_or_else(|e| e.into_inner()).take() {
                            mixer.update(p);
                        }
                        let n = (frames - sent).min(CHUNK_FRAMES as u64) as usize;
                        mixer.render(&mut block[..n]);
                        let samples: Vec<f32> = block[..n].iter().flatten().copied().collect();
                        let start = from + sent as f64 / SAMPLE_RATE as f64;
                        sent += n as u64;
                        if tx.blocking_send(AudioChunk { start, samples }).is_err() {
                            return; // the stream was dropped
                        }
                    }
                })
                .map_err(MediaError::Io)?;
        }
        Ok(stream)
    }

    /// The mix changed while playing (a fader, an effect, a clip): the sound follows within a
    /// block or two, without restarting. Pictures keep playing what they started with.
    pub fn update(&self, project: Arc<Project>) {
        if let Some(live) = &self.mixing {
            *live.project.lock().unwrap_or_else(|e| e.into_inner()) = Some(project);
        }
    }

    /// The levels of what is playing: read them for the time the speakers are at
    /// ([`Meters::at`]).
    pub fn meters(&self) -> Arc<Meters> {
        self.meters.clone()
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
        if let Some(live) = &self.mixing {
            live.stop.store(true, Ordering::Relaxed);
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
