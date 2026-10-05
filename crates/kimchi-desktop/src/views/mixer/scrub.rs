//! Hearing the sound while scrubbing: when the playhead moves while nothing plays (dragged on
//! the ruler, stepped a frame, a cut jumped to) and Settings › Audio's "Hear while scrubbing"
//! is on, a short piece of the mix at the new time (`kimchi_media::audio::scrub`) goes to the
//! speakers. One piece is rendered at a time; a newer playhead replaces the one waiting.

use std::collections::VecDeque;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, StreamTrait};
use gpui::{App, Global};
use parking_lot::Mutex;

use crate::store::StoreExt;

/// Seconds of sound a scrub plays.
const SNIPPET: f64 = 0.12;

#[derive(Default)]
pub struct Scrubber {
    /// The speakers, opened on the first scrub (kept on the main thread: cpal streams aren't
    /// `Send` everywhere).
    out: Option<Output>,
    /// A render is running; `wanted` is the newest time asked for meanwhile.
    busy: bool,
    wanted: Option<f64>,
    last: Option<f64>,
}

impl Global for Scrubber {}

struct Output {
    _stream: cpal::Stream,
    queue: Arc<Mutex<VecDeque<[f32; 2]>>>,
    /// The mix's rate and the device's.
    rate: u32,
}

impl Output {
    fn open(device: &str, rate: u32) -> Option<Self> {
        let device = kimchi_audio::devices::output(Some(device).filter(|d| !d.is_empty()))?;
        let config = device.default_output_config().ok()?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return None;
        }
        let channels = config.channels() as usize;
        let ratio = rate as f64 / config.sample_rate().0 as f64;
        let queue: Arc<Mutex<VecDeque<[f32; 2]>>> = Arc::default();
        let q = queue.clone();
        let mut pos = 0.0f64;
        let stream = device
            .build_output_stream::<f32, _, _>(
                &config.config(),
                move |data: &mut [f32], _| {
                    let mut q = q.lock();
                    for frame in data.chunks_mut(channels) {
                        let i = pos.floor() as usize;
                        let f = (pos - i as f64) as f32;
                        let at = |k: usize| q.get(k).copied().unwrap_or([0.0; 2]);
                        let (a, b) = (at(i), at(i + 1));
                        for (c, out) in frame.iter_mut().enumerate() {
                            let ch = c % 2;
                            *out = a[ch] * (1.0 - f) + b[ch] * f;
                        }
                        if !q.is_empty() {
                            pos += ratio;
                        }
                    }
                    let used = (pos.floor() as usize).min(q.len());
                    q.drain(..used);
                    pos -= used as f64;
                    if q.is_empty() {
                        pos = 0.0;
                    }
                },
                |e| tracing::warn!("scrub output: {e}"),
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(Self { _stream: stream, queue, rate })
    }
}

/// The playhead moved: play the sound there when scrubbing is on and nothing plays.
pub fn on_playhead(cx: &mut App) {
    let store = cx.store();
    let s = store.read(cx);
    let pb = s.playback.read(cx);
    if pb.moving() || !s.settings.audio.scrub || s.project.is_none() {
        return;
    }
    let t = pb.playhead;
    let scrubber = cx.default_global::<Scrubber>();
    if scrubber.last.is_some_and(|l| (l - t).abs() < 1e-6) {
        return;
    }
    scrubber.last = Some(t);
    scrubber.wanted = Some(t);
    if !scrubber.busy {
        render_next(cx);
    }
}

fn render_next(cx: &mut App) {
    let store = cx.store();
    let Some(t) = cx.default_global::<Scrubber>().wanted.take() else {
        cx.default_global::<Scrubber>().busy = false;
        return;
    };
    let s = store.read(cx);
    let (Some(project), session) = (s.project.clone(), s.session.clone()) else { return };
    let device = s.settings.audio.output_device.clone();
    cx.default_global::<Scrubber>().busy = true;
    let rate = kimchi_media::audio::rate(&project);
    let task = gpui_tokio::Tokio::spawn(cx, async move {
        let tools = session.tools().ok()?;
        kimchi_media::audio::scrub(&tools, &project, t, SNIPPET).await.ok()
    });
    cx.spawn(async move |cx| {
        let frames = task.await.ok().flatten();
        cx.update(|cx| {
            if let Some(frames) = frames.filter(|f| f.iter().any(|x| x[0] != 0.0 || x[1] != 0.0)) {
                let sc = cx.default_global::<Scrubber>();
                if sc.out.as_ref().is_none_or(|o| o.rate != rate) {
                    sc.out = Output::open(&device, rate);
                }
                if let Some(o) = &sc.out {
                    let mut q = o.queue.lock();
                    // The newest piece replaces what hasn't played yet.
                    q.clear();
                    q.extend(frames);
                }
            }
            render_next(cx);
        });
    })
    .detach();
}
