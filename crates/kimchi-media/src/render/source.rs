//! Pictures from media files, decoded by ffmpeg into raw RGBA at the size they will be drawn.
//!
//! [`VideoStream`] runs one ffmpeg per playing clip, ahead of the compositor, with a small
//! bounded buffer (playback and export ask for frames in order). [`grab`] decodes one frame
//! (scrubbing). Stills are decoded once and kept.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use tiny_skia::Pixmap;

use crate::{MediaError, MediaResult, Tools};

/// Frames decoded ahead of the compositor, per stream.
const AHEAD: usize = 4;

/// An ffmpeg process that is killed when dropped.
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(tools: &Tools, args: &[String]) -> MediaResult<Child> {
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"]).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => MediaError::ToolsMissing,
        _ => MediaError::Io(e),
    })
}

/// `scale=…,format=rgba` for a frame of exactly `w`×`h`.
fn scale(w: u32, h: u32) -> String {
    format!("scale={w}:{h}:flags=bicubic,setsar=1,format=rgba")
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

/// Width and height of a picture or video file (ffprobe), for files that aren't media items.
pub(crate) fn dimensions(tools: &Tools, path: &Path) -> Option<(u32, u32)> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, (u32, u32)>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(d) = cache.lock().ok()?.get(path) {
        return Some(*d);
    }
    let mut cmd = Command::new(&tools.ffprobe);
    cmd.args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0:s=x"]).arg(path);
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (w, h) = text.trim().lines().next()?.split_once('x')?;
    let d = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    cache.lock().ok()?.insert(path.to_path_buf(), d);
    Some(d)
}

/// One frame of `path` at `time` seconds into it (0 for stills), `w`×`h`.
pub(crate) fn grab(tools: &Tools, path: &Path, time: Option<f64>, w: u32, h: u32) -> MediaResult<Pixmap> {
    let mut args = vec![];
    if let Some(t) = time.filter(|t| *t > 1e-6) {
        args.extend(["-ss".into(), format!("{t:.6}")]);
    }
    args.extend(["-i".into(), path.to_string_lossy().into_owned(), "-an".into(), "-frames:v".into(), "1".into()]);
    args.extend(["-vf".into(), scale(w, h), "-f".into(), "rawvideo".into(), "-".into()]);
    let mut child = Proc(command(tools, &args)?);
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    child.0.stdout.take().expect("piped").read_to_end(&mut out)?;
    let status = child.0.wait()?;
    let len = w as usize * h as usize * 4;
    if out.len() < len {
        // Seeking at (or past) the very end gives nothing: take the last frame instead.
        if let Some(t) = time.filter(|t| *t > 0.05) {
            return grab_last(tools, path, t, w, h);
        }
        return Err(MediaError::Ffmpeg(format!("no picture from {} ({status})", path.display())));
    }
    out.truncate(len);
    pixmap(out, w, h).ok_or_else(|| MediaError::Unsupported("bad frame size".into()))
}

fn grab_last(tools: &Tools, path: &Path, before: f64, w: u32, h: u32) -> MediaResult<Pixmap> {
    let from = (before - 1.0).max(0.0);
    let args: Vec<String> = vec![
        "-ss".into(),
        format!("{from:.6}"),
        "-i".into(),
        path.to_string_lossy().into_owned(),
        "-an".into(),
        "-vf".into(),
        scale(w, h),
        "-f".into(),
        "rawvideo".into(),
        "-".into(),
    ];
    let mut child = Proc(command(tools, &args)?);
    let mut out = vec![];
    child.0.stdout.take().expect("piped").read_to_end(&mut out)?;
    let len = w as usize * h as usize * 4;
    let n = out.len() / len;
    if n == 0 {
        return Err(MediaError::Ffmpeg(format!("no picture from {}", path.display())));
    }
    let last = out[(n - 1) * len..n * len].to_vec();
    pixmap(last, w, h).ok_or_else(|| MediaError::Unsupported("bad frame size".into()))
}

/// A clip's pictures, in order, at a fixed rate.
pub(crate) struct VideoStream {
    frames: Receiver<Pixmap>,
    _proc: Arc<std::sync::Mutex<Proc>>,
    /// Owner-local time (clip or scene seconds) of frame 0.
    first: f64,
    fps: f64,
    /// Index of the next frame to come out of `frames`.
    next: u64,
    last: Option<Arc<Pixmap>>,
    ended: bool,
}

impl VideoStream {
    /// Frames of `path` from `source_start` seconds into it, `speed`× fast, at `fps`, `w`×`h`.
    /// Frame `k` is what the owner shows at local time `first + k / fps`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(tools: &Tools, path: &Path, source_start: f64, speed: f64, fps: f64, w: u32, h: u32, first: f64) -> MediaResult<Self> {
        let speed = speed.max(1e-3);
        let mut args = vec![];
        if source_start > 1e-6 {
            args.extend(["-ss".into(), format!("{source_start:.6}")]);
        }
        args.extend(["-i".into(), path.to_string_lossy().into_owned(), "-an".into(), "-vf".into()]);
        let setpts = if (speed - 1.0).abs() > 1e-9 { format!("setpts=(PTS-STARTPTS)/{speed}") } else { "setpts=PTS-STARTPTS".into() };
        args.push(format!("{setpts},fps={fps},{}", scale(w, h)));
        args.extend(["-f".into(), "rawvideo".into(), "-".into()]);
        let mut child = command(tools, &args)?;
        let mut stdout = child.stdout.take().expect("piped");
        let proc = Arc::new(std::sync::Mutex::new(Proc(child)));
        let (tx, rx): (SyncSender<Pixmap>, Receiver<Pixmap>) = sync_channel(AHEAD);
        let len = w as usize * h as usize * 4;
        std::thread::Builder::new()
            .name("kimchi-decode".into())
            .spawn(move || {
                loop {
                    let mut buf = vec![0u8; len];
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    let Some(p) = pixmap(buf, w, h) else { break };
                    if tx.send(p).is_err() {
                        break; // the stream was dropped
                    }
                }
            })
            .map_err(MediaError::Io)?;
        Ok(Self { frames: rx, _proc: proc, first, fps, next: 0, last: None, ended: false })
    }

    /// The frame for owner-local time `local`; the last one once the media has ended.
    pub(crate) fn at(&mut self, local: f64) -> Option<Arc<Pixmap>> {
        let want = ((local - self.first) * self.fps).round().max(0.0) as u64;
        while !self.ended && self.next <= want {
            match self.frames.recv() {
                Ok(p) => {
                    self.last = Some(Arc::new(p));
                    self.next += 1;
                }
                Err(_) => self.ended = true,
            }
        }
        self.last.clone()
    }

    /// Can it still serve `local` (not already past it)?
    pub(crate) fn serves(&self, local: f64) -> bool {
        let want = ((local - self.first) * self.fps).round();
        want >= self.next as f64 - 1.0
    }
}
