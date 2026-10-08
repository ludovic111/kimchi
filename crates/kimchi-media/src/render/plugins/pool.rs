//! The plugin instances a renderer draws with: one per (clip, slot), made the first time the
//! slot draws, kept between frames, and dropped when the slot or its clip is gone. When a
//! renderer is dropped (the preview makes one per scrubbed frame) its instances wait in a small
//! shared pool for the next renderer, so scrubbing doesn't remake them every frame.
//!
//! Each frame, a slot's parameters are its values at that time (the clip's keyframes already
//! applied by `Clip::effects_at`) over the plugin's defaults. A renderer draws one project, so
//! its slots live as long as it does; instances of slots that are gone age out of the spares. A plugin that is missing, refuses
//! to start or fails a frame is skipped (the picture shows without it) and logged once. One that
//! panics is switched off for the run ([`super::report_failure`]); one the person switched off is
//! skipped. When plugins change ([`super::generation`]: installed, rebuilt, switched), every
//! instance is made again, so a rebuilt plugin draws at once (hot reload).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use kimchi_core::{Effects, Id, PluginEffect, PluginValue};
use tiny_skia::Pixmap;

use super::{Instance, PluginInfo, PluginKind, RenderCtx, catalogue, value};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    clip: Id,
    slot: String,
    plugin: String,
}

/// Instances left by renderers that are gone, newest last.
type Spares = Mutex<Vec<(Key, Box<dyn Instance>)>>;

fn spares() -> &'static Spares {
    static S: OnceLock<Spares> = OnceLock::new();
    S.get_or_init(Default::default)
}
const SPARES: usize = 32;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Logs `message` the first time `key` comes up.
fn warn_once(key: String, message: impl FnOnce() -> String) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    if lock(SEEN.get_or_init(Default::default)).insert(key) {
        tracing::warn!("{}", message());
    }
}

/// See the module.
#[derive(Default)]
pub struct Pool {
    slots: HashMap<Key, Option<Box<dyn Instance>>>,
    infos: HashMap<String, Option<PluginInfo>>,
    /// [`super::generation`] the instances were made in.
    generation: u64,
}

impl Pool {
    /// Plugins changed since the instances were made: make them again.
    fn sync(&mut self) {
        let g = super::generation();
        if g != self.generation {
            self.slots.clear();
            self.infos.clear();
            lock(spares()).clear();
            self.generation = g;
        }
    }

    fn info(&mut self, id: &str) -> Option<PluginInfo> {
        if super::is_off(id) {
            return None;
        }
        self.infos
            .entry(id.to_string())
            .or_insert_with(|| {
                let found = catalogue::find(id);
                if found.is_none() {
                    warn_once(format!("missing {id}"), || format!("video plugin {id} isn't on this computer: clips that use it are drawn without it"));
                }
                found
            })
            .clone()
    }

    /// Are all of these plugins timeless (their result may be kept on a still picture)?
    pub fn timeless(&mut self, fx: &Effects) -> bool {
        self.sync();
        fx.active_plugins().all(|p| self.info(&p.plugin).is_none_or(|i| i.timeless))
    }

    fn instance(&mut self, key: &Key, info: &PluginInfo) -> Option<&mut Box<dyn Instance>> {
        if !self.slots.contains_key(key) {
            let spare = {
                let mut s = lock(spares());
                s.iter().rposition(|(k, _)| k == key).map(|i| s.remove(i).1)
            };
            let made = spare.map(Ok).unwrap_or_else(|| super::instantiate(info));
            let made = match made {
                Ok(i) => Some(i),
                Err(e) => {
                    warn_once(format!("start {}", info.id), || format!("video plugin {} couldn't start: {e}", info.name));
                    None
                }
            };
            self.slots.insert(key.clone(), made);
        }
        self.slots.get_mut(key).and_then(|i| i.as_mut())
    }

    fn draw(&mut self, clip: Id, slot: &PluginEffect, inputs: &[&Pixmap], size: (u32, u32), ctx: &RenderCtx) -> Option<Pixmap> {
        let info = self.info(&slot.plugin)?;
        let key = Key { clip, slot: slot.id.clone(), plugin: slot.plugin.clone() };
        let params: BTreeMap<String, PluginValue> = value::values(&info, &slot.params);
        let instance = self.instance(&key, &info)?;
        let mut out = Pixmap::new(size.0, size.1)?;
        // The SDK catches a plugin's panics; this catches the host's own glue (frei0r's conversions).
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| instance.render(&params, inputs, &mut out, ctx)));
        match drawn {
            Ok(Ok(())) => Some(out),
            Ok(Err(e)) if e.fatal => {
                super::report_failure(&info.id, &e.message);
                self.slots.remove(&key);
                None
            }
            Ok(Err(e)) => {
                let m = e.message;
                warn_once(format!("render {clip} {} {m}", slot.id), || format!("video plugin {} ({}) skipped: {m}", slot.name, info.name));
                None
            }
            Err(payload) => {
                let why = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "a panic".into());
                super::report_failure(&info.id, &format!("kimchi's host for it panicked: {why}"));
                self.slots.remove(&key);
                None
            }
        }
    }

    /// Runs the clip's plugins (effects and generators, not bypassed) on `pic`, first to last.
    /// `scale` is `pic`'s pixels per project pixel.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, clip: Id, fx: &Effects, pic: &mut Pixmap, time: f64, fps: f64, scale: f32, draft: bool) {
        self.sync();
        let ctx = RenderCtx { time, fps, scale, progress: 0.0, draft };
        let size = (pic.width(), pic.height());
        for slot in fx.active_plugins() {
            let kind = match self.info(&slot.plugin) {
                Some(i) => i.kind,
                None => continue,
            };
            let out = match kind {
                PluginKind::Effect => self.draw(clip, slot, &[&*pic], size, &ctx),
                PluginKind::Generator => self.draw(clip, slot, &[], size, &ctx),
                PluginKind::Transition => {
                    warn_once(format!("transition {}", slot.plugin), || format!("{} is a transition plugin: use it as a clip's transition (transition.set plugin=…)", slot.name));
                    None
                }
            };
            if let Some(out) = out {
                *pic = out;
            }
        }
    }

    /// A transition plugin from `a` to `b` at `progress`, or `None` when it can't draw (the
    /// caller draws the transition's kind instead).
    #[allow(clippy::too_many_arguments)]
    pub fn transition(&mut self, clip: Id, slot: &PluginEffect, a: &Pixmap, b: &Pixmap, progress: f32, time: f64, fps: f64, scale: f32, draft: bool) -> Option<Pixmap> {
        self.sync();
        if self.info(&slot.plugin)?.kind != PluginKind::Transition {
            warn_once(format!("not a transition {}", slot.plugin), || format!("{} isn't a transition plugin", slot.name));
            return None;
        }
        let ctx = RenderCtx { time, fps, scale, progress, draft };
        self.draw(clip, slot, &[a, b], (a.width(), a.height()), &ctx)
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let mut s = lock(spares());
        for (k, i) in self.slots.drain() {
            if let Some(i) = i {
                s.retain(|(other, _)| *other != k);
                s.push((k, i));
            }
        }
        let extra = s.len().saturating_sub(SPARES);
        s.drain(..extra);
    }
}
