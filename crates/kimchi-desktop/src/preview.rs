//! Preview engine glue: composited frames from kimchi-media (the same compositor as the export,
//! so the preview is what renders) and the mixer's sound out of the speakers while playing.
//!
//! The sound goes from the preview stream's real-time mixer straight to the device: the audio
//! callback takes the mixer's chunks as it needs them (about 100 ms are mixed ahead), so a fader
//! moved while playing ([`AudioBuffer::update`]) is heard almost at once, and the time of what
//! the speakers play ([`AudioBuffer::time`]) is the clock the picture can follow.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use gpui::RenderImage;
use kimchi_audio::meter::{Meters, Snapshot};
use kimchi_control::Session;
use kimchi_core::Project;
use kimchi_media::preview::{AudioChunk, CHANNELS, Frame, PreviewStream, SAMPLE_RATE};
use parking_lot::Mutex;
use tokio::sync::mpsc;

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

/// Starts streaming from `from`: frames come out of `frames` in order; sound goes to `audio`
/// (when the project has any), which also carries changes to the mix back to the stream.
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
    audio.attach(stream.audio(), stream.meters());
    let mut frames = frames;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! {
            next = stream.next_frame() => {
                let Some((pts, frame)) = next.map_err(|e| e.to_string())? else { break };
                if frames.send((pts, frame)).await.is_err() {
                    break;
                }
            }
            _ = tick.tick() => {}
        }
        if let Some(p) = audio.take_update() {
            stream.update(p);
        }
        if audio.closed.load(Ordering::Relaxed) && frames.is_closed() {
            break;
        }
    }
    // The pictures are done; the sound plays on to the end (or until playback stops).
    while !audio.closed.load(Ordering::Relaxed) && audio.playing() {
        if let Some(p) = audio.take_update() {
            stream.update(p);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

/// The mixer's sound on its way to the speakers, and what the window asks of it while
/// playing: the time being heard, the meters, changes to the mix.
pub struct AudioBuffer {
    inner: Mutex<Inner>,
    volume: Mutex<f32>,
    closed: AtomicBool,
    /// A newer project for the mixer, picked up by [`stream`].
    update: Mutex<Option<Arc<Project>>>,
}

#[derive(Default)]
struct Inner {
    chunks: Option<mpsc::Receiver<AudioChunk>>,
    /// Interleaved stereo not yet played, and the timeline time of its first frame.
    pending: VecDeque<f32>,
    pending_time: f64,
    /// The stream ended (every chunk was taken).
    ended: bool,
    meters: Option<Arc<Meters>>,
    /// Timeline time of the first frame of the last callback, and when it reaches the ears.
    anchor: Option<(f64, Instant)>,
}

impl AudioBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(Inner::default()), volume: Mutex::new(1.0), closed: AtomicBool::new(false), update: Mutex::new(None) })
    }

    /// Takes the stream's sound (none for a silent project) and its meters.
    fn attach(&self, chunks: Option<mpsc::Receiver<AudioChunk>>, meters: Arc<Meters>) {
        let mut inner = self.inner.lock();
        inner.ended = chunks.is_none();
        inner.chunks = if self.closed.load(Ordering::Relaxed) { None } else { chunks };
        inner.meters = Some(meters);
    }

    /// The mix changed while playing: the speakers follow within about 100 ms.
    pub fn update(&self, project: Arc<Project>) {
        *self.update.lock() = Some(project);
    }

    fn take_update(&self) -> Option<Arc<Project>> {
        self.update.lock().take()
    }

    /// Sound is still coming or playing.
    fn playing(&self) -> bool {
        let inner = self.inner.lock();
        !inner.ended || !inner.pending.is_empty()
    }

    /// Timeline time of what the speakers play now, once sound has started.
    pub fn time(&self) -> Option<f64> {
        let (t, at) = self.inner.lock().anchor?;
        let now = Instant::now();
        Some(if now >= at { t + (now - at).as_secs_f64() } else { t - (at - now).as_secs_f64() })
    }

    /// The meters for what is heard now (the newest reading when sound hasn't started).
    pub fn meters(&self) -> Option<Snapshot> {
        let meters = self.inner.lock().meters.clone()?;
        match self.time() {
            Some(t) => meters.at(t),
            None => meters.latest(),
        }
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let mut inner = self.inner.lock();
        inner.chunks = None;
        inner.pending.clear();
        inner.ended = true;
    }

    /// Fills `out` (interleaved, `channels` per frame, at `rate`) from the mixer's chunks,
    /// resampling linearly; silence where nothing is there yet. `pos` carries the fractional
    /// read position between calls; `delay` is how long until the first frame is heard.
    fn fill<T: cpal::SizedSample + cpal::FromSample<f32>>(&self, out: &mut [T], channels: usize, rate: u32, pos: &mut f64, delay: Duration) {
        let volume = *self.volume.lock();
        let mut inner = self.inner.lock();
        let ratio = SAMPLE_RATE as f64 / rate as f64;
        let frames = out.len() / channels.max(1);
        // Enough of the mixer's sound for this callback (and the next frame, to interpolate).
        let need = ((frames as f64 * ratio).ceil() as usize + 2) * CHANNELS as usize;
        while inner.pending.len() < need {
            let Some(rx) = inner.chunks.as_mut() else { break };
            match rx.try_recv() {
                Ok(chunk) => {
                    if inner.pending.is_empty() {
                        inner.pending_time = chunk.start;
                    }
                    inner.pending.extend(chunk.samples);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    inner.chunks = None;
                    inner.ended = true;
                    break;
                }
            }
        }
        if inner.pending.is_empty() {
            out.fill(T::from_sample(0.0f32));
            return;
        }
        let start = inner.pending_time + *pos / SAMPLE_RATE as f64;
        inner.anchor = Some((start, Instant::now() + delay));
        let q = &inner.pending;
        let at = |k: usize, ch: usize| q.get(k * CHANNELS as usize + ch).copied().unwrap_or(0.0);
        for frame in out.chunks_mut(channels) {
            let i = pos.floor() as usize;
            let f = (*pos - i as f64) as f32;
            let l = at(i, 0) * (1.0 - f) + at(i + 1, 0) * f;
            let r = at(i, 1) * (1.0 - f) + at(i + 1, 1) * f;
            for (c, s) in frame.iter_mut().enumerate() {
                let value = volume * if channels == 1 { (l + r) * 0.5 } else if c % 2 == 0 { l } else { r };
                let value = if value.is_finite() { value } else { 0.0 };
                *s = T::from_sample(if T::FORMAT.is_float() { value } else { value.clamp(-1.0, 1.0 - f32::EPSILON) });
            }
            *pos += ratio;
        }
        let consumed = (pos.floor() as usize).min(q.len() / CHANNELS as usize);
        inner.pending.drain(..consumed * CHANNELS as usize);
        inner.pending_time += consumed as f64 / SAMPLE_RATE as f64;
        // An underrun must not leave a read offset beyond the next chunk: otherwise every
        // subsequent callback skips the new sound too and playback never recovers.
        *pos = if inner.pending.is_empty() { 0.0 } else { *pos - consumed as f64 };
    }
}

/// The speakers. Kept on the main thread (cpal streams aren't `Send` on every platform).
pub struct AudioOut {
    _stream: cpal::Stream,
    buffer: Arc<AudioBuffer>,
}

impl AudioOut {
    /// Opens the output called `device` (Settings › Audio; the default when `None` or gone).
    pub fn open_on(buffer: Arc<AudioBuffer>, device: Option<&str>) -> Option<Self> {
        let device = kimchi_audio::devices::output(device)?;
        let supported = device.default_output_config().ok()?;
        // Prefer the mixer's own rate when the device can do it.
        let wanted = device
            .supported_output_configs()
            .ok()
            .and_then(|mut it| it.find(|c| c.min_sample_rate().0 <= SAMPLE_RATE && c.max_sample_rate().0 >= SAMPLE_RATE && c.sample_format() == cpal::SampleFormat::F32))
            .map(|c| c.with_sample_rate(cpal::SampleRate(SAMPLE_RATE)));
        let config = wanted.unwrap_or(supported);
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => output::<f32>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::F64 => output::<f64>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::I8 => output::<i8>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::I16 => output::<i16>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::I24 => output::<cpal::I24>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::I32 => output::<i32>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::I64 => output::<i64>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::U8 => output::<u8>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::U16 => output::<u16>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::U32 => output::<u32>(&device, &config.config(), buffer.clone()),
            cpal::SampleFormat::U64 => output::<u64>(&device, &config.config(), buffer.clone()),
            other => { tracing::warn!(?other, "unsupported audio output format"); return None; }
        }.map_err(|e| tracing::warn!("audio output: {e}")).ok()?;
        stream.play().map_err(|e| tracing::warn!("audio output: {e}")).ok()?;
        tracing::debug!(device = %device.name().unwrap_or_default(), rate, channels, "playing sound");
        Some(Self { _stream: stream, buffer })
    }
}

fn output<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device, config: &cpal::StreamConfig, buffer: Arc<AudioBuffer>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let (channels, rate) = (config.channels as usize, config.sample_rate.0);
    let mut pos = 0.0;
    device.build_output_stream::<T, _, _>(config, move |data, info| {
        let ts = info.timestamp();
        let delay = ts.playback.duration_since(&ts.callback).unwrap_or_default();
        buffer.fill(data, channels, rate, &mut pos, delay);
    }, |e| tracing::warn!("audio output: {e}"), None)
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.buffer.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_recovers_after_an_underrun_and_integer_devices_get_valid_samples() {
        let b = AudioBuffer::new();
        let (tx, rx) = mpsc::channel(2);
        b.attach(Some(rx), Arc::new(Meters::default()));
        tx.try_send(AudioChunk { start: 0.0, samples: vec![0.25, 0.75] }).unwrap();
        let mut pos = 0.0;
        let mut first = [0.0f32; 32];
        b.fill(&mut first, 2, 48_000, &mut pos, Duration::ZERO);
        assert_eq!(&first[..2], &[0.25, 0.75]);
        assert!(first[2..].iter().all(|v| *v == 0.0));
        tx.try_send(AudioChunk { start: 1.0, samples: vec![0.5, -0.5, 2.0, -2.0] }).unwrap();
        let mut pcm = [0i16; 4];
        b.fill(&mut pcm, 2, 48_000, &mut pos, Duration::ZERO);
        assert_eq!(pcm, [16384, -16384, i16::MAX, i16::MIN]);
        tx.try_send(AudioChunk { start: 2.0, samples: vec![0.25, 0.75] }).unwrap();
        let mut mono = [0.0f32; 1];
        b.fill(&mut mono, 1, 48_000, &mut pos, Duration::ZERO);
        assert_eq!(mono, [0.5]);
        b.close();
        let mut unsigned = [0u16; 4];
        b.fill(&mut unsigned, 2, 48_000, &mut pos, Duration::ZERO);
        assert_eq!(unsigned, [32768; 4]);
    }

    #[test]
    fn the_buffer_plays_chunks_in_time_and_resamples() {
        let b = AudioBuffer::new();
        let (tx, rx) = mpsc::channel(10);
        b.attach(Some(rx), Arc::new(Meters::default()));
        // 1 s of a ramp at 48 kHz, from timeline time 2.
        let samples: Vec<f32> = (0..48_000).flat_map(|i| [i as f32, -(i as f32)]).collect();
        for (k, chunk) in samples.chunks(2 * 4_800).enumerate() {
            tx.try_send(AudioChunk { start: 2.0 + k as f64 * 0.1, samples: chunk.to_vec() }).unwrap();
        }
        let mut pos = 0.0;
        // A 24 kHz stereo device: every other frame.
        let mut out = vec![0.0f32; 2 * 1_000];
        b.fill(&mut out, 2, 24_000, &mut pos, Duration::ZERO);
        assert_eq!(&out[..6], [0.0, 0.0, 2.0, -2.0, 4.0, -4.0]);
        let t = b.time().unwrap();
        assert!((t - 2.0).abs() < 0.01, "{t}");
        b.fill(&mut out, 2, 24_000, &mut pos, Duration::ZERO);
        assert_eq!(out[0], 2_000.0);
        assert!((b.time().unwrap() - 2.0 - 2_000.0 / 48_000.0).abs() < 0.01);
        // A 4-channel device gets left/right/left/right.
        b.fill(&mut out[..8], 4, 48_000, &mut pos, Duration::ZERO);
        assert_eq!(out[..4], [4_000.0, -4_000.0, 4_000.0, -4_000.0]);
        // Nothing left: silence, and no panic.
        b.close();
        b.fill(&mut out, 2, 48_000, &mut pos, Duration::ZERO);
        assert!(out.iter().all(|s| *s == 0.0));
        // The device opens or fails quietly (this machine may have no sound card).
        let _ = AudioOut::open_on(AudioBuffer::new(), Some("no such device"));
    }
}
