//! Voice-over takes: a microphone (cpal input, chosen by name like Settings › Audio says) into a
//! WAV file at the project's rate, with a live level for the meter and the take's place on the
//! timeline.
//!
//! The device's callback only copies samples into a lock-free ring and keeps the peak and
//! power; a writer thread converts to the project's rate (and to mono or stereo) and writes
//! 32-bit float WAV, so nothing the device sends is ever clipped or waited on. A [`Recording`]
//! holds the input stream, which belongs to the thread that opened it on some platforms: keep
//! it there (the window's main thread) and [`Recording::stop`] it there.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use serde::Serialize;

use crate::Result;

/// What to record.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The input's name (`None`: the system's default).
    pub device: Option<String>,
    /// The file's sample rate (the project's).
    pub rate: u32,
    /// 1 (mono, a voice) or 2; at most what the input has.
    pub channels: u16,
    pub path: PathBuf,
    /// Timeline time the take starts at (the playhead when recording starts).
    pub start: f64,
}

/// The level since it was last read (dBFS).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputLevel {
    pub peak_db: f64,
    pub rms_db: f64,
    /// Something hit full scale since the take started.
    pub clipped: bool,
}

/// A finished take.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Take {
    pub path: PathBuf,
    /// Where it goes on the timeline.
    pub start: f64,
    pub seconds: f64,
    pub rate: u32,
    pub channels: u16,
    pub peak_db: f64,
    /// Samples the writer couldn't keep up with (should be 0).
    pub dropped: u64,
    /// How long after its capture the device handed sound over (the clip can be moved earlier
    /// by this much to line up with what was heard).
    pub latency: f64,
    pub device: String,
}

/// Shared between the device's callback, the writer and the window.
#[derive(Default)]
struct Shared {
    peak: AtomicU32,
    squares: AtomicU64,
    count: AtomicU64,
    clipped: AtomicBool,
    dropped: AtomicU64,
    written: AtomicU64,
    latency_us: AtomicU64,
    stop: AtomicBool,
}

/// A take in progress.
pub struct Recording {
    _stream: cpal::Stream,
    shared: Arc<Shared>,
    writer: Option<std::thread::JoinHandle<Result<(u64, f32)>>>,
    options: Options,
    channels: u16,
    device: String,
}

/// Starts recording into `options.path`.
pub fn start(options: Options) -> Result<Recording> {
    if !(8_000..=192_000).contains(&options.rate) {
        return Err(format!("{} Hz isn't a rate to record at", options.rate));
    }
    let device = crate::devices::input(options.device.as_deref()).ok_or("There is no microphone or audio input on this computer.")?;
    let name = device.name().unwrap_or_else(|_| "input".into());
    let config = device.default_input_config().map_err(|e| format!("{name}: {e}"))?;
    let (device_rate, device_channels) = (config.sample_rate().0, config.channels().max(1));
    let channels = options.channels.clamp(1, 2).min(device_channels.max(1));
    if let Some(dir) = options.path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let shared = Arc::new(Shared::default());
    // Two seconds of the device's sound between its callback and the writer.
    let (producer, consumer) = rtrb::RingBuffer::<f32>::new(device_rate as usize * device_channels as usize * 2);
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => open::<f32>(&device, &config.config(), producer, shared.clone()),
        cpal::SampleFormat::I16 => open::<i16>(&device, &config.config(), producer, shared.clone()),
        cpal::SampleFormat::I32 => open::<i32>(&device, &config.config(), producer, shared.clone()),
        cpal::SampleFormat::U16 => open::<u16>(&device, &config.config(), producer, shared.clone()),
        other => Err(format!("{name} sends {other:?} samples, which kimchi can't record")),
    }?;
    let writer = Writer::create(&options.path, options.rate, channels, device_rate, device_channels)?;
    let s = shared.clone();
    let handle = std::thread::Builder::new()
        .name("kimchi-record".into())
        .spawn(move || writer.run(consumer, &s))
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| format!("{name}: {e}"))?;
    tracing::info!(device = %name, rate = device_rate, channels = device_channels, path = %options.path.display(), "recording");
    Ok(Recording { _stream: stream, shared, writer: Some(handle), options, channels, device: name })
}

fn open<T>(device: &cpal::Device, config: &cpal::StreamConfig, mut producer: rtrb::Producer<f32>, shared: Arc<Shared>) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                let ts = info.timestamp();
                if let Some(d) = ts.callback.duration_since(&ts.capture) {
                    shared.latency_us.store(d.as_micros() as u64, Ordering::Relaxed);
                }
                let (mut peak, mut squares) = (0.0f32, 0.0f64);
                for &x in data {
                    let v = <f32 as FromSample<T>>::from_sample_(x);
                    peak = peak.max(v.abs());
                    squares += (v as f64) * (v as f64);
                    if producer.push(v).is_err() {
                        shared.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                shared.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
                shared.squares.fetch_add((squares * 1e9) as u64, Ordering::Relaxed);
                shared.count.fetch_add(data.len() as u64, Ordering::Relaxed);
                if peak >= 0.999 {
                    shared.clipped.store(true, Ordering::Relaxed);
                }
            },
            |e| tracing::warn!("recording input: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

impl Recording {
    /// The input's level since the last call (for the meter by the record button).
    pub fn level(&self) -> InputLevel {
        let s = &self.shared;
        let peak = f32::from_bits(s.peak.swap(0, Ordering::Relaxed));
        let squares = s.squares.swap(0, Ordering::Relaxed) as f64 / 1e9;
        let count = s.count.swap(0, Ordering::Relaxed).max(1) as f64;
        let db = kimchi_core::audio::gain_to_db;
        InputLevel { peak_db: db(peak as f64), rms_db: db((squares / count).sqrt()), clipped: s.clipped.load(Ordering::Relaxed) }
    }

    /// Seconds written so far.
    pub fn seconds(&self) -> f64 {
        self.shared.written.load(Ordering::Relaxed) as f64 / self.options.rate as f64
    }

    /// Where the take starts on the timeline.
    pub fn start_time(&self) -> f64 {
        self.options.start
    }

    pub fn path(&self) -> &Path {
        &self.options.path
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    /// Stops and finishes the file.
    pub fn stop(mut self) -> Result<Take> {
        self.shared.stop.store(true, Ordering::Relaxed);
        let (frames, peak) = self.writer.take().ok_or("already stopped")?.join().map_err(|_| "the recording's writer stopped unexpectedly".to_string())??;
        Ok(Take {
            path: self.options.path.clone(),
            start: self.options.start,
            seconds: frames as f64 / self.options.rate as f64,
            rate: self.options.rate,
            channels: self.channels,
            peak_db: kimchi_core::audio::gain_to_db(peak as f64),
            dropped: self.shared.dropped.load(Ordering::Relaxed),
            latency: self.shared.latency_us.load(Ordering::Relaxed) as f64 / 1e6,
            device: self.device.clone(),
        })
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }
}

/// Converts the device's interleaved sound to the file's rate and channels, and writes it.
struct Writer {
    wav: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    channels: u16,
    device_channels: u16,
    /// Device frames per file frame, and the read position between device frames.
    step: f64,
    pos: f64,
    /// The last device frame (folded to the file's channels), for interpolation.
    last: [f32; 2],
    frames: u64,
    peak: f32,
}

impl Writer {
    fn create(path: &Path, rate: u32, channels: u16, device_rate: u32, device_channels: u16) -> Result<Self> {
        let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let wav = hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { wav, channels, device_channels: device_channels.max(1), step: device_rate as f64 / rate as f64, pos: 0.0, last: [0.0; 2], frames: 0, peak: 0.0 })
    }

    /// One device frame (all its channels) folded to the file's channels.
    fn fold(&self, frame: &[f32]) -> [f32; 2] {
        match (self.channels, frame.len()) {
            (_, 0) => [0.0; 2],
            (1, 1) => [frame[0], 0.0],
            // A voice: the channels averaged.
            (1, n) => [frame.iter().sum::<f32>() / n as f32, 0.0],
            (_, 1) => [frame[0], frame[0]],
            (_, _) => [frame[0], frame[1]],
        }
    }

    /// Takes device frames (interleaved) and writes what they make at the file's rate.
    fn push(&mut self, samples: &[f32]) -> Result<()> {
        for frame in samples.chunks_exact(self.device_channels as usize) {
            let now = self.fold(frame);
            // File frames between the last device frame and this one.
            while self.pos < 1.0 {
                let f = self.pos as f32;
                for c in 0..self.channels as usize {
                    let v = self.last[c] + (now[c] - self.last[c]) * f;
                    self.peak = self.peak.max(v.abs());
                    self.wav.write_sample(v).map_err(|e| e.to_string())?;
                }
                self.frames += 1;
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.last = now;
        }
        Ok(())
    }

    fn run(mut self, mut consumer: rtrb::Consumer<f32>, shared: &Shared) -> Result<(u64, f32)> {
        let mut buf = vec![];
        loop {
            let stopping = shared.stop.load(Ordering::Relaxed);
            let n = consumer.slots();
            // Whole device frames only.
            let n = n - n % self.device_channels as usize;
            if n > 0 {
                buf.clear();
                if let Ok(chunk) = consumer.read_chunk(n) {
                    buf.extend(chunk.into_iter());
                }
                self.push(&buf)?;
                shared.written.store(self.frames, Ordering::Relaxed);
            }
            if stopping && consumer.slots() < self.device_channels as usize {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.wav.finalize().map_err(|e| e.to_string())?;
        Ok((self.frames, self.peak))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_are_written_at_the_projects_rate() {
        let dir = tempfile::tempdir().unwrap();
        // A stereo 44.1 kHz input into a mono 48 kHz voice-over.
        let path = dir.path().join("take.wav");
        let mut w = Writer::create(&path, 48_000, 1, 44_100, 2).unwrap();
        let input: Vec<f32> = (0..44_100).flat_map(|i| {
            let v = (std::f32::consts::TAU * 440.0 * i as f32 / 44_100.0).sin() * 0.5;
            [v, v]
        }).collect();
        for chunk in input.chunks(512) {
            w.push(chunk).unwrap();
        }
        let (frames, peak) = (w.frames, w.peak);
        w.wav.finalize().unwrap();
        assert!((frames as i64 - 48_000).abs() <= 2, "{frames}");
        assert!((peak - 0.5).abs() < 0.01);
        let r = hound::WavReader::open(&path).unwrap();
        assert_eq!((r.spec().sample_rate, r.spec().channels), (48_000, 1));
        // The tone kept its pitch: 440 zero crossings upwards in a second.
        let s: Vec<f32> = r.into_samples::<f32>().map(|x| x.unwrap()).collect();
        let ups = s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((ups as i64 - 440).abs() <= 1, "{ups}");
        // A mono microphone into a stereo take: both sides.
        let path = dir.path().join("stereo.wav");
        let mut w = Writer::create(&path, 48_000, 2, 48_000, 1).unwrap();
        w.push(&[0.25; 480]).unwrap();
        w.wav.finalize().unwrap();
        let s: Vec<f32> = hound::WavReader::open(&path).unwrap().into_samples::<f32>().map(|x| x.unwrap()).collect();
        assert_eq!(s.len(), 960);
        assert_eq!(&s[100..104], [0.25; 4]);
    }

    #[test]
    fn without_a_microphone_recording_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let options = Options { device: None, rate: 48_000, channels: 1, path: dir.path().join("t.wav"), start: 2.0 };
        match start(options.clone()) {
            // A machine with an input records (briefly) and finishes the file.
            Ok(rec) => {
                assert_eq!(rec.start_time(), 2.0);
                std::thread::sleep(Duration::from_millis(200));
                let _ = rec.level();
                let take = rec.stop().unwrap();
                assert!(take.path.exists() && take.start == 2.0);
            }
            Err(e) => assert!(!e.is_empty()),
        }
        assert!(start(Options { rate: 1, ..options }).is_err());
    }
}
