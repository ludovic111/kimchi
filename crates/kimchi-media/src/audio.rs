//! The project's sound through kimchi-audio's mixer, with ffmpeg decoding each clip
//! ([`FfmpegOpener`]): offline mixes (exports, stems, loudness, speech), scrub snippets, an
//! asset's samples (beat detection) and the whole audio timeline as a ryolune session.
//!
//! Everything here mixes on a blocking thread: the mixer holds plugin editors, which stay on
//! the thread that made them.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kimchi_audio::loudness::{Loudness, LoudnessMeter};
use kimchi_audio::mixer::{Mixer, Mode, Selection, SourceOpener};
use kimchi_audio::{Frame, SAMPLE_RATE};
use kimchi_core::{Asset, Clip, ClipContent, Id, Project};
use tokio_util::sync::CancellationToken;

use crate::{MediaError, MediaResult, Tools};

mod opener;
mod session;

pub use opener::{FfmpegOpener, atempo};
pub use session::write_ryolune_session;

/// What to measure or mix.
#[derive(Debug, Clone, Default)]
pub struct Range {
    /// Timeline seconds (the whole timeline by default).
    pub span: Option<(f64, f64)>,
    pub selection: Selection,
}

fn failed(e: impl std::fmt::Display) -> MediaError {
    MediaError::Io(std::io::Error::other(e.to_string()))
}

/// Runs `work` with a fresh offline mixer of `project` at `rate` on a blocking thread.
async fn with_mixer<T: Send + 'static>(
    tools: &Tools,
    project: &Project,
    rate: u32,
    selection: Selection,
    work: impl FnOnce(&mut Mixer) -> MediaResult<T> + Send + 'static,
) -> MediaResult<T> {
    let opener: Arc<dyn SourceOpener> = FfmpegOpener::new(tools.clone());
    let project = Arc::new(project.clone());
    tokio::task::spawn_blocking(move || {
        let mut mixer = Mixer::new(project, opener, rate, Mode::Offline).map_err(MediaError::Unsupported)?.with_selection(selection);
        work(&mut mixer)
    })
    .await
    .map_err(failed)?
}

/// `(from, to)` of `range` on `project`'s timeline.
fn span(project: &Project, range: &Range) -> MediaResult<(f64, f64)> {
    let end = project.duration();
    let (a, b) = range.span.unwrap_or((0.0, end));
    let (a, b) = (a.max(0.0), b.max(0.0));
    if !(a.is_finite() && b.is_finite()) || b - a < 1e-3 {
        return Err(MediaError::Unsupported("the range to mix is empty".into()));
    }
    Ok((a, b))
}

/// Renders `frames` frames from the mixer's position in blocks, handing each to `each`.
fn pull(mixer: &mut Mixer, frames: u64, mut each: impl FnMut(&[Frame]) -> MediaResult<()>) -> MediaResult<()> {
    let mut block = vec![[0.0f32; 2]; 4096];
    let mut left = frames;
    while left > 0 {
        let n = left.min(block.len() as u64) as usize;
        mixer.render(&mut block[..n]);
        each(&block[..n])?;
        left -= n as u64;
    }
    Ok(())
}

/// Sources that couldn't be read make an offline mix fail (an export must not lose its sound
/// quietly); effects that couldn't be loaded only warn.
fn check(mixer: &Mixer) -> MediaResult<()> {
    let problems = mixer.problems();
    if let Some(p) = problems.iter().find(|p| p.contains("missing media file") || p.contains("ffmpeg")) {
        return Err(MediaError::Unsupported(p.clone()));
    }
    for p in &problems {
        tracing::warn!("mix: {p}");
    }
    Ok(())
}

/// The mix of `range`, offline, at the project's sample rate.
pub async fn render(tools: &Tools, project: &Project, range: &Range) -> MediaResult<Vec<Frame>> {
    let (from, to) = span(project, range)?;
    let rate = rate(project);
    with_mixer(tools, project, rate, range.selection.clone(), move |m| {
        m.seek(from);
        let frames = ((to - from) * rate as f64).round() as u64;
        let mut out = Vec::with_capacity(frames as usize);
        pull(m, frames, |b| {
            out.extend_from_slice(b);
            Ok(())
        })?;
        check(m)?;
        Ok(out)
    })
    .await
}

/// Loudness of the mix of `range` (EBU R128).
pub async fn measure(tools: &Tools, project: &Project, range: &Range) -> MediaResult<Loudness> {
    let (from, to) = span(project, range)?;
    let rate = rate(project);
    with_mixer(tools, project, rate, range.selection.clone(), move |m| {
        m.seek(from);
        let mut meter = LoudnessMeter::new(rate);
        pull(m, ((to - from) * rate as f64).round() as u64, |b| {
            meter.push(b);
            Ok(())
        })?;
        check(m)?;
        Ok(meter.result())
    })
    .await
}

/// A project holding only `clip` (and its asset) on a plain track, at volume 1 without fades:
/// its source through its own effects.
fn alone(project: &Project, clip: Id) -> MediaResult<(Project, f64, f64)> {
    let c = project
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .find(|c| c.id == clip)
        .ok_or_else(|| MediaError::Unsupported(format!("there is no clip {clip}")))?;
    let mut c: Clip = c.clone();
    let ClipContent::Media { asset_id } = c.content else {
        return Err(MediaError::Unsupported(format!("{} has no sound", c.name)));
    };
    let asset = project.asset(asset_id).ok_or_else(|| MediaError::Unsupported(format!("{} has no media", c.name)))?;
    c.volume = 1.0;
    c.keyframes.remove("volume");
    c.keyframes.remove("pan");
    c.fade_in = 0.0;
    c.fade_out = 0.0;
    c.transition = None;
    c.audio.pan = 0.0;
    c.audio.muted = false;
    let (from, to) = (c.start, c.end());
    let mut p = Project::new("clip", project.settings.clone());
    p.tracks = vec![kimchi_core::Track::new(kimchi_core::TrackKind::Audio, "clip")];
    p.tracks[0].clips.push(c);
    p.assets = vec![asset.clone()];
    p.mixer.master.limiter = false;
    Ok((p, from, to))
}

/// Loudness of one clip on its own: its source through its own effects, at volume 1 (what
/// `audio.normalize` sets the volume from).
pub async fn clip_loudness(tools: &Tools, project: &Project, clip: Id) -> MediaResult<Loudness> {
    let (p, from, to) = alone(project, clip)?;
    measure(tools, &p, &Range { span: Some((from, to)), ..Default::default() }).await
}

/// A short piece of the mix from timeline time `t`, to hear while scrubbing.
pub async fn scrub(tools: &Tools, project: &Project, t: f64, seconds: f64) -> MediaResult<Vec<Frame>> {
    let seconds = seconds.clamp(0.01, 2.0);
    let rate = rate(project);
    let t = t.max(0.0);
    with_mixer(tools, project, rate, Selection::default(), move |m| {
        m.seek(t);
        let mut out = vec![[0.0; 2]; (seconds * rate as f64) as usize];
        m.render(&mut out);
        // A short fade at both ends so snippets played one after another don't click.
        let ramp = (out.len() / 8).min((0.005 * rate as f64) as usize).max(1);
        let n = out.len();
        for i in 0..ramp.min(n) {
            let g = i as f32 / ramp as f32;
            out[i] = [out[i][0] * g, out[i][1] * g];
            out[n - 1 - i] = [out[n - 1 - i][0] * g, out[n - 1 - i][1] * g];
        }
        Ok(out)
    })
    .await
}

/// A whole asset's sound, stereo at `rate` (beat detection, waveforms).
pub async fn asset_samples(tools: &Tools, asset: &Asset, rate: u32) -> MediaResult<Vec<Frame>> {
    let seconds = asset.duration().filter(|d| *d > 0.0).ok_or_else(|| MediaError::Unsupported(format!("{} has no sound", asset.name)))?;
    let clip = Clip::new(&asset.name, 0.0, seconds, ClipContent::Media { asset_id: asset.id });
    let opener = FfmpegOpener::new(tools.clone());
    let asset = asset.clone();
    tokio::task::spawn_blocking(move || {
        let mut reader = opener.open(&clip, &asset, 0.0, rate).map_err(MediaError::Unsupported)?;
        let mut out = vec![[0.0; 2]; (seconds * rate as f64).round() as usize];
        let got = reader.read(&mut out);
        if let Some(e) = reader.error() {
            return Err(MediaError::Ffmpeg(e));
        }
        out.truncate(got);
        Ok(out)
    })
    .await
    .map_err(failed)?
}

/// The mixer's sample rate for a project.
pub fn rate(project: &Project) -> u32 {
    match project.settings.sample_rate {
        8_000..=192_000 => project.settings.sample_rate,
        _ => SAMPLE_RATE,
    }
}

/// A mix written to a file of raw interleaved f32le stereo frames, for ffmpeg to encode.
#[derive(Debug, Clone)]
pub(crate) struct Mixdown {
    pub frames: u64,
    pub loudness: Loudness,
}

/// Mixes `from..to` of `project` at `rate` to `path` (raw f32le stereo), as fast as it goes.
/// With a loudness target on the master (and the master applied), the mix is measured first and
/// brought to the target, then limited. `progress` gets 0..1; cancelling stops it.
pub(crate) async fn mixdown(
    tools: &Tools,
    project: &Project,
    (from, to): (f64, f64),
    rate: u32,
    selection: Selection,
    path: &Path,
    progress: &(dyn Fn(f64) + Sync),
    cancel: &CancellationToken,
) -> MediaResult<Mixdown> {
    let frames = ((to - from) * rate as f64).round().max(0.0) as u64;
    let target = project.mixer.master.loudness.filter(|_| !selection.skip_master);
    let limit = project.mixer.master.limiter;
    let ceiling = project.mixer.master.ceiling_db;
    let done = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (d, s, out) = (done.clone(), stop.clone(), path.to_path_buf());
    let work = with_mixer(tools, project, rate, selection, move |m| {
        m.seek(from);
        // With a target, the limiter waits for the loudness gain.
        m.bypass_limiter(target.is_some());
        let raw = if target.is_some() { out.with_extension("unlimited") } else { out.clone() };
        let mut file = std::io::BufWriter::new(std::fs::File::create(&raw)?);
        let mut meter = LoudnessMeter::new(rate);
        let mut written = 0u64;
        let mut bytes = Vec::with_capacity(4096 * 8);
        let result = pull(m, frames, |b| {
            if s.load(Ordering::Relaxed) {
                return Err(MediaError::Cancelled);
            }
            meter.push(b);
            bytes.clear();
            for f in b {
                bytes.extend_from_slice(&f[0].to_le_bytes());
                bytes.extend_from_slice(&f[1].to_le_bytes());
            }
            file.write_all(&bytes)?;
            written += b.len() as u64;
            let share = if target.is_some() { 0.8 } else { 1.0 };
            d.store((share * written as f64 / frames.max(1) as f64).to_bits(), Ordering::Relaxed);
            Ok(())
        })
        .and_then(|()| check(m))
        .and_then(|()| file.flush().map_err(MediaError::from));
        drop(file);
        if let Err(e) = result {
            let _ = std::fs::remove_file(&raw);
            return Err(e);
        }
        let mut loudness = meter.result();
        if let Some(target) = target {
            // Second pass over the file: the gain to the target, then the limiter.
            let gain = kimchi_core::audio::db_to_gain(kimchi_audio::loudness::gain_to(&loudness, target)) as f32;
            let result = normalise(&raw, &out, rate, gain, limit.then_some(ceiling), frames, &d, &s);
            let _ = std::fs::remove_file(&raw);
            loudness = result?;
        }
        Ok(Mixdown { frames: written, loudness })
    });
    tokio::pin!(work);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        tokio::select! {
            r = &mut work => {
                progress(1.0);
                return r;
            }
            _ = cancel.cancelled(), if !stop.load(Ordering::Relaxed) => stop.store(true, Ordering::Relaxed),
            _ = tick.tick() => progress(f64::from_bits(done.load(Ordering::Relaxed)).clamp(0.0, 1.0)),
        }
    }
}

/// Copies the raw mix `from` to `to` with `gain` and the true-peak limiter (when a ceiling is
/// given), lined up again after the limiter's delay. Returns the result's loudness.
#[allow(clippy::too_many_arguments)]
fn normalise(from: &Path, to: &Path, rate: u32, gain: f32, ceiling: Option<f64>, frames: u64, done: &AtomicU64, stop: &AtomicBool) -> MediaResult<Loudness> {
    let mut input = std::io::BufReader::new(std::fs::File::open(from)?);
    let mut output = std::io::BufWriter::new(std::fs::File::create(to)?);
    let mut limiter = kimchi_audio::dsp::Limiter::new(rate, ceiling.unwrap_or(0.0));
    limiter.set_enabled(ceiling.is_some());
    let mut skip = limiter.latency();
    let mut meter = LoudnessMeter::new(rate);
    let mut bytes = vec![0u8; 4096 * 8];
    let mut block: Vec<Frame> = Vec::with_capacity(4096);
    let mut out = Vec::with_capacity(4096 * 8);
    let mut seen = 0u64;
    let mut emit = |block: &mut Vec<Frame>, limiter: &mut kimchi_audio::dsp::Limiter, skip: &mut usize| -> MediaResult<()> {
        limiter.process(block);
        let keep = &block[(*skip).min(block.len())..];
        *skip -= (*skip).min(block.len());
        meter.push(keep);
        out.clear();
        for f in keep {
            out.extend_from_slice(&f[0].to_le_bytes());
            out.extend_from_slice(&f[1].to_le_bytes());
        }
        output.write_all(&out)?;
        Ok(())
    };
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        let mut filled = 0;
        while filled < bytes.len() {
            match input.read(&mut bytes[filled..])? {
                0 => break,
                k => filled += k,
            }
        }
        if filled < 8 {
            break;
        }
        block.clear();
        for f in bytes[..filled - filled % 8].chunks_exact(8) {
            let l = f32::from_le_bytes([f[0], f[1], f[2], f[3]]) * gain;
            let r = f32::from_le_bytes([f[4], f[5], f[6], f[7]]) * gain;
            block.push([l, r]);
        }
        seen += block.len() as u64;
        emit(&mut block, &mut limiter, &mut skip)?;
        done.store((0.8 + 0.2 * seen as f64 / frames.max(1) as f64).to_bits(), Ordering::Relaxed);
    }
    // Flush what the limiter still holds.
    let mut tail = vec![[0.0f32; 2]; limiter.latency()];
    emit(&mut tail, &mut limiter, &mut 0)?;
    output.flush()?;
    drop(output);
    // The flush wrote the limiter's whole delay; cut the file back to the frames mixed.
    let file = std::fs::OpenOptions::new().write(true).open(to)?;
    file.set_len(frames * 8)?;
    Ok(meter.result())
}

#[cfg(test)]
mod tests;
