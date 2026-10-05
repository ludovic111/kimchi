//! [`FfmpegOpener`]: a clip's sound decoded by ffmpeg, one process per playing clip, for the
//! mixer. ffmpeg reads any file it can (codec, sample rate, channel layout) and applies the
//! clip's speed (time-stretched when the pitch is kept, like tape when it isn't) and pitch shift
//! (rubberband when this ffmpeg has it); reversed clips are decoded in short chunks from the end
//! back. More than two channels are folded to stereo here, with ryolune's law (ITU-R BS.775:
//! centre and surrounds at -3 dB, LFE dropped), and mono plays at full level on both sides.
//!
//! A reader thread keeps a small queue filled, so the mixer never waits in real time: what isn't
//! decoded yet is played as silence and skipped when it arrives.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use kimchi_audio::Frame;
use kimchi_audio::mixer::{SourceOpener, SourceReader};
use kimchi_core::{Asset, Clip};

use crate::Tools;

/// Decoded before a clip's first sample, then cut by timestamp: input seeking alone isn't
/// sample-exact in every ffmpeg (9.0 starts AAC ~15 ms off and short), the timestamps are.
const PREROLL: f64 = 0.25;
/// Seconds of decoded sound queued ahead of the mixer.
const QUEUE_SECONDS: f64 = 1.0;
/// Timeline seconds per chunk when playing backwards.
const REVERSE_CHUNK: f64 = 4.0;

/// Opens clip sounds with ffmpeg: one raw stereo f32 stream per clip, with speed, reverse and
/// pitch applied.
pub struct FfmpegOpener {
    pub tools: Tools,
}

impl FfmpegOpener {
    pub fn new(tools: Tools) -> Arc<Self> {
        Arc::new(Self { tools })
    }
}

/// The file to read a sound from: the original, else its proxy (a transcode of it).
pub(crate) fn sound_path(asset: &Asset) -> Result<PathBuf, String> {
    let original = PathBuf::from(&asset.path);
    if original.is_file() {
        return Ok(original);
    }
    if let Some(proxy) = asset.proxy.as_deref().map(PathBuf::from).filter(|p| p.is_file()) {
        return Ok(proxy);
    }
    Err(format!("missing media file {}", asset.path))
}

impl SourceOpener for FfmpegOpener {
    fn open(&self, clip: &Clip, asset: &Asset, from: f64, rate: u32) -> kimchi_audio::Result<Box<dyn SourceReader>> {
        let path = sound_path(asset)?;
        Ok(Box::new(Reader::start(self.tools.clone(), path, clip.clone(), from, rate)))
    }
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Frame>,
    ended: bool,
    error: Option<String>,
}

struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
    stop: AtomicBool,
    /// The ffmpeg being read, so dropping the reader stops it.
    child: Mutex<Option<Child>>,
    capacity: usize,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues decoded frames, waiting while the queue is full. False once the reader is gone.
    fn push(&self, frames: &[Frame]) -> bool {
        let mut q = self.lock();
        for chunk in frames.chunks(4096) {
            while q.frames.len() >= self.capacity && !self.stop.load(Ordering::Relaxed) {
                q = self.changed.wait(q).unwrap_or_else(|e| e.into_inner());
            }
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            q.frames.extend(chunk);
            self.changed.notify_all();
        }
        true
    }

    fn finish(&self, error: Option<String>) {
        let mut q = self.lock();
        q.ended = true;
        if q.error.is_none() {
            q.error = error;
        }
        self.changed.notify_all();
    }
}

struct Reader {
    shared: Arc<Shared>,
}

impl Reader {
    fn start(tools: Tools, path: PathBuf, clip: Clip, from: f64, rate: u32) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
            child: Mutex::new(None),
            capacity: (QUEUE_SECONDS * rate as f64) as usize,
        });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new().name("kimchi-sound".into()).spawn(move || {
            let result = decode(&worker, &tools, &path, &clip, from, rate);
            let error = result.err().map(|e| format!("couldn't decode the sound of {}: {e}", path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())));
            worker.finish(error);
        });
        if let Err(e) = spawned {
            shared.finish(Some(format!("couldn't start decoding: {e}")));
        }
        Self { shared }
    }
}

impl SourceReader for Reader {
    fn read(&mut self, out: &mut [Frame]) -> usize {
        let mut n = 0;
        let mut q = self.shared.lock();
        loop {
            let k = q.frames.len().min(out.len() - n);
            for (slot, f) in out[n..n + k].iter_mut().zip(q.frames.drain(..k)) {
                *slot = f;
            }
            n += k;
            if k > 0 {
                self.shared.changed.notify_all();
            }
            if n == out.len() || q.ended {
                return n;
            }
            q = self.shared.changed.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn available(&self) -> usize {
        let q = self.shared.lock();
        if q.ended { usize::MAX } else { q.frames.len() }
    }

    fn error(&self) -> Option<String> {
        self.shared.lock().error.clone()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.changed.notify_all();
        if let Some(child) = self.shared.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = child.kill();
        }
    }
}

/// Decodes the clip from timeline time `from` to its end into the queue.
fn decode(shared: &Shared, tools: &Tools, path: &Path, clip: &Clip, from: f64, rate: u32) -> Result<(), String> {
    let channels = channels(tools, path);
    let stretch = stretch(clip, rate, rubberband(tools));
    let total = ((clip.end() - from).max(0.0) * rate as f64).round() as usize;
    if total == 0 {
        return Ok(());
    }
    if !clip.reverse {
        let src = clip.source_time(from).max(0.0);
        let seek = (src - PREROLL).max(0.0);
        let mut filters = vec![format!("atrim=start={}", num(src - seek)), "asetpts=PTS-STARTPTS".into(), format!("aresample={rate}")];
        filters.extend(stretch);
        run(shared, tools, path, seek, &filters, channels, rate, Some(total))?;
        return Ok(());
    }
    // Backwards: chunks from the latest source time down, each decoded forwards, reversed and
    // stretched, then padded or cut to the frames it covers on the timeline.
    let speed = clip.speed.clamp(0.01, 100.0);
    let top = clip.source_time(from);
    let bottom = clip.in_point.max(0.0);
    let mut emitted = 0usize;
    let mut k = 0;
    while emitted < total && !shared.stop.load(Ordering::Relaxed) {
        let hi = top - k as f64 * REVERSE_CHUNK * speed;
        if hi <= bottom + 1e-9 {
            break;
        }
        let lo = (hi - REVERSE_CHUNK * speed).max(bottom);
        let seek = (lo - PREROLL).max(0.0);
        let mut filters = vec![
            format!("atrim=start={}:end={}", num(lo - seek), num(hi - seek)),
            "asetpts=PTS-STARTPTS".into(),
            format!("aresample={rate}"),
            "areverse".into(),
        ];
        filters.extend(stretch.clone());
        let until = ((((top - lo) / speed) * rate as f64).round() as usize).min(total);
        let want = until.saturating_sub(emitted);
        let got = run(shared, tools, path, seek, &filters, channels, rate, Some(want))?;
        if got < want && !shared.push(&vec![[0.0; 2]; want - got]) {
            return Ok(());
        }
        emitted += want;
        k += 1;
    }
    Ok(())
}

/// One ffmpeg run into the queue: `limit` frames at most (cut there). Returns the frames queued.
#[allow(clippy::too_many_arguments)]
fn run(shared: &Shared, tools: &Tools, path: &Path, seek: f64, filters: &[String], channels: Option<usize>, rate: u32, limit: Option<usize>) -> Result<usize, String> {
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error"].map(OsString::from).to_vec();
    if seek > 1e-6 {
        args.extend(["-ss".into(), num(seek).into()]);
    }
    args.extend(["-i".into(), crate::probe::input_path(path)]);
    args.extend(["-vn", "-sn", "-dn", "-map", "0:a:0", "-af"].map(OsString::from));
    let mut chain = filters.to_vec();
    chain.push("aformat=sample_fmts=flt".into());
    args.push(chain.join(",").into());
    // Up to 7.1 is folded here; anything wider by ffmpeg.
    let channels = match channels {
        Some(c @ 1..=8) => c,
        _ => {
            args.extend(["-ac", "2"].map(OsString::from));
            2
        }
    };
    args.extend(["-ar".into(), rate.to_string().into(), "-f".into(), "f32le".into(), "-".into()]);
    let mut cmd = crate::process::blocking(&tools.ffmpeg);
    cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "ffmpeg wasn't found".to_string(),
        _ => e.to_string(),
    })?;
    let mut stdout = child.stdout.take().ok_or("no output from ffmpeg")?;
    let mut stderr = child.stderr.take().ok_or("no output from ffmpeg")?;
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    *shared.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    let frame_bytes = channels * 4;
    let mut buf = vec![0u8; frame_bytes * 2048];
    let mut carry = 0usize;
    let mut queued = 0usize;
    let mut frames = Vec::with_capacity(2048);
    let mut stopped = false;
    loop {
        let k = match stdout.read(&mut buf[carry..]) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let have = carry + k;
        let whole = have / frame_bytes;
        frames.clear();
        for f in buf[..whole * frame_bytes].chunks_exact(frame_bytes) {
            let mut s = [0f32; 8];
            for (c, b) in f.chunks_exact(4).enumerate() {
                s[c] = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            frames.push(fold(&s[..channels]));
        }
        let take = limit.map_or(frames.len(), |l| frames.len().min(l - queued));
        if !shared.push(&frames[..take]) {
            stopped = true;
            break;
        }
        queued += take;
        if limit.is_some_and(|l| queued >= l) {
            stopped = true;
            break;
        }
        buf.copy_within(whole * frame_bytes..have, 0);
        carry = have - whole * frame_bytes;
    }
    let child = shared.child.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(mut child) = child else { return Ok(queued) };
    if stopped {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(queued);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let errors = errors.join().unwrap_or_default();
    if !status.success() && !shared.stop.load(Ordering::Relaxed) {
        return Err(crate::process::summarize(&errors, status));
    }
    Ok(queued)
}

/// One frame of up to eight channels folded to stereo, as ryolune does: mono to both sides at
/// full level; 3.0, 5.0, 5.1, 7.0 and 7.1 by ITU-R BS.775 (centre and surrounds at -3 dB, LFE
/// dropped, scaled so full-scale input can't clip); quad and other layouts alternate sides.
pub(crate) fn fold(frame: &[f32]) -> Frame {
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match frame.len() {
        0 => [0.0; 2],
        1 => [frame[0], frame[0]],
        2 => [frame[0], frame[1]],
        3 => {
            let n = 1.0 / (1.0 + H);
            [(frame[0] + H * frame[2]) * n, (frame[1] + H * frame[2]) * n]
        }
        5 => {
            let n = 1.0 / (1.0 + 2.0 * H);
            [(frame[0] + H * frame[2] + H * frame[3]) * n, (frame[1] + H * frame[2] + H * frame[4]) * n]
        }
        6 => {
            let n = 1.0 / (1.0 + 2.0 * H);
            [(frame[0] + H * frame[2] + H * frame[4]) * n, (frame[1] + H * frame[2] + H * frame[5]) * n]
        }
        7 => {
            let n = 1.0 / (1.0 + 3.0 * H);
            [(frame[0] + H * (frame[2] + frame[3] + frame[5])) * n, (frame[1] + H * (frame[2] + frame[4] + frame[6])) * n]
        }
        8 => {
            let n = 1.0 / (1.0 + 3.0 * H);
            [(frame[0] + H * (frame[2] + frame[4] + frame[6])) * n, (frame[1] + H * (frame[2] + frame[5] + frame[7])) * n]
        }
        count => {
            let (mut l, mut r) = (0.0, 0.0);
            for (i, v) in frame.iter().enumerate() {
                if i % 2 == 0 { l += v } else { r += v }
            }
            [l / count.div_ceil(2) as f32, r / (count / 2).max(1) as f32]
        }
    }
}

/// The filters for the clip's speed and pitch at `rate`: varispeed by resampling when the pitch
/// may follow the speed, else a time stretch (rubberband when there, `atempo` otherwise) with
/// the pitch shifted by resampling.
pub(crate) fn stretch(clip: &Clip, rate: u32, rubberband: bool) -> Vec<String> {
    let speed = clip.speed.clamp(0.01, 100.0);
    let shift = 2f64.powf(clip.audio.pitch.clamp(-kimchi_core::audio::MAX_PITCH, kimchi_core::audio::MAX_PITCH) / 12.0);
    let ratio = shift * if clip.audio.preserve_pitch { 1.0 } else { speed };
    let same = |a: f64, b: f64| (a - b).abs() < 1e-9;
    if same(speed, 1.0) && same(ratio, 1.0) {
        return vec![];
    }
    if rubberband && !same(ratio, speed) {
        return vec![format!("rubberband=tempo={}:pitch={}", num(speed), num(ratio))];
    }
    let mut f = vec![];
    // `asetrate` takes whole rates: the tempo left over is corrected by `atempo`.
    let mut actual = 1.0;
    if !same(ratio, 1.0) {
        let shifted = (rate as f64 * ratio).round().max(1.0);
        actual = shifted / rate as f64;
        f.push(format!("asetrate={shifted}"));
        f.push(format!("aresample={rate}"));
    }
    f.extend(atempo(speed / actual).into_iter().map(|t| format!("atempo={}", num(t))));
    f
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

/// Channels of the file's first sound stream (asked once per file).
fn channels(tools: &Tools, path: &Path) -> Option<usize> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<usize>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(c) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return *c;
    }
    let out = crate::process::blocking(&tools.ffprobe)
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels", "-of", "csv=p=0"])
        .arg(crate::probe::input_path(path))
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    let found = out.and_then(|o| String::from_utf8_lossy(&o.stdout).trim().lines().next().and_then(|l| l.trim().parse().ok()));
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), found);
    found
}

/// Whether this ffmpeg has the rubberband filter (better time stretching and pitch shifting).
pub(crate) fn rubberband(tools: &Tools) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(&b) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&tools.ffmpeg) {
        return b;
    }
    let found = crate::process::blocking(&tools.ffmpeg)
        .args(["-hide_banner", "-filters"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some("rubberband")));
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(tools.ffmpeg.clone(), found);
    found
}

/// Compact decimal for filter arguments.
fn num(x: f64) -> String {
    let x = if x.abs() < 5e-10 { 0.0 } else { x };
    let t = format!("{x:.9}");
    let t = t.trim_end_matches('0');
    if t.ends_with('.') { format!("{t}0") } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_fold_like_ryolune() {
        assert_eq!(fold(&[0.5]), [0.5, 0.5]);
        assert_eq!(fold(&[0.25, -0.5]), [0.25, -0.5]);
        let full = fold(&[1.0; 6]);
        assert!((full[0] - 1.0).abs() < 1e-6 && full[0] == full[1]);
        assert_eq!(fold(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]), [0.0, 0.0], "LFE is dropped");
        let centre = fold(&[0.0, 0.0, 1.0]);
        assert!(centre[0] > 0.3 && centre[0] == centre[1]);
        for frame in [&[1.0f32; 3][..], &[1.0; 5], &[1.0; 7], &[1.0; 8]] {
            let [l, r] = fold(frame);
            assert!(l <= 1.0 + 1e-6 && r <= 1.0 + 1e-6);
        }
        assert_eq!(fold(&[0.6, 0.2, 0.2, 0.0]), [0.4, 0.1]);
    }

    #[test]
    fn speed_and_pitch_become_filters() {
        assert_eq!(atempo(3.0), [2.0, 1.5]);
        assert_eq!(atempo(0.25), [0.5, 0.5]);
        assert!(atempo(1.0).is_empty());
        let mut c = Clip::new("c", 0.0, 1.0, kimchi_core::ClipContent::Solid { color: "#000".into() });
        assert!(stretch(&c, 48_000, true).is_empty());
        c.speed = 2.0;
        // Kept pitch: a time stretch.
        assert_eq!(stretch(&c, 48_000, false), ["atempo=2.0"]);
        assert_eq!(stretch(&c, 48_000, true), ["rubberband=tempo=2.0:pitch=1.0"]);
        // Like tape: resampled, faster and higher.
        c.audio.preserve_pitch = false;
        assert_eq!(stretch(&c, 48_000, true), ["asetrate=96000", "aresample=48000"]);
        // A fifth up at normal speed, without rubberband: resampled up, then slowed back.
        c.speed = 1.0;
        c.audio.preserve_pitch = true;
        c.audio.pitch = 7.0;
        let f = stretch(&c, 48_000, false);
        assert_eq!(f[0], format!("asetrate={}", (48_000.0 * 2f64.powf(7.0 / 12.0)).round()));
        assert!(f[2].starts_with("atempo=0.667"), "{f:?}");
        assert!(stretch(&c, 48_000, true)[0].starts_with("rubberband=tempo=1.0:pitch=1.498"));
    }
}
