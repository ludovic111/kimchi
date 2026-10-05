//! An effect chain (a clip's, a track's, a bus's or the master's) played by ryolune's engine:
//! each active slot is a plugin instance from `ryolune_engine::host::instantiate` (stock and
//! external alike, with the same ids and saved states as ryolune) mounted in a ryolune `Rack`.
//!
//! Instances belong to their slot id: when the project changes, a slot that keeps its id and its
//! plugin keeps its instance, so a reverb's tail and a plugin's state survive a fader move or a
//! parameter change. Bypassed slots stay loaded but are skipped. Automated parameters are sent
//! at the block's first frame and every [`AUTOMATION_GRAIN`] frames while they move, like
//! ryolune's renderer.
//!
//! A chain holds plugin editors, which belong to the thread that made them: create and run a
//! chain (and so the [`crate::mixer::Mixer`]) on one thread.

use std::collections::{BTreeMap, HashMap};

use kimchi_core::Insert;
use kimchi_core::anim::{Keyframes, number_at};
use ryolune_engine::plugin::{Editor, ProcessContext, Rack};

use crate::Frame;

/// Frames between two values of a moving automated parameter (ryolune's `AUTOMATION_GRAIN`).
pub const AUTOMATION_GRAIN: usize = ryolune_engine::render::AUTOMATION_GRAIN;

/// Saved parameters one slot can take before a block (more are dropped by the rack).
const PARAMETER_CAPACITY: usize = 1024;

/// A chain whose input and output have both been silent this long stops being processed until
/// sound comes in again (delays and reverbs ring out first). At least this, or the longest tail
/// a plugin reports.
const QUIET_SECONDS: f64 = 5.0;

struct Slot {
    id: String,
    plugin: String,
    blob: String,
    /// Where it is mounted in the rack.
    index: u32,
    bypassed: bool,
    /// Values sent so far (saved parameters).
    applied: BTreeMap<u32, f64>,
    defaults: HashMap<u32, f64>,
    latency: u32,
    tail: f64,
    /// Automated parameters: (parameter id, keyframe name).
    automated: Vec<(u32, String)>,
    editor: Box<dyn Editor>,
}

pub(crate) struct Chain {
    // Declared before the slots: the rack (the processors) drops before the editors.
    rack: Rack,
    slots: Vec<Slot>,
    free: Vec<u32>,
    rate: u32,
    /// Frames of silence in and out in a row.
    quiet: usize,
    idle: u32,
    /// Slots that couldn't be loaded, as (slot id, plugin, why).
    problems: Vec<(String, String, String)>,
}

impl Chain {
    pub(crate) fn new(rate: u32) -> Self {
        let capacity = kimchi_core::audio::MAX_EFFECTS;
        Self {
            rack: Rack::with_parameter_capacity(capacity, PARAMETER_CAPACITY),
            slots: vec![],
            free: (0..capacity as u32).rev().collect(),
            rate,
            quiet: 0,
            idle: 0,
            problems: vec![],
        }
    }

    /// Follows `inserts` (in order): keeps the instances whose slot keeps its id, plugin and
    /// saved state, loads new ones, unloads the rest, and sends changed parameter values.
    /// `owner` names the chain in warnings.
    pub(crate) fn sync(&mut self, inserts: &[Insert], owner: &str) {
        let mut old: Vec<Slot> = std::mem::take(&mut self.slots);
        let mut problems = vec![];
        for insert in inserts.iter().filter(|i| !i.is_empty()) {
            let plugin = plugin_id(insert);
            let reuse = old.iter().position(|s| s.id == insert.id && s.plugin == plugin && s.blob == insert.blob);
            let mut slot = match reuse {
                Some(i) => old.remove(i),
                None => {
                    if let Some(p) = self.problems.iter().find(|(id, p, _)| *id == insert.id && *p == plugin) {
                        // It failed before with the same settings: don't try again on every change.
                        problems.push(p.clone());
                        continue;
                    }
                    match self.load(insert, &plugin) {
                        Ok(slot) => slot,
                        Err(e) => {
                            tracing::warn!(effect = %insert.name, plugin = %plugin, chain = owner, "effect not loaded: {e}");
                            problems.push((insert.id.clone(), plugin.clone(), format!("{}: {e}", insert.name)));
                            continue;
                        }
                    }
                }
            };
            slot.bypassed = insert.is_bypassed();
            // Parameter values: changed ones are sent, ones no longer saved go back to default.
            for (&id, &value) in &insert.params {
                if slot.applied.get(&id).is_none_or(|v| (v - value).abs() > 1e-12) {
                    self.rack.set_param(slot.index, id, value);
                    slot.applied.insert(id, value);
                }
            }
            let gone: Vec<u32> = slot.applied.keys().copied().filter(|id| !insert.params.contains_key(id)).collect();
            for id in gone {
                slot.applied.remove(&id);
                if let Some(&d) = slot.defaults.get(&id) {
                    self.rack.set_param(slot.index, id, d);
                }
            }
            slot.latency = slot.editor.latency();
            self.slots.push(slot);
        }
        for slot in old {
            self.unload(slot);
        }
        self.problems = problems;
    }

    fn load(&mut self, insert: &Insert, plugin: &str) -> Result<Slot, String> {
        let index = self.free.pop().ok_or("the chain is full")?;
        let loaded = (|| {
            let mut instance = ryolune_engine::host::instantiate(plugin, &insert.name, self.rate)?;
            if !insert.blob.is_empty() {
                let bytes = ryolune_engine::host::decode_blob(&insert.blob)?;
                instance.editor.load(&bytes)?;
            }
            let processor = instance.processor.take().ok_or("the plugin has no audio processor")?;
            Ok::<_, String>((instance.editor, processor))
        })();
        let (editor, processor) = match loaded {
            Ok(v) => v,
            Err(e) => {
                self.free.push(index);
                return Err(e);
            }
        };
        if let Some(previous) = self.rack.mount(index, processor) {
            drop(previous);
        }
        for (&id, &value) in &insert.params {
            self.rack.set_param(index, id, value);
        }
        let defaults = editor.params().iter().map(|p| (p.id, p.default)).collect();
        Ok(Slot {
            id: insert.id.clone(),
            plugin: plugin.to_string(),
            blob: insert.blob.clone(),
            index,
            bypassed: insert.is_bypassed(),
            applied: insert.params.clone(),
            defaults,
            latency: editor.latency(),
            tail: editor.tail_seconds(),
            automated: vec![],
            editor,
        })
    }

    fn unload(&mut self, slot: Slot) {
        // The processor stops and goes before its editor.
        drop(self.rack.unmount(slot.index));
        self.free.push(slot.index);
        drop(slot);
    }

    /// Which parameters follow keyframes: `effects.<slot>.<param>` names in `keys`.
    pub(crate) fn automate(&mut self, keys: &Keyframes) {
        for slot in &mut self.slots {
            let automated: Vec<(u32, String)> = keys
                .keys()
                .filter_map(|name| kimchi_core::audio::parse_effect_key(name).filter(|(s, _)| *s == slot.id).map(|(_, p)| (p, name.clone())))
                .collect();
            // A parameter no longer automated goes back to its saved value.
            for (param, _) in slot.automated.iter().filter(|(p, _)| !automated.iter().any(|(q, _)| q == p)) {
                if let Some(&v) = slot.applied.get(param).or(slot.defaults.get(param)) {
                    self.rack.set_param(slot.index, *param, v);
                }
            }
            slot.automated = automated;
        }
    }

    /// Frames the chain delays the sound by (its running plugins' latency).
    pub(crate) fn latency(&self) -> usize {
        self.slots.iter().filter(|s| !s.bypassed).map(|s| s.latency as usize).sum()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.bypassed)
    }

    pub(crate) fn problems(&self) -> impl Iterator<Item = &str> {
        self.problems.iter().map(|(_, _, p)| p.as_str())
    }

    /// Silences tails and voices (a seek).
    pub(crate) fn reset(&mut self) {
        for s in &self.slots {
            self.rack.reset(s.index);
        }
        self.quiet = 0;
    }

    /// Runs `block` (at most `MAX_BLOCK` frames) through the chain. `automation`: the keyframes
    /// and the timeline time of the block's first frame (as heard at the chain's input).
    pub(crate) fn process(&mut self, block: &mut [Frame], ctx: &ProcessContext, automation: Option<(&Keyframes, f64)>) {
        if self.is_empty() {
            return;
        }
        let silent_in = crate::dsp::peak(block) < 1e-7;
        let quiet_limit = (self.slots.iter().map(|s| s.tail).filter(|t| t.is_finite()).fold(QUIET_SECONDS, f64::max).min(60.0) * self.rate as f64) as usize;
        if silent_in && self.quiet >= quiet_limit {
            return;
        }
        let n = block.len();
        let rate = self.rate as f64;
        for slot in self.slots.iter().filter(|s| !s.bypassed) {
            if let Some((keys, t0)) = automation {
                for (param, name) in &slot.automated {
                    let mut sent = None;
                    let mut frame = 0;
                    while frame < n {
                        if let Some(v) = number_at(keys, name, t0 + frame as f64 / rate)
                            && sent.is_none_or(|s: f64| (s - v).abs() > 1e-9)
                        {
                            self.rack.set_param_at(slot.index, frame as u32, *param, v);
                            sent = Some(v);
                        }
                        frame += AUTOMATION_GRAIN;
                    }
                }
            }
            self.rack.process(slot.index, block, &[], ctx);
        }
        crate::dsp::sanitize(block);
        if silent_in && crate::dsp::peak(block) < 1e-6 {
            self.quiet += n;
        } else {
            self.quiet = 0;
        }
        self.idle += 1;
        if self.idle >= 16 {
            self.idle = 0;
            for s in &mut self.slots {
                s.editor.idle();
            }
        }
    }

    /// Nothing comes out of it any more (input silent long enough): a finished clip's chain can go.
    pub(crate) fn rang_out(&self) -> bool {
        self.is_empty() || self.quiet >= (QUIET_SECONDS * self.rate as f64) as usize
    }
}

/// The descriptor id of an insert's plugin, with stock plugins under their current names
/// (songs from before ryolune's rename keep their effects).
fn plugin_id(insert: &Insert) -> String {
    let id = insert.plugin_id();
    match id.strip_prefix("stock:") {
        Some(name) => format!("stock:{}", ryolune_engine::model::current_stock_name(name)),
        None => id,
    }
}
