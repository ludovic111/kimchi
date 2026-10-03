//! Expressions: small formulas that compute a property every frame, like After Effects
//! expressions or Blender drivers (`"time * 90"`, `"wiggle(2, 30)"`, `"value + sin(time) * 20"`).
//!
//! The language is a safe slice of JavaScript: numbers, vectors (`[x, y, z]`), strings (colours
//! like `"#ff5a36"`) and booleans; `+ - * / % **` (`^` is a power too), comparisons, `&& || !`,
//! `?:`, function calls, `let` bindings and `;` between steps (the last one is the value). There
//! are no loops, no functions of your own and no I/O, so a formula always finishes quickly.
//!
//! A formula is read once ([`compile`], cached by its text) and run every frame ([`eval`])
//! against a [`Context`]: the time, the property's keyframed value, its keyframes, the scene's
//! other properties. [`GUIDE`] explains it to people and agents.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::anim::{KeyValue, Keyframe};

mod guide;
mod noise;
mod parse;
mod run;
#[cfg(test)]
mod tests;

pub use guide::GUIDE;

/// The longest formula accepted, in bytes.
pub const MAX_LEN: usize = 8000;

/// What a formula can see while it runs. Scene evaluation provides one per property.
pub trait Context {
    /// Seconds: the scene's time (or the composition's, inside one).
    fn time(&self) -> f64;
    /// The property's value at [`Context::time`] before the formula (keyframes applied).
    fn value(&self) -> Option<KeyValue>;
    /// The property's keyframed value at another time.
    fn value_at(&self, t: f64) -> Option<KeyValue>;
    /// The property's keyframes (empty when it has none).
    fn keys(&self) -> &[Keyframe];
    /// Another thing's property (keyframes and its own formula applied) at `t`, or now.
    fn prop(&self, id: &str, name: &str, t: Option<f64>) -> Result<KeyValue, String>;
    /// Position among its siblings, from 1.
    fn index(&self) -> f64;
    fn fps(&self) -> f64;
    /// The scene's (or composition's) length in seconds.
    fn duration(&self) -> f64;
    /// Different for every property of every thing, so `random()` and `wiggle()` differ too.
    fn seed(&self) -> u64;
}

/// A formula read and checked, ready to run.
#[derive(Debug)]
pub struct Program {
    src: Arc<str>,
    body: Vec<parse::Stmt>,
    slots: usize,
}

impl Program {
    /// The formula's text.
    pub fn source(&self) -> &str {
        &self.src
    }
}

/// Checks that a formula reads: syntax, names and functions (with "did you mean" hints).
pub fn check(src: &str) -> Result<(), String> {
    compile(src).map(|_| ())
}

/// Reads a formula, or takes it from the cache when the same text was read before.
pub fn compile(src: &str) -> Result<Arc<Program>, String> {
    let cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
    if let Ok(mut c) = cache.lock() {
        c.tick += 1;
        let tick = c.tick;
        if let Some(entry) = c.map.get_mut(src) {
            entry.1 = tick;
            return entry.0.clone();
        }
    }
    let made = parse::program(src).map(Arc::new);
    if let Ok(mut c) = cache.lock() {
        if c.map.len() >= CACHE_SIZE {
            c.shrink();
        }
        let tick = c.tick;
        c.map.insert(src.to_string(), (made.clone(), tick));
    }
    made
}

/// Runs a formula. Errors name the function or operator and where it is in the formula.
pub fn eval(program: &Program, ctx: &dyn Context) -> Result<KeyValue, String> {
    run::program(program, ctx)
}

/// Compiles (cached) and runs `src`.
pub fn eval_str(src: &str, ctx: &dyn Context) -> Result<KeyValue, String> {
    eval(&*compile(src)?, ctx)
}

// ---------------------------------------------------------------------------------------------
// The cache of compiled formulas

const CACHE_SIZE: usize = 1024;

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

#[derive(Default)]
struct Cache {
    map: HashMap<String, (Result<Arc<Program>, String>, u64)>,
    tick: u64,
}

impl Cache {
    /// Forgets the half used longest ago.
    fn shrink(&mut self) {
        let mut ticks: Vec<u64> = self.map.values().map(|(_, t)| *t).collect();
        ticks.sort_unstable();
        let cut = ticks.get(ticks.len() / 2).copied().unwrap_or(0);
        self.map.retain(|_, (_, t)| *t > cut);
    }
}

// ---------------------------------------------------------------------------------------------
// Where things are in a formula

/// "column 7", or "line 2, column 3" in a formula of several lines (`at` is a byte offset).
pub(crate) fn place(src: &str, at: usize) -> String {
    let at = at.min(src.len());
    let before = &src[..floor_char(src, at)];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    if src.contains('\n') { format!("line {line}, column {col}") } else { format!("column {col}") }
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A stable 64-bit hash (FNV-1a) of some text, for seeds: the same on every run and machine.
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Mixes two 64-bit numbers into a well-spread one (splitmix64's finaliser).
pub(crate) fn mix(a: u64, b: u64) -> u64 {
    let mut z = a ^ b.wrapping_add(0x9e37_79b9_7f4a_7c15).wrapping_add(a << 6).wrapping_add(a >> 2);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A simple [`Context`] with fixed values: for tests, previews and checking formulas outside a
/// scene. `prop()` reads from `props` by `"id.name"`.
#[derive(Debug, Clone, Default)]
pub struct Fixed {
    pub time: f64,
    pub value: Option<KeyValue>,
    pub keys: Vec<Keyframe>,
    pub props: HashMap<String, KeyValue>,
    pub index: f64,
    pub fps: f64,
    pub duration: f64,
    pub seed: u64,
}

impl Fixed {
    pub fn at(time: f64) -> Fixed {
        Fixed { time, index: 1.0, fps: 30.0, duration: 10.0, ..Fixed::default() }
    }
}

impl Context for Fixed {
    fn time(&self) -> f64 {
        self.time
    }
    fn value(&self) -> Option<KeyValue> {
        if self.keys.is_empty() { self.value.clone() } else { self.value_at(self.time) }
    }
    fn value_at(&self, t: f64) -> Option<KeyValue> {
        crate::anim::value_at(&self.keys, t).or_else(|| self.value.clone())
    }
    fn keys(&self) -> &[Keyframe] {
        &self.keys
    }
    fn prop(&self, id: &str, name: &str, _t: Option<f64>) -> Result<KeyValue, String> {
        self.props.get(&format!("{id}.{name}")).cloned().ok_or_else(|| format!("no property `{name}` on \"{id}\""))
    }
    fn index(&self) -> f64 {
        self.index
    }
    fn fps(&self) -> f64 {
        self.fps
    }
    fn duration(&self) -> f64 {
        self.duration
    }
    fn seed(&self) -> u64 {
        self.seed
    }
}
