//! The playhead and the playback clock, in their own entity so a playing
//! timeline only re-renders what shows time (preview, playhead, clock), not
//! the whole window.
//!
//! Playing at 1× streams picture and sound. J/K/L shuttle (backwards, or
//! faster than real time) moves the playhead on a timer instead and the
//! preview shows the frames it can render on the way, without sound.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, Task};
use kimchi_control::Session;

pub struct Playback {
    session: Arc<Session>,
    pub playhead: f64,
    pub playing: bool,
    /// Start over at the end while playing.
    pub looping: bool,
    /// Wall clock and playhead when playback (re)started.
    clock: Option<(Instant, f64)>,
    pub duration: f64,
    pub fps: f64,
    /// Bumped on every seek, so the preview can drop stale frames.
    pub generation: u64,
    /// Shuttle speed (J/L): 0 when not shuttling, else −8…8 but never 1 (that is `playing`).
    pub shuttle: f64,
    shuttle_clock: Option<(Instant, f64)>,
    _shuttle: Option<Task<()>>,
}

/// Fastest shuttle, either way.
const MAX_SHUTTLE: f64 = 8.0;

impl Playback {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            playhead: 0.0,
            playing: false,
            looping: false,
            clock: None,
            duration: 0.0,
            fps: 30.0,
            generation: 0,
            shuttle: 0.0,
            shuttle_clock: None,
            _shuttle: None,
        }
    }

    /// Playing or shuttling: the playhead is moving on its own.
    pub fn moving(&self) -> bool {
        self.playing || self.shuttle != 0.0
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.stop_shuttle();
        self.playing = false;
        self.clock = None;
        self.playhead = 0.0;
        self.generation += 1;
        self.publish();
        cx.notify();
    }

    /// Keeps the project's length and frame rate up to date.
    pub fn set_project(&mut self, duration: f64, fps: f64) {
        self.duration = duration;
        self.fps = fps.max(1.0);
    }

    pub fn seek(&mut self, t: f64, cx: &mut Context<Self>) {
        self.stop_shuttle();
        self.set_time(t, cx);
    }

    fn set_time(&mut self, t: f64, cx: &mut Context<Self>) {
        let snapped = ((t.max(0.0)) * self.fps).round() / self.fps;
        self.playhead = snapped;
        if self.playing {
            self.clock = Some((Instant::now(), snapped));
        }
        self.generation += 1;
        self.publish();
        cx.notify();
    }

    pub fn step(&mut self, frames: f64, cx: &mut Context<Self>) {
        self.pause(cx);
        let t = self.playhead + frames / self.fps;
        self.seek(t, cx);
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.moving() { self.pause(cx) } else { self.play(cx) }
    }

    pub fn play(&mut self, cx: &mut Context<Self>) {
        self.stop_shuttle();
        if self.playing {
            return;
        }
        if self.playhead >= self.duration - 1.0 / self.fps {
            self.playhead = 0.0;
        }
        self.playing = true;
        self.clock = Some((Instant::now(), self.playhead));
        self.generation += 1;
        self.publish();
        cx.notify();
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if self.shuttle != 0.0 {
            self.stop_shuttle();
            cx.notify();
        }
        if !self.playing {
            return;
        }
        self.playing = false;
        self.clock = None;
        self.generation += 1;
        self.publish();
        cx.notify();
    }

    // ---- shuttle (J/K/L) --------------------------------------------------------

    /// L: play; pressed again while playing, faster (2×, 4×, 8×).
    pub fn shuttle_forward(&mut self, cx: &mut Context<Self>) {
        if self.shuttle > 0.0 {
            self.start_shuttle((self.shuttle * 2.0).min(MAX_SHUTTLE), cx);
        } else if self.playing {
            self.start_shuttle(2.0, cx);
        } else {
            self.play(cx);
        }
    }

    /// J: play backwards; pressed again, faster.
    pub fn shuttle_back(&mut self, cx: &mut Context<Self>) {
        let rate = if self.shuttle < 0.0 { (self.shuttle * 2.0).max(-MAX_SHUTTLE) } else { -1.0 };
        self.start_shuttle(rate, cx);
    }

    fn start_shuttle(&mut self, rate: f64, cx: &mut Context<Self>) {
        // Leaves the 1× stream: the timer drives the playhead from here.
        if self.playing {
            self.playing = false;
            self.clock = None;
        }
        self.shuttle = rate;
        self.shuttle_clock = Some((Instant::now(), self.playhead));
        self._shuttle = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                if !this.update(cx, |p, cx| p.shuttle_tick(cx)).unwrap_or(false) {
                    break;
                }
            }
        }));
        self.generation += 1;
        self.publish();
        cx.notify();
    }

    fn stop_shuttle(&mut self) {
        self.shuttle = 0.0;
        self.shuttle_clock = None;
        self._shuttle = None;
    }

    /// Returns whether the shuttle goes on.
    fn shuttle_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((at, from)) = self.shuttle_clock else { return false };
        let t = from + at.elapsed().as_secs_f64() * self.shuttle;
        let end = t <= 0.0 || t >= self.duration;
        self.set_time(t.clamp(0.0, self.duration), cx);
        if end {
            // The task ends with this turn.
            self.shuttle = 0.0;
            self.shuttle_clock = None;
            cx.notify();
        }
        !end
    }

    /// Advances the playhead from the clock; returns whether still playing.
    /// Called once per frame by the preview while playing.
    pub fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((at, from)) = self.clock else { return false };
        let t = from + at.elapsed().as_secs_f64();
        if t >= self.duration {
            if self.looping && self.duration > 0.0 {
                self.clock = Some((Instant::now(), 0.0));
                self.playhead = 0.0;
                self.generation += 1;
            } else {
                self.playhead = self.duration;
                self.playing = false;
                self.clock = None;
                self.publish();
                cx.notify();
                return false;
            }
        } else {
            self.playhead = t;
        }
        self.publish();
        cx.notify();
        true
    }

    /// Re-anchors the clock (the audio device reports where it really is).
    pub fn resync(&mut self, t: f64) {
        if self.playing {
            self.clock = Some((Instant::now(), t));
            self.playhead = t;
        }
    }

    fn publish(&self) {
        let (playhead, playing) = (self.playhead, self.playing);
        self.session.update_ui_state(|s| {
            s.playhead = playhead;
            s.playing = playing;
        });
    }
}
