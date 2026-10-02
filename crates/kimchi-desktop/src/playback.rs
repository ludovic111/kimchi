//! The playhead and the playback clock, in their own entity so a playing
//! timeline only re-renders what shows time (preview, playhead, clock), not
//! the whole window.

use std::sync::Arc;
use std::time::Instant;

use gpui::Context;
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
}

impl Playback {
    pub fn new(session: Arc<Session>) -> Self {
        Self { session, playhead: 0.0, playing: false, looping: false, clock: None, duration: 0.0, fps: 30.0, generation: 0 }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
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
        if self.playing { self.pause(cx) } else { self.play(cx) }
    }

    pub fn play(&mut self, cx: &mut Context<Self>) {
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
        if !self.playing {
            return;
        }
        self.playing = false;
        self.clock = None;
        self.generation += 1;
        self.publish();
        cx.notify();
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
