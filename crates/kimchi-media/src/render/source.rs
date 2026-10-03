//! Pictures from media files, decoded by ffmpeg into raw RGBA at the size they will be drawn.
//!
//! [`VideoStream`] runs one ffmpeg per playing clip, ahead of the compositor, with a small
//! bounded buffer (playback and export ask for frames in order). [`grab`] decodes one frame
//! (scrubbing). Stills are decoded once and kept.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use tiny_skia::Pixmap;

use crate::probe::{Source, source};
use crate::process::Features;
use crate::{MediaError, MediaResult, Tools};

/// Frames decoded ahead of the compositor, per stream.
const AHEAD: usize = 4;
/// Frames decoded at once when playing backwards.
const REVERSE_CHUNK: usize = 12;

/// An ffmpeg process that is killed when dropped.
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(tools: &Tools, args: &[OsString]) -> MediaResult<Child> {
    let mut cmd = crate::process::blocking(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"]).args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => MediaError::ToolsMissing,
        _ => MediaError::Io(e),
    })
}

/// Drain stderr while frames are read; retain only the last lines of ffmpeg's explanation.
fn read_stderr(child: &mut Child) -> std::thread::JoinHandle<String> {
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        let mut tail = std::collections::VecDeque::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tail.len() == 60 { tail.pop_front(); }
            tail.push_back(line);
        }
        Vec::from(tail).join("\n")
    })
}

/// How to decode `path` (orientation, alpha, HDR) with this ffmpeg; plain when ffprobe can't tell.
fn decoding(tools: &Tools, path: &Path) -> (Arc<Features>, Arc<Source>) {
    (Features::get(tools), source(tools, path).unwrap_or_default())
}

fn os(args: impl IntoIterator<Item = impl Into<OsString>>) -> Vec<OsString> {
    args.into_iter().map(Into::into).collect()
}

/// `-ss t` (input seeking), when `t` isn't the start.
fn seek(t: f64) -> Vec<OsString> {
    if t > 1e-6 { os(["-ss".to_string(), format!("{t:.6}")]) } else { vec![] }
}

/// Straight RGBA (ffmpeg) → premultiplied (tiny-skia), in place.
pub(crate) fn premultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let a = px[3];
        if a == 255 {
            continue;
        }
        let a16 = a as u16;
        px[0] = ((px[0] as u16 * a16 + 127) / 255) as u8;
        px[1] = ((px[1] as u16 * a16 + 127) / 255) as u8;
        px[2] = ((px[2] as u16 * a16 + 127) / 255) as u8;
    }
}

fn pixmap(mut rgba: Vec<u8>, w: u32, h: u32) -> Option<Pixmap> {
    premultiply(&mut rgba);
    Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h)?)
}

/// Upright width and height of a picture or video file (ffprobe, as [`crate::probe`] reports
/// them), for files that aren't media items.
pub(crate) fn dimensions(tools: &Tools, path: &Path) -> Option<(u32, u32)> {
    let s = source(tools, path)?;
    Some((s.width?, s.height?))
}

/// One frame of `path` at `time` seconds into it (0 for stills), `w`×`h`.
pub(crate) fn grab(tools: &Tools, path: &Path, time: Option<f64>, w: u32, h: u32) -> MediaResult<Pixmap> {
    let (f, src) = decoding(tools, path);
    // Well past the picture's end (a longer sound track): straight to its last picture.
    if let Some(t) = time
        && src.video_end.is_some_and(|end| t > end + 0.5)
    {
        return grab_last(tools, path, t, w, h);
    }
    let mut args = seek(time.unwrap_or(0.0));
    args.extend(src.input(&f, path));
    args.extend(os(["-an", "-frames:v", "1", "-vf"]));
    args.push(src.picture(&f, w, h).into());
    args.extend(os(["-f", "rawvideo", "-"]));
    let mut child = Proc(command(tools, &args)?);
    let errors = read_stderr(&mut child.0);
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    child.0.stdout.take().expect("piped").read_to_end(&mut out)?;
    let status = child.0.wait()?;
    let errors = errors.join().unwrap_or_default();
    if !status.success() { return Err(MediaError::Ffmpeg(crate::process::summarize(&errors, status))); }
    let len = w as usize * h as usize * 4;
    if out.len() < len {
        // Seeking at (or past) the very end gives nothing: take the last frame instead.
        if let Some(t) = time.filter(|t| *t > 0.05) {
            return grab_last(tools, path, t, w, h);
        }
        let why = crate::process::summarize(&errors, status);
        return Err(MediaError::Ffmpeg(format!("no picture from {}: {why}", path.display())));
    }
    out.truncate(len);
    pixmap(out, w, h).ok_or_else(|| MediaError::Unsupported("bad frame size".into()))
}

/// The last picture at or before `before` seconds (the media ends earlier, or its picture does).
fn grab_last(tools: &Tools, path: &Path, before: f64, w: u32, h: u32) -> MediaResult<Pixmap> {
    let (f, src) = decoding(tools, path);
    let until = src.video_end.map_or(before, |end| end.min(before));
    let len = w as usize * h as usize * 4;
    // Decode the last second, or ten when the picture stopped well before the file says.
    for back in [1.0, 10.0] {
        let from = (until - back).max(0.0);
        let mut args = seek(from);
        args.extend(src.input(&f, path));
        args.extend(os(["-an".to_string(), "-t".into(), format!("{:.6}", before - from + 0.05), "-vf".into()]));
        args.push(src.picture(&f, w, h).into());
        args.extend(os(["-f", "rawvideo", "-"]));
        let mut child = Proc(command(tools, &args)?);
        let errors = read_stderr(&mut child.0);
        let mut out = vec![];
        child.0.stdout.take().expect("piped").read_to_end(&mut out)?;
        let status = child.0.wait()?;
        let errors = errors.join().unwrap_or_default();
        if !status.success() { return Err(MediaError::Ffmpeg(crate::process::summarize(&errors, status))); }
        let n = out.len() / len;
        if n > 0 {
            let last = out[(n - 1) * len..n * len].to_vec();
            return pixmap(last, w, h).ok_or_else(|| MediaError::Unsupported("bad frame size".into()));
        }
        if from <= 0.0 {
            break;
        }
    }
    Err(MediaError::Ffmpeg(format!("no picture from {}", path.display())))
}

/// The decoder chunk being read by a reverse stream's thread; killed with the stream.
struct ProcSlot(Arc<std::sync::Mutex<Option<Proc>>>);

impl Drop for ProcSlot {
    fn drop(&mut self) {
        super::lock(&self.0).take();
    }
}

/// What keeps a stream's ffmpeg alive (and kills it when dropped).
enum Keep {
    Proc(Arc<std::sync::Mutex<Proc>>),
    Slot(ProcSlot),
}

/// A clip's pictures, in order, at a fixed rate.
pub(crate) struct VideoStream {
    frames: Receiver<MediaResult<Pixmap>>,
    _proc: Keep,
    /// Owner-local time (clip or scene seconds) of frame 0.
    first: f64,
    fps: f64,
    /// Index of the next frame to come out of `frames`.
    next: u64,
    last: Option<Arc<Pixmap>>,
    ended: bool,
    fallback: Option<(Tools, std::path::PathBuf, f64, f64, u32, u32)>,
    /// Where a forward stream started, to hold the media's last picture when it gave none (it
    /// started past the end of the picture: a longer sound track).
    tail: Option<(Tools, std::path::PathBuf, f64, u32, u32)>,
}

impl VideoStream {
    /// Frames of `path` from `source_start` seconds into it, `speed`× fast, at `fps`, `w`×`h`.
    /// Frame `k` is what the owner shows at local time `first + k / fps`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(tools: &Tools, path: &Path, source_start: f64, speed: f64, fps: f64, w: u32, h: u32, first: f64) -> MediaResult<Self> {
        Self::start_with_decode(tools, path, source_start, speed, fps, w, h, first, vec![])
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_with_decode(tools: &Tools, path: &Path, source_start: f64, speed: f64, fps: f64, w: u32, h: u32, first: f64, decode: Vec<String>) -> MediaResult<Self> {
        let (f, src) = decoding(tools, path);
        // libvpx (for the alpha) decodes on the CPU.
        let decode = if src.forces_decoder(&f) { vec![] } else { decode };
        let hardware = !decode.is_empty();
        let speed = speed.max(1e-3);
        let mut args = os(decode);
        args.extend(seek(source_start));
        args.extend(src.input(&f, path));
        args.extend(os(["-an", "-vf"]));
        let setpts = if (speed - 1.0).abs() > 1e-9 { format!("setpts=(PTS-STARTPTS)/{speed}") } else { "setpts=PTS-STARTPTS".into() };
        args.push(format!("{setpts},fps={},{}", crate::rate(fps), src.picture(&f, w, h)).into());
        args.extend(os(["-f", "rawvideo", "-"]));
        let mut child = command(tools, &args)?;
        let errors = read_stderr(&mut child);
        let mut stdout = child.stdout.take().expect("piped");
        let proc = Arc::new(std::sync::Mutex::new(Proc(child)));
        let (tx, rx): (SyncSender<MediaResult<Pixmap>>, Receiver<MediaResult<Pixmap>>) = sync_channel(AHEAD);
        let len = w as usize * h as usize * 4;
        let worker_proc = proc.clone();
        std::thread::Builder::new()
            .name("kimchi-decode".into())
            .spawn(move || {
                loop {
                    let mut buf = vec![0u8; len];
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    let Some(p) = pixmap(buf, w, h) else { break };
                    if tx.send(Ok(p)).is_err() {
                        break; // the stream was dropped
                    }
                }
                let status = worker_proc.lock().unwrap().0.wait();
                let errors = errors.join().unwrap_or_default();
                if let Ok(status) = status && !status.success() {
                    let _ = tx.send(Err(MediaError::Ffmpeg(crate::process::summarize(&errors, status))));
                }
            })
            .map_err(MediaError::Io)?;
        Ok(Self { frames: rx, _proc: Keep::Proc(proc), first, fps, next: 0, last: None, ended: false,
            fallback: hardware.then(|| (tools.clone(), path.to_path_buf(), source_start, speed, w, h)),
            tail: Some((tools.clone(), path.to_path_buf(), source_start, w, h)) })
    }

    /// A clip played backwards: frame `k` is what the owner shows at local time `first + k / fps`,
    /// source time `source_start − k × speed / fps` (held at the source's first frame).
    ///
    /// ffmpeg only decodes forwards, so a thread decodes the source in short chunks, from the
    /// latest one back, and hands each chunk's frames over in reverse.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_reversed(tools: &Tools, path: &Path, source_start: f64, speed: f64, fps: f64, w: u32, h: u32, first: f64) -> MediaResult<Self> {
        let speed = speed.max(1e-3);
        let step = speed / fps;
        let (tools, path) = (tools.clone(), path.to_path_buf());
        let (f, src) = decoding(&tools, &path);
        let (tx, rx): (SyncSender<MediaResult<Pixmap>>, Receiver<MediaResult<Pixmap>>) = sync_channel(AHEAD);
        // The decoder of the chunk being read, so dropping the stream stops it.
        let current: Arc<std::sync::Mutex<Option<Proc>>> = Arc::default();
        let held = current.clone();
        let len = w as usize * h as usize * 4;
        std::thread::Builder::new()
            .name("kimchi-decode-rev".into())
            .spawn(move || {
                // Frames past the end of the picture (the sound goes on) hold the next picture
                // that comes: the media's last.
                let mut owed = src.video_end.map(|end| end + 0.5).filter(|end| source_start > *end).map_or(0, |end| ((source_start - end) / step).floor() as u64);
                let mut k = owed;
                let mut skipped = 0.0;
                loop {
                    // This chunk: frames k … k + n − 1, the earliest source time last.
                    let top = source_start - k as f64 * step;
                    if top < -1e-6 {
                        break;
                    }
                    let n = (REVERSE_CHUNK as u64).min((top / step + 1e-6).floor() as u64 + 1);
                    let lo = (top - (n - 1) as f64 * step).max(0.0);
                    let mut args = seek(lo);
                    let setpts = if (speed - 1.0).abs() > 1e-9 { format!("setpts=(PTS-STARTPTS)/{speed}") } else { "setpts=PTS-STARTPTS".into() };
                    args.extend(src.input(&f, &path));
                    args.extend(os(["-an".to_string(), "-frames:v".into(), n.to_string(), "-vf".into()]));
                    args.push(format!("{setpts},fps={},{}", crate::rate(fps), src.picture(&f, w, h)).into());
                    args.extend(os(["-f", "rawvideo", "-"]));
                    let Ok(mut child) = command(&tools, &args) else { break };
                    let _errors = read_stderr(&mut child);
                    let mut stdout = child.stdout.take().expect("piped");
                    *super::lock(&held) = Some(Proc(child));
                    let mut chunk = Vec::with_capacity(n as usize);
                    loop {
                        let mut buf = vec![0u8; len];
                        if stdout.read_exact(&mut buf).is_err() {
                            break;
                        }
                        match pixmap(buf, w, h) {
                            Some(p) => chunk.push(p),
                            None => break,
                        }
                    }
                    *super::lock(&held) = None;
                    if chunk.is_empty() {
                        // Past the end of the picture (it ends earlier than the file says), or it
                        // can't be read: try earlier, for a minute of source at most.
                        owed += n;
                        k += n;
                        skipped += n as f64 * step;
                        if skipped > 60.0 {
                            break;
                        }
                        continue;
                    }
                    skipped = 0.0;
                    // Fewer frames than asked (the media ends early): the latest is held.
                    while chunk.len() < n as usize {
                        let last = chunk.last().cloned().expect("non-empty");
                        chunk.push(last);
                    }
                    let latest = chunk.last().cloned().expect("non-empty");
                    for p in std::iter::repeat_n(latest, owed as usize).chain(chunk.into_iter().rev()) {
                        if tx.send(Ok(p)).is_err() {
                            return; // the stream was dropped
                        }
                    }
                    owed = 0;
                    k += n;
                }
            })
            .map_err(MediaError::Io)?;
        Ok(Self { frames: rx, _proc: Keep::Slot(ProcSlot(current)), first, fps, next: 0, last: None, ended: false, fallback: None, tail: None })
    }

    /// The frame for owner-local time `local`; the last one once the media has ended.
    pub(crate) fn at(&mut self, local: f64) -> MediaResult<Option<Arc<Pixmap>>> {
        let want = ((local - self.first) * self.fps).round().max(0.0) as u64;
        while !self.ended && self.next <= want {
            match self.frames.recv() {
                Ok(Ok(p)) => {
                    self.last = Some(Arc::new(p));
                    self.next += 1;
                }
                outcome => {
                    if (matches!(&outcome, Ok(Err(_))) || self.last.is_none())
                        && let Some((tools, path, from, speed, w, h)) = self.fallback.take() {
                        tracing::warn!(path = %path.display(), "hardware decoder stopped; decoding in software");
                        *self = Self::start(&tools, &path, from, speed, self.fps, w, h, self.first)?;
                        return self.at(local);
                    }
                    if let Ok(Err(e)) = outcome { return Err(e); }
                    self.ended = true;
                    if self.last.is_none()
                        && let Some((tools, path, from, w, h)) = self.tail.take()
                    {
                        self.last = grab_last(&tools, &path, from, w, h).ok().map(Arc::new);
                    }
                }
            }
        }
        Ok(self.last.clone())
    }

    /// Can it still serve `local` (not already past it)?
    pub(crate) fn serves(&self, local: f64) -> bool {
        let want = ((local - self.first) * self.fps).round();
        want >= self.next as f64 - 1.0
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        // The decoder thread also owns the process. Kill it before releasing the receiver so
        // a blocked read cannot keep ffmpeg alive after playback/export is cancelled.
        match &self._proc {
            Keep::Proc(p) => {
                let _ = super::lock(p).0.kill();
            }
            Keep::Slot(slot) => {
                if let Some(p) = super::lock(&slot.0).as_mut() {
                    let _ = p.0.kill();
                }
            }
        }
    }
}
