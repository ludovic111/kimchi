//! Particles: many small things born, moving and dying over time (sparks, snow, confetti, dust,
//! bubbles), for 2D layers and 3D objects alike.
//!
//! The simulation has no state: where particle *i* is at time *t* is worked out from its birth
//! (`emitFrom + i / rate`), its random draw (from `seed` and *i*) and the forces, so any frame can
//! be drawn on its own (scrubbing, exports in any order) and always looks the same. Units are
//! pixels in 2D (y down) and world units in 3D (y up); defaults follow.

use serde::{Deserialize, Serialize};

use super::{check_color, is_one, is_zero};
use crate::anim::{KeyValue, Rgba};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParticleSystem {
    /// Particles born per second.
    #[serde(default = "rate")]
    pub rate: f64,
    /// Particles born all at once at `emitFrom`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub burst: f64,
    /// Never more than this many alive.
    #[serde(default = "max")]
    pub max: f64,
    /// Seconds each lives.
    #[serde(default = "lifetime")]
    pub lifetime: f64,
    /// 0–1: how much lifetimes vary.
    #[serde(default = "random")]
    pub lifetime_random: f64,
    /// Where they are born: `"point"`, `"line"` (along x), `"rect"` (2D) / `"box"` (3D), `"circle"`
    /// (2D) / `"disc"` (3D, flat), `"ring"` (on a circle's edge) or `"sphere"`.
    #[serde(default = "point")]
    pub emitter: String,
    /// The emitter's size: [width, height, depth] (line/rect/box) or [radius] (circle/ring/sphere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitter_size: Option<Vec<f64>>,
    /// The direction they fly (default up).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<[f64; 3]>,
    /// Degrees they scatter around the direction (180 = every direction in 2D; 360 in 3D too).
    #[serde(default = "spread")]
    pub spread: f64,
    /// Starting speed per second (default 200 px or 2 units).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    #[serde(default = "random")]
    pub speed_random: f64,
    /// Pull per second² ([x, y, z]; default none). 2D: [0, 400] falls; 3D: [0, -4, 0].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gravity: Option<[f64; 3]>,
    /// Air resistance: how quickly they slow down (0 = never, 2 = fast).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub drag: f64,
    /// Swirling wander (pixels or units).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub turbulence: f64,
    /// Size (diameter; default 12 px or 0.08 units).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Size at the end of life, relative to the start.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub size_end: f64,
    #[serde(default = "random")]
    pub size_random: f64,
    /// Degrees per second each turns.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spin: f64,
    /// 0–1: random starting angle and spin.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spin_random: f64,
    #[serde(default = "white")]
    pub color: String,
    /// Colour at the end of life (fades from `color`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_end: Option<String>,
    /// Each particle picks one of these (confetti); overrides `color`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<String>,
    /// Share of the life spent fading in, and out.
    #[serde(default = "fade_in")]
    pub fade_in: f64,
    #[serde(default = "fade_out")]
    pub fade_out: f64,
    /// What each is: 2D `"circle"`, `"square"`, `"triangle"`, `"star"`, `"spark"` (a streak along its
    /// motion), `"image"` (`asset`); 3D `"sphere"`, `"cube"`, `"tetra"`, `"spark"`, `"image"` (a
    /// card facing the camera). Default circle / sphere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// For `"image"` particles: a media item by id, name or path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Spark length in seconds of motion.
    #[serde(default = "stretch")]
    pub stretch: f64,
    /// Scene seconds when emission starts and stops.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub emit_from: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emit_until: Option<f64>,
    /// Born particles stay where they were when the emitter moves (a trail); off = they move
    /// with it.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub trail: bool,
    /// Start as if it had been running this many seconds already.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub prewarm: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seed: f64,
}

pub(super) const PARTICLE_KEYS: &[&str] = &[
    "rate", "burst", "max", "lifetime", "lifetimeRandom", "emitter", "emitterSize", "direction", "spread", "speed", "speedRandom",
    "gravity", "drag", "turbulence", "size", "sizeEnd", "sizeRandom", "spin", "spinRandom", "color", "colorEnd", "colors", "fadeIn",
    "fadeOut", "shape", "asset", "stretch", "emitFrom", "emitUntil", "trail", "prewarm", "seed",
];

/// Properties keyframes can animate. Changing birth-related ones (rate, lifetime) over time
/// would rewrite the past, so they can't be animated: use emitFrom/emitUntil and burst.
pub(super) const PARTICLE_PROPS: &[&str] = &[
    "spread", "speed", "gravity", "drag", "turbulence", "size", "sizeEnd", "spin", "color", "colorEnd", "direction", "stretch",
];

pub const EMITTERS: &[&str] = &["point", "line", "rect", "box", "circle", "disc", "ring", "sphere"];
pub const SHAPES_2D: &[&str] = &["circle", "square", "triangle", "star", "spark", "image"];
pub const SHAPES_3D: &[&str] = &["sphere", "cube", "tetra", "spark", "image"];

/// One particle at an instant.
#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    /// Its number (stable over its life).
    pub index: u64,
    /// Position relative to the emitter's current origin (pixels in 2D, world units in 3D).
    pub pos: [f64; 3],
    /// Velocity per second.
    pub vel: [f64; 3],
    /// 0 at birth, 1 at death.
    pub life: f64,
    /// Diameter.
    pub size: f64,
    /// Degrees.
    pub rotation: f64,
    /// Straight RGBA, 0–255 like [`Rgba`], with the fades in alpha.
    pub color: Rgba,
}

impl Default for ParticleSystem {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

impl ParticleSystem {
    pub(super) fn check(&self) -> Result<(), String> {
        if !EMITTERS.contains(&self.emitter.as_str()) {
            return Err(format!("emitter is one of {}", EMITTERS.join(", ")));
        }
        if let Some(s) = &self.shape
            && !SHAPES_2D.contains(&s.as_str())
            && !SHAPES_3D.contains(&s.as_str())
        {
            return Err(format!("shape is one of {} (2D) or {} (3D)", SHAPES_2D.join(", "), SHAPES_3D.join(", ")));
        }
        if self.shape.as_deref() == Some("image") && self.asset.is_none() {
            return Err("image particles need an asset".into());
        }
        check_color(&self.color, "color")?;
        if let Some(c) = &self.color_end {
            check_color(c, "colorEnd")?;
        }
        for c in &self.colors {
            check_color(c, "colors")?;
        }
        Ok(())
    }

    pub(super) fn set(&mut self, name: &str, v: &KeyValue) -> Result<bool, String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        let c = || -> Result<String, String> {
            let s = v.as_str().ok_or_else(|| format!("{name} takes a colour"))?;
            check_color(s, name)?;
            Ok(s.to_string())
        };
        let v3 = || v.as_vec(3).map(|p| [p[0], p[1], p[2]]).ok_or_else(|| format!("{name} takes [x, y, z]"));
        match name {
            "spread" => self.spread = n()?,
            "speed" => self.speed = Some(n()?),
            "gravity" => self.gravity = Some(v3()?),
            "direction" => self.direction = Some(v3()?),
            "drag" => self.drag = n()?,
            "turbulence" => self.turbulence = n()?,
            "size" => self.size = Some(n()?),
            "sizeEnd" => self.size_end = n()?,
            "spin" => self.spin = n()?,
            "stretch" => self.stretch = n()?,
            "color" => self.color = c()?,
            "colorEnd" => self.color_end = Some(c()?),
            "rate" | "lifetime" | "burst" | "max" => return Err(format!("{name} can't change over time (it would rewrite particles already born); set it once, and use emitFrom / emitUntil")),
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(super) fn get(&self, name: &str) -> Option<KeyValue> {
        let n = |v: f64| Some(KeyValue::Number(v));
        match name {
            "spread" => n(self.spread),
            "speed" => self.speed.and_then(n),
            "gravity" => Some(KeyValue::Vector(self.gravity.unwrap_or([0.0; 3]).to_vec())),
            "direction" => self.direction.map(|d| KeyValue::Vector(d.to_vec())),
            "drag" => n(self.drag),
            "turbulence" => n(self.turbulence),
            "size" => self.size.and_then(n),
            "sizeEnd" => n(self.size_end),
            "spin" => n(self.spin),
            "stretch" => n(self.stretch),
            "color" => Some(KeyValue::from(self.color.as_str())),
            "colorEnd" => self.color_end.as_deref().map(KeyValue::from),
            _ => None,
        }
    }

    /// The particles alive at scene time `t`. `three` picks 3D units and directions;
    /// `origin(time)` is where the emitter was at a time (for trails), in the space particles
    /// are drawn in; positions come back relative to `origin(t)`.
    pub fn at(&self, t: f64, three: bool, origin: &dyn Fn(f64) -> [f64; 3]) -> Vec<Particle> {
        let rate = self.rate.max(0.0);
        let life = self.lifetime.max(0.01);
        let t = t + self.prewarm.max(0.0);
        let from = self.emit_from;
        let until = self.emit_until.map(|u| u + self.prewarm.max(0.0)).unwrap_or(f64::INFINITY);
        let now = t - self.prewarm.max(0.0);
        let speed = self.speed.unwrap_or(if three { 2.0 } else { 200.0 });
        let size = self.size.unwrap_or(if three { 0.08 } else { 12.0 });
        let dir = normalize(self.direction.unwrap_or(if three { [0.0, 1.0, 0.0] } else { [0.0, -1.0, 0.0] }));
        let gravity = self.gravity.unwrap_or([0.0; 3]);
        let base = Rgba::parse(&self.color).unwrap_or(Rgba([255.0; 4]));
        let end = self.color_end.as_deref().and_then(Rgba::parse);
        let palette: Vec<Rgba> = self.colors.iter().filter_map(|c| Rgba::parse(c)).collect();
        let here = origin(now);
        let max = self.max.clamp(0.0, 100_000.0) as usize;
        let mut out = vec![];

        // Births: the burst (indices 0..burst) at `from`, then one every 1/rate seconds.
        let burst = self.burst.max(0.0).floor() as u64;
        let mut births: Vec<(u64, f64)> = vec![];
        if t >= from {
            births.extend((0..burst).map(|i| (i, from)));
        }
        if rate > 0.0 {
            let first = ((t - life - from) * rate).floor().max(0.0) as u64;
            let last = ((t.min(until) - from) * rate).floor();
            if last >= 0.0 {
                let last = last as u64;
                // Never walk more than the cap allows (newest first, below).
                let first = first.max(last.saturating_sub(max as u64 * 2));
                births.extend((first..=last).map(|k| (burst + k, from + k as f64 / rate)));
            }
        }
        for (index, born) in births.into_iter().rev() {
            if out.len() >= max {
                break;
            }
            let mut r = Rand::new(self.seed as u64, index);
            let my_life = life * (1.0 - self.lifetime_random.clamp(0.0, 1.0) * r.f());
            let age = t - born;
            if age < 0.0 || age >= my_life {
                continue;
            }
            let k = age / my_life;
            // Where on the emitter, which way, how fast.
            let p0 = self.emit_point(&mut r, three);
            let v0 = scale(scatter(dir, self.spread.to_radians(), three, &mut r), speed * (1.0 - self.speed_random.clamp(0.0, 1.0) * r.f()));
            let (mut pos, vel) = travel(p0, v0, gravity, self.drag.max(0.0), age);
            if self.turbulence != 0.0 {
                let w = wander(&mut r, age, three);
                let grow = (age / my_life.min(1.0)).min(1.0);
                for i in 0..3 {
                    pos[i] += w[i] * self.turbulence * grow;
                }
            }
            if self.trail {
                let at_birth = origin(born - self.prewarm.max(0.0));
                for i in 0..3 {
                    pos[i] += at_birth[i] - here[i];
                }
            }
            let grow = 1.0 + (self.size_end - 1.0) * k;
            let sz = (size * (1.0 - self.size_random.clamp(0.0, 1.0) * r.f()) * grow).max(0.0);
            let sr = self.spin_random.clamp(0.0, 1.0);
            let rotation = (r.f() * 2.0 - 1.0) * 180.0 * sr + self.spin * (1.0 - sr * r.f()) * age;
            let mut color = if palette.is_empty() { base } else { palette[(r.u() % palette.len() as u64) as usize] };
            if let Some(e) = end {
                color = color.lerp(e, k);
            }
            let fade_in = if self.fade_in > 0.0 { (k / self.fade_in).min(1.0) } else { 1.0 };
            let fade_out = if self.fade_out > 0.0 { ((1.0 - k) / self.fade_out).min(1.0) } else { 1.0 };
            color.0[3] *= fade_in.max(0.0) * fade_out.max(0.0);
            out.push(Particle { index, pos, vel, life: k, size: sz, rotation, color });
        }
        // Oldest first, so newer ones draw on top.
        out.reverse();
        out
    }

    fn emit_point(&self, r: &mut Rand, three: bool) -> [f64; 3] {
        let s = self.emitter_size.clone().unwrap_or_default();
        let get = |i: usize, d: f64| s.get(i).or(s.first()).copied().unwrap_or(d);
        let unit = if three { 1.0 } else { 100.0 };
        match self.emitter.as_str() {
            "line" => [(r.f() - 0.5) * get(0, unit), 0.0, 0.0],
            "rect" | "box" => {
                let z = if three { (r.f() - 0.5) * get(2, unit) } else { 0.0 };
                [(r.f() - 0.5) * get(0, unit), (r.f() - 0.5) * get(1, unit), z]
            }
            "circle" | "disc" | "ring" => {
                let a = r.f() * std::f64::consts::TAU;
                let rad = get(0, unit / 2.0) * if self.emitter == "ring" { 1.0 } else { r.f().sqrt() };
                if three { [a.cos() * rad, 0.0, a.sin() * rad] } else { [a.cos() * rad, a.sin() * rad, 0.0] }
            }
            "sphere" => {
                let d = random_unit(r, three);
                let rad = get(0, unit / 2.0) * r.f().cbrt();
                scale(d, rad)
            }
            _ => [0.0; 3],
        }
    }
}

/// Position and velocity after `age` seconds from `p0` at `v0`, with gravity and drag.
fn travel(p0: [f64; 3], v0: [f64; 3], g: [f64; 3], drag: f64, age: f64) -> ([f64; 3], [f64; 3]) {
    let mut p = [0.0; 3];
    let mut v = [0.0; 3];
    for i in 0..3 {
        if drag > 1e-6 {
            let e = (-drag * age).exp();
            p[i] = p0[i] + v0[i] * (1.0 - e) / drag + g[i] * (age / drag - (1.0 - e) / (drag * drag));
            v[i] = v0[i] * e + g[i] * (1.0 - e) / drag;
        } else {
            p[i] = p0[i] + v0[i] * age + 0.5 * g[i] * age * age;
            v[i] = v0[i] + g[i] * age;
        }
    }
    (p, v)
}

/// A smooth wandering offset (about −1..1 per axis) that changes with age.
fn wander(r: &mut Rand, age: f64, three: bool) -> [f64; 3] {
    let mut w = [0.0; 3];
    let axes = if three { 3 } else { 2 };
    for item in w.iter_mut().take(axes) {
        let (f1, f2) = (0.6 + r.f() * 1.4, 1.7 + r.f() * 2.3);
        let (a, b) = (r.f() * std::f64::consts::TAU, r.f() * std::f64::consts::TAU);
        *item = 0.7 * (age * f1 + a).sin() + 0.3 * (age * f2 + b).sin();
    }
    w
}

/// A direction within `spread` (full angle, radians) of `dir`.
fn scatter(dir: [f64; 3], spread: f64, three: bool, r: &mut Rand) -> [f64; 3] {
    let half = (spread / 2.0).clamp(0.0, std::f64::consts::PI);
    if !three {
        let a = (r.f() * 2.0 - 1.0) * half;
        let (s, c) = a.sin_cos();
        return [dir[0] * c - dir[1] * s, dir[0] * s + dir[1] * c, 0.0];
    }
    // Uniform over the spherical cap around `dir`.
    let cos_t = 1.0 - r.f() * (1.0 - half.cos());
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    let phi = r.f() * std::f64::consts::TAU;
    let helper = if dir[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let u = normalize(cross(helper, dir));
    let v = cross(dir, u);
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] = dir[i] * cos_t + (u[i] * phi.cos() + v[i] * phi.sin()) * sin_t;
    }
    out
}

fn random_unit(r: &mut Rand, three: bool) -> [f64; 3] {
    if three {
        let z = r.f() * 2.0 - 1.0;
        let a = r.f() * std::f64::consts::TAU;
        let s = (1.0 - z * z).sqrt();
        [s * a.cos(), z, s * a.sin()]
    } else {
        let a = r.f() * std::f64::consts::TAU;
        [a.cos(), a.sin(), 0.0]
    }
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-12 { [0.0, -1.0, 0.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn scale(v: [f64; 3], k: f64) -> [f64; 3] {
    [v[0] * k, v[1] * k, v[2] * k]
}

/// Random numbers for particle `i` of a system with `seed` (splitmix64): the same every time.
pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64, i: u64) -> Rand {
        Rand(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ i.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ 0x2545_F491_4F6C_DD1D)
    }

    pub fn u(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0..1.
    pub fn f(&mut self) -> f64 {
        (self.u() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn rate() -> f64 {
    30.0
}
fn max() -> f64 {
    2000.0
}
fn lifetime() -> f64 {
    2.0
}
fn random() -> f64 {
    0.3
}
fn point() -> String {
    "point".into()
}
fn spread() -> f64 {
    25.0
}
fn one() -> f64 {
    1.0
}
fn white() -> String {
    "#ffffff".into()
}
fn fade_in() -> f64 {
    0.1
}
fn fade_out() -> f64 {
    0.3
}
fn stretch() -> f64 {
    0.05
}
fn yes() -> bool {
    true
}
fn is_true(v: &bool) -> bool {
    *v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_frame_same_particles_and_they_die() {
        let p: ParticleSystem = serde_json::from_value(serde_json::json!({"rate": 50, "lifetime": 1, "gravity": [0, 300, 0]})).unwrap();
        let still = |_: f64| [0.0; 3];
        let a = p.at(2.0, false, &still);
        let b = p.at(2.0, false, &still);
        assert_eq!(a, b);
        assert!(a.len() > 30 && a.len() <= 51, "{} alive", a.len());
        assert!(a.iter().all(|q| q.life >= 0.0 && q.life < 1.0));
        assert!(p.at(-1.0, false, &still).is_empty());
        let until: ParticleSystem = serde_json::from_value(serde_json::json!({"rate": 50, "lifetime": 1, "emitUntil": 1})).unwrap();
        assert!(until.at(2.5, false, &still).is_empty(), "all dead after emission stopped");
        let burst: ParticleSystem = serde_json::from_value(serde_json::json!({"rate": 0, "burst": 100, "lifetimeRandom": 0})).unwrap();
        assert_eq!(burst.at(0.5, true, &still).len(), 100);
    }

    #[test]
    fn trails_stay_behind_a_moving_emitter() {
        let p: ParticleSystem = serde_json::from_value(serde_json::json!({"rate": 20, "lifetime": 3, "speed": 0, "spread": 0, "fadeIn": 0})).unwrap();
        let moving = |t: f64| [t * 100.0, 0.0, 0.0];
        let ps = p.at(2.0, false, &moving);
        let oldest = ps.first().unwrap();
        assert!(oldest.pos[0] < -150.0, "the first particle is left behind: {:?}", oldest.pos);
    }
}
