//! Preview engine glue: composited frames from kimchi-media (the same ffmpeg
//! graph as the export, so the preview is what renders) and sound out of the
//! speakers while playing.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gpui::RenderImage;
use kimchi_control::Session;
use kimchi_core::Project;
use kimchi_media::preview::{AudioChunk, CHANNELS, Frame, PreviewStream, SAMPLE_RATE};
use parking_lot::Mutex;

/// One composited frame at `time`. Runs on Tokio.
pub async fn render(session: Arc<Session>, project: Arc<Project>, time: f64, width: u32, height: u32) -> Result<Frame, String> {
    let tools = session.tools()?;
    kimchi_media::preview::render_frame(&tools, &project, time, width, height).await.map_err(|e| e.to_string())
}

/// A frame as a GPUI image (BGRA).
pub fn to_image(frame: Frame) -> Option<Arc<RenderImage>> {
    let (w, h) = (frame.width, frame.height);
    let buf = image::RgbaImage::from_raw(w, h, frame.into_bgra())?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)])))
}

/// Starts streaming from `from`: frames come out of `frames` in order; sound
/// goes to `audio` (when the project has any).
pub async fn stream(
    session: Arc<Session>,
    project: Arc<Project>,
    from: f64,
    width: u32,
    height: u32,
    frames: futures::channel::mpsc::Sender<(f64, Frame)>,
    audio: Arc<AudioBuffer>,
) -> Result<(), String> {
    use futures::SinkExt;
    let tools = session.tools()?;
    let fps = project.settings.fps.clamp(1.0, 30.0);
    let mut stream = PreviewStream::start(&tools, &project, from, width, height, fps).await.map_err(|e| e.to_string())?;
    if let Some(mut rx) = stream.audio() {
        let audio = audio.clone();
        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                if !audio.push(chunk) {
                    break;
                }
            }
        });
    }
    let mut frames = frames;
    while let Some((pts, frame)) = stream.next_frame().await.map_err(|e| e.to_string())? {
        if frames.send((pts, frame)).await.is_err() {
            break;
        }
    }
    Ok(())
}

/// Interleaved stereo at [`SAMPLE_RATE`], filled by the decoder and drained by the device.
pub struct AudioBuffer {
    samples: Mutex<VecDeque<f32>>,
    volume: Mutex<f32>,
    closed: AtomicBool,
}

impl AudioBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { samples: Mutex::new(VecDeque::new()), volume: Mutex::new(1.0), closed: AtomicBool::new(false) })
    }

    /// Adds decoded sound; false once playback stopped.
    fn push(&self, chunk: AudioChunk) -> bool {
        if self.closed.load(Ordering::Relaxed) {
            return false;
        }
        self.samples.lock().extend(chunk.samples);
        true
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.samples.lock().clear();
    }
}

/// The speakers. Kept on the main thread (cpal streams aren't `Send` on every platform).
pub struct AudioOut {
    _stream: cpal::Stream,
    buffer: Arc<AudioBuffer>,
}

impl AudioOut {
    /// Opens the default output device and plays whatever `buffer` receives.
    pub fn open(buffer: Arc<AudioBuffer>) -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let supported = device.default_output_config().ok()?;
        // Prefer the stream's own rate when the device can do it.
        let wanted = device
            .supported_output_configs()
            .ok()
            .and_then(|mut it| it.find(|c| c.min_sample_rate().0 <= SAMPLE_RATE && c.max_sample_rate().0 >= SAMPLE_RATE && c.sample_format() == cpal::SampleFormat::F32))
            .map(|c| c.with_sample_rate(cpal::SampleRate(SAMPLE_RATE)));
        let config = wanted.unwrap_or(supported);
        if config.sample_format() != cpal::SampleFormat::F32 {
            tracing::warn!("audio device doesn't take f32 samples; playing silently");
            return None;
        }
        let out_channels = config.channels() as usize;
        let ratio = SAMPLE_RATE as f64 / config.sample_rate().0 as f64;
        let buf = buffer.clone();
        // Fractional read position for resampling (linear).
        let mut pos = 0.0f64;
        let stream = device
            .build_output_stream::<f32, _, _>(
                &config.config(),
                move |data: &mut [f32], _| {
                    let volume = *buf.volume.lock();
                    let mut q = buf.samples.lock();
                    for frame in data.chunks_mut(out_channels) {
                        let i = pos.floor() as usize;
                        let f = (pos - i as f64) as f32;
                        let at = |k: usize, ch: usize| q.get(k * CHANNELS as usize + ch).copied().unwrap_or(0.0);
                        let l = at(i, 0) * (1.0 - f) + at(i + 1, 0) * f;
                        let r = at(i, 1) * (1.0 - f) + at(i + 1, 1) * f;
                        for (c, out) in frame.iter_mut().enumerate() {
                            *out = volume * if c % 2 == 0 { l } else { r };
                        }
                        pos += ratio;
                    }
                    let consumed = pos.floor() as usize;
                    let drain = (consumed * CHANNELS as usize).min(q.len());
                    q.drain(..drain);
                    pos -= consumed as f64;
                },
                |e| tracing::warn!("audio output: {e}"),
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(Self { _stream: stream, buffer })
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.buffer.close();
    }
}
