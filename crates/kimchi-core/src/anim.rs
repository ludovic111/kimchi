//! Keyframes: values that change over time, shared by clips, motion layers and 3D objects.
//!
//! A property's keyframes are kept sorted by time. Before the first keyframe the value is the
//! first one, after the last it is the last one, and between two keyframes it travels along the
//! *second* keyframe's easing ("arrive at 100 with easeOut"). One keyframe is a constant.
//!
//! Values are numbers, vectors (`[x, y, z]`) or strings. Two colours blend; two strings with the
//! same shape once their numbers are taken out (SVG paths with the same commands) morph number by
//! number; any other pair of strings holds the first until the second keyframe.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Keyframes by property name (`"x"`, `"opacity"`, `"rotation.y"`…), each list sorted by time.
pub type Keyframes = BTreeMap<String, Vec<Keyframe>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe {
    /// Seconds from the start of whatever owns the keyframes (the clip, the scene).
    pub time: f64,
    pub value: KeyValue,
    /// How the value arrives here from the previous keyframe.
    pub easing: Easing,
}

impl Keyframe {
    pub fn new(time: f64, value: impl Into<KeyValue>, easing: Easing) -> Self {
        Self { time, value: value.into(), easing }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyValue {
    Number(f64),
    Vector(Vec<f64>),
    Text(String),
}

impl From<f64> for KeyValue {
    fn from(v: f64) -> Self {
        KeyValue::Number(v)
    }
}

impl From<&str> for KeyValue {
    fn from(v: &str) -> Self {
        KeyValue::Text(v.to_string())
    }
}

impl From<String> for KeyValue {
    fn from(v: String) -> Self {
        KeyValue::Text(v)
    }
}

impl From<[f64; 2]> for KeyValue {
    fn from(v: [f64; 2]) -> Self {
        KeyValue::Vector(v.to_vec())
    }
}

impl From<[f64; 3]> for KeyValue {
    fn from(v: [f64; 3]) -> Self {
        KeyValue::Vector(v.to_vec())
    }
}

impl KeyValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            KeyValue::Number(n) => Some(*n),
            KeyValue::Vector(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        }
    }

    /// A vector of `n` numbers; a single number is repeated (`scale: 2` → `[2, 2, 2]`).
    pub fn as_vec(&self, n: usize) -> Option<Vec<f64>> {
        match self {
            KeyValue::Number(x) => Some(vec![*x; n]),
            KeyValue::Vector(v) if v.len() == n => Some(v.clone()),
            KeyValue::Vector(v) if v.len() == 1 => Some(vec![v[0]; n]),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            KeyValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The value `p` of the way from `self` to `to` (`p` may overshoot 0..1 with back/elastic easings).
    pub fn lerp(&self, to: &KeyValue, p: f64) -> KeyValue {
        match (self, to) {
            (KeyValue::Number(a), KeyValue::Number(b)) => KeyValue::Number(a + (b - a) * p),
            (KeyValue::Vector(a), KeyValue::Vector(b)) if a.len() == b.len() => {
                KeyValue::Vector(a.iter().zip(b).map(|(a, b)| a + (b - a) * p).collect())
            }
            (KeyValue::Number(a), KeyValue::Vector(b)) => KeyValue::Vector(b.iter().map(|b| a + (b - a) * p).collect()),
            (KeyValue::Vector(a), KeyValue::Number(b)) => KeyValue::Vector(a.iter().map(|a| a + (b - a) * p).collect()),
            (KeyValue::Text(a), KeyValue::Text(b)) => {
                if let (Some(ca), Some(cb)) = (Rgba::parse(a), Rgba::parse(b)) {
                    return KeyValue::Text(ca.lerp(cb, p).to_hex());
                }
                match morph(a, b, p) {
                    Some(s) => KeyValue::Text(s),
                    None => KeyValue::Text(if p >= 1.0 { b.clone() } else { a.clone() }),
                }
            }
            _ => if p >= 1.0 { to.clone() } else { self.clone() },
        }
    }
}

/// The value of a property at `t`, or `None` without keyframes.
pub fn value_at(keys: &[Keyframe], t: f64) -> Option<KeyValue> {
    let first = keys.first()?;
    if keys.len() == 1 || t <= first.time {
        return Some(first.value.clone());
    }
    let last = keys.last().expect("non-empty");
    if t >= last.time {
        return Some(last.value.clone());
    }
    // First keyframe strictly after t; the segment is [i - 1, i].
    let i = keys.partition_point(|k| k.time <= t);
    let (a, b) = (&keys[i - 1], &keys[i]);
    let span = b.time - a.time;
    if span <= 1e-9 {
        return Some(b.value.clone());
    }
    let p = b.easing.apply((t - a.time) / span);
    Some(a.value.lerp(&b.value, p))
}

/// A number property at `t`.
pub fn number_at(keys: &Keyframes, name: &str, t: f64) -> Option<f64> {
    keys.get(name).and_then(|k| value_at(k, t)).and_then(|v| v.as_f64())
}

/// Sorts every list by time, drops empty lists, and keeps one keyframe per instant (the last).
pub fn normalize(keys: &mut Keyframes) {
    keys.retain(|_, list| !list.is_empty());
    for list in keys.values_mut() {
        list.sort_by(|a, b| a.time.total_cmp(&b.time));
        list.dedup_by(|later, earlier| {
            if (later.time - earlier.time).abs() < 1e-6 {
                *earlier = later.clone();
                true
            } else {
                false
            }
        });
    }
}

/// Moves every keyframe by `dt` seconds (clips keep their animation in place on the timeline
/// when their start is trimmed or split).
pub fn shift(keys: &mut Keyframes, dt: f64) {
    for list in keys.values_mut() {
        for k in list {
            k.time += dt;
        }
    }
}

/// Sets (or replaces) the keyframe at `time`.
pub fn set_key(keys: &mut Keyframes, name: &str, key: Keyframe) {
    let list = keys.entry(name.to_string()).or_default();
    match list.iter_mut().find(|k| (k.time - key.time).abs() < 1e-6) {
        Some(k) => *k = key,
        None => list.push(key),
    }
    normalize(keys);
}

/// Removes the keyframe at `time` (within half a millisecond); true if there was one.
pub fn remove_key(keys: &mut Keyframes, name: &str, time: f64) -> bool {
    let Some(list) = keys.get_mut(name) else { return false };
    let before = list.len();
    list.retain(|k| (k.time - time).abs() >= 5e-4);
    let removed = list.len() != before;
    normalize(keys);
    removed
}

// ---------------------------------------------------------------------------------------------
// Easing

/// The shape of a move between two keyframes. Written as a string in JSON: `"linear"`, `"hold"`,
/// `"easeIn"`, `"easeOut"`, `"easeInOut"`, `"ease"`, `"easeOutBack"`, `"easeInOutExpo"`… (any of
/// sine, quad, cubic, quart, quint, expo, circ, back, elastic, bounce with In, Out or InOut),
/// `"cubicBezier(0.2, 0.8, 0.2, 1)"` or `"spring"` / `"spring(0.6)"` (0 = no bounce, 1 = very
/// bouncy).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Easing {
    #[default]
    Linear,
    /// Keeps the previous value until this keyframe, then jumps.
    Hold,
    Curve(Curve, Mode),
    Bezier(f64, f64, f64, f64),
    Spring(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    Sine,
    Quad,
    Cubic,
    Quart,
    Quint,
    Expo,
    Circ,
    Back,
    Elastic,
    Bounce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    In,
    Out,
    InOut,
}

const CURVES: [(Curve, &str); 10] = [
    (Curve::Sine, "Sine"),
    (Curve::Quad, "Quad"),
    (Curve::Cubic, "Cubic"),
    (Curve::Quart, "Quart"),
    (Curve::Quint, "Quint"),
    (Curve::Expo, "Expo"),
    (Curve::Circ, "Circ"),
    (Curve::Back, "Back"),
    (Curve::Elastic, "Elastic"),
    (Curve::Bounce, "Bounce"),
];

impl Easing {
    pub const EASE_IN: Easing = Easing::Curve(Curve::Cubic, Mode::In);
    pub const EASE_OUT: Easing = Easing::Curve(Curve::Cubic, Mode::Out);
    pub const EASE_IN_OUT: Easing = Easing::Curve(Curve::Cubic, Mode::InOut);

    /// Every named easing, for pickers and error messages.
    pub fn names() -> Vec<String> {
        let mut out = vec!["linear".to_string(), "hold".into(), "ease".into(), "easeIn".into(), "easeOut".into(), "easeInOut".into()];
        for (_, c) in CURVES {
            for m in ["In", "Out", "InOut"] {
                out.push(format!("ease{m}{c}"));
            }
        }
        out.push("spring".into());
        out
    }

    /// Progress (0 at the previous keyframe, 1 at this one) after a fraction `x` of the time.
    pub fn apply(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match *self {
            Easing::Linear => x,
            Easing::Hold => {
                if x >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Easing::Curve(c, Mode::In) => ease_in(c, x),
            Easing::Curve(c, Mode::Out) => 1.0 - ease_in(c, 1.0 - x),
            Easing::Curve(c, Mode::InOut) => {
                if x < 0.5 {
                    ease_in(c, x * 2.0) / 2.0
                } else {
                    1.0 - ease_in(c, (1.0 - x) * 2.0) / 2.0
                }
            }
            Easing::Bezier(x1, y1, x2, y2) => bezier(x1, y1, x2, y2, x),
            Easing::Spring(bounce) => spring(bounce, x),
        }
    }

    pub fn parse(s: &str) -> Result<Easing, String> {
        let raw = s.trim();
        let key: String = raw.chars().filter(|c| !matches!(c, '-' | '_' | ' ')).collect::<String>().to_ascii_lowercase();
        let args = |key: &str, name: &str| -> Option<Vec<f64>> {
            let inner = key.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')?;
            inner.split(',').map(|v| v.trim().parse::<f64>().ok()).collect()
        };
        match key.as_str() {
            "" | "linear" | "none" => return Ok(Easing::Linear),
            "hold" | "step" | "steps" | "constant" => return Ok(Easing::Hold),
            "ease" => return Ok(Easing::Bezier(0.25, 0.1, 0.25, 1.0)),
            "easein" | "in" => return Ok(Easing::EASE_IN),
            "easeout" | "out" => return Ok(Easing::EASE_OUT),
            "easeinout" | "inout" | "smooth" => return Ok(Easing::EASE_IN_OUT),
            "spring" => return Ok(Easing::Spring(0.4)),
            _ => {}
        }
        if let Some(v) = args(&key, "cubicbezier").or_else(|| args(&key, "bezier"))
            && let [x1, y1, x2, y2] = v[..]
        {
            return Ok(Easing::Bezier(x1.clamp(0.0, 1.0), y1, x2.clamp(0.0, 1.0), y2));
        }
        if let Some(v) = args(&key, "spring")
            && let [b] = v[..]
        {
            return Ok(Easing::Spring(b.clamp(0.0, 1.0)));
        }
        // easeOutBack, outBack, easeOut-back, back-out, easeBackOut…
        let body = key.strip_prefix("ease").unwrap_or(&key);
        for (curve, name) in CURVES {
            let name = name.to_ascii_lowercase();
            for (mode, m) in [(Mode::InOut, "inout"), (Mode::In, "in"), (Mode::Out, "out")] {
                if body == format!("{m}{name}") || body == format!("{name}{m}") {
                    return Ok(Easing::Curve(curve, mode));
                }
            }
        }
        Err(format!(
            "Unknown easing \"{raw}\". Use linear, hold, ease, easeIn, easeOut, easeInOut, ease<In|Out|InOut><Sine|Quad|Cubic|Quart|Quint|Expo|Circ|Back|Elastic|Bounce>, cubicBezier(x1,y1,x2,y2) or spring(bounce 0-1)."
        ))
    }
}

impl fmt::Display for Easing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Easing::Linear => write!(f, "linear"),
            Easing::Hold => write!(f, "hold"),
            Easing::EASE_IN => write!(f, "easeIn"),
            Easing::EASE_OUT => write!(f, "easeOut"),
            Easing::EASE_IN_OUT => write!(f, "easeInOut"),
            Easing::Curve(c, m) => {
                let c = CURVES.iter().find(|(k, _)| *k == c).map_or("Cubic", |(_, n)| n);
                let m = match m {
                    Mode::In => "In",
                    Mode::Out => "Out",
                    Mode::InOut => "InOut",
                };
                write!(f, "ease{m}{c}")
            }
            Easing::Bezier(a, b, c, d) if (a, b, c, d) == (0.25, 0.1, 0.25, 1.0) => write!(f, "ease"),
            Easing::Bezier(a, b, c, d) => write!(f, "cubicBezier({a}, {b}, {c}, {d})"),
            Easing::Spring(b) => write!(f, "spring({b})"),
        }
    }
}

impl Serialize for Easing {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Easing {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Easing::parse(&s).map_err(serde::de::Error::custom)
    }
}

fn ease_in(c: Curve, x: f64) -> f64 {
    use std::f64::consts::PI;
    match c {
        Curve::Sine => 1.0 - (x * PI / 2.0).cos(),
        Curve::Quad => x * x,
        Curve::Cubic => x * x * x,
        Curve::Quart => x.powi(4),
        Curve::Quint => x.powi(5),
        Curve::Expo => {
            if x <= 0.0 {
                0.0
            } else {
                (2f64).powf(10.0 * x - 10.0)
            }
        }
        Curve::Circ => 1.0 - (1.0 - x * x).max(0.0).sqrt(),
        Curve::Back => {
            let c1 = 1.70158;
            (c1 + 1.0) * x * x * x - c1 * x * x
        }
        Curve::Elastic => {
            if x <= 0.0 || x >= 1.0 {
                x
            } else {
                -(2f64.powf(10.0 * x - 10.0)) * ((x * 10.0 - 10.75) * (2.0 * PI / 3.0)).sin()
            }
        }
        Curve::Bounce => 1.0 - bounce_out(1.0 - x),
    }
}

fn bounce_out(x: f64) -> f64 {
    let (n1, d1) = (7.5625, 2.75);
    if x < 1.0 / d1 {
        n1 * x * x
    } else if x < 2.0 / d1 {
        let x = x - 1.5 / d1;
        n1 * x * x + 0.75
    } else if x < 2.5 / d1 {
        let x = x - 2.25 / d1;
        n1 * x * x + 0.9375
    } else {
        let x = x - 2.625 / d1;
        n1 * x * x + 0.984375
    }
}

/// CSS `cubic-bezier`: solve x(s) = x for s by Newton then bisection, return y(s).
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    let coord = |a: f64, b: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * s * a + 3.0 * m * s * s * b + s * s * s
    };
    let slope = |a: f64, b: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * a + 6.0 * m * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = x;
    for _ in 0..8 {
        let err = coord(x1, x2, s) - x;
        if err.abs() < 1e-7 {
            return coord(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    s = x;
    for _ in 0..40 {
        let v = coord(x1, x2, s);
        if (v - x).abs() < 1e-7 {
            break;
        }
        if v < x {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    coord(y1, y2, s)
}

/// A damped spring that settles by the end of the segment; `bounce` 0 doesn't overshoot.
fn spring(bounce: f64, x: f64) -> f64 {
    if x >= 1.0 {
        return 1.0;
    }
    let b = bounce.clamp(0.0, 1.0);
    // Envelope down to ~0.1% at the end; more bounce = more oscillations.
    let decay = 6.9;
    let freq = std::f64::consts::PI * (0.5 + 4.0 * b);
    1.0 - (-decay * x).exp() * (freq * x).cos()
}

// ---------------------------------------------------------------------------------------------
// Colours and morphing

/// A colour as four 0–255 channels, for blending keyframes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub [f64; 4]);

impl Rgba {
    /// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
    pub fn parse(s: &str) -> Option<Rgba> {
        let hex = s.trim().strip_prefix('#')?;
        let d: Vec<u8> = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
        let ch: [u8; 4] = match d.len() {
            3 | 4 => std::array::from_fn(|i| d.get(i).map_or(255, |v| v * 17)),
            6 | 8 => std::array::from_fn(|i| d.get(i * 2..i * 2 + 2).map_or(255, |v| v[0] * 16 + v[1])),
            _ => return None,
        };
        Some(Rgba(ch.map(f64::from)))
    }

    pub fn lerp(self, to: Rgba, p: f64) -> Rgba {
        Rgba(std::array::from_fn(|i| (self.0[i] + (to.0[i] - self.0[i]) * p).clamp(0.0, 255.0)))
    }

    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.0.map(|v| v.round().clamp(0.0, 255.0) as u8);
        if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
    }
}

/// Splits a string into its text skeleton and the numbers in it.
fn numbers(s: &str) -> (String, Vec<f64>) {
    let mut skeleton = String::new();
    let mut nums = vec![];
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        let starts_number = c.is_ascii_digit()
            || ((c == '-' || c == '+' || c == '.') && b.get(i + 1).is_some_and(|n| n.is_ascii_digit() || (*n == b'.' && c != '.')));
        if starts_number {
            let mut j = i + 1;
            let mut dot = c == '.';
            let mut exp = false;
            while j < b.len() {
                let d = b[j] as char;
                if d.is_ascii_digit() {
                    j += 1;
                } else if d == '.' && !dot && !exp {
                    dot = true;
                    j += 1;
                } else if (d == 'e' || d == 'E') && !exp && b.get(j + 1).is_some_and(|n| n.is_ascii_digit() || *n == b'-' || *n == b'+') {
                    exp = true;
                    j += 2;
                } else {
                    break;
                }
            }
            if let Ok(v) = s[i..j].parse::<f64>() {
                nums.push(v);
                skeleton.push('\u{1}');
                i = j;
                continue;
            }
        }
        skeleton.push(c);
        i += 1;
    }
    (skeleton, nums)
}

/// Number-by-number blend of two strings of the same shape (SVG paths with the same commands).
fn morph(a: &str, b: &str, p: f64) -> Option<String> {
    let (sa, na) = numbers(a);
    let (sb, nb) = numbers(b);
    if sa != sb || na.len() != nb.len() || na.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(a.len() + 8);
    let mut it = na.iter().zip(&nb);
    for c in sa.chars() {
        if c == '\u{1}' {
            let (x, y) = it.next()?;
            let v = x + (y - x) * p;
            let v = if v.abs() < 5e-5 { 0.0 } else { v };
            out.push_str(&format!("{}", (v * 10_000.0).round() / 10_000.0));
        } else {
            out.push(c);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// JSON shape: `{"time": 1, "value": 0, "easing": "easeOut"}`, with `t`/`v`/`ease` accepted, or
// the short `[time, value]` / `[time, value, "easing"]`.

#[derive(Serialize)]
struct KeyOut<'a> {
    time: f64,
    value: &'a KeyValue,
    #[serde(skip_serializing_if = "is_linear")]
    easing: &'a Easing,
}

fn is_linear(e: &&Easing) -> bool {
    **e == Easing::Linear
}

impl Serialize for Keyframe {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        KeyOut { time: self.time, value: &self.value, easing: &self.easing }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Keyframe {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(d)?;
        Keyframe::from_json(&v).map_err(D::Error::custom)
    }
}

impl Keyframe {
    /// `{"time", "value", "easing"}` (or `t`/`v`/`ease`), `[time, value]` or `[time, value, easing]`,
    /// with errors that say what is wrong.
    pub fn from_json(v: &serde_json::Value) -> Result<Keyframe, String> {
        use serde_json::Value;
        let (time, value, easing) = match v {
            Value::Object(o) => {
                let pick = |names: &[&str]| names.iter().find_map(|n| o.get(*n));
                for k in o.keys() {
                    if !["time", "t", "at", "value", "v", "easing", "ease"].contains(&k.as_str()) {
                        return Err(format!("a keyframe has time, value and easing, not `{k}`"));
                    }
                }
                (pick(&["time", "t", "at"]), pick(&["value", "v"]), pick(&["easing", "ease"]))
            }
            Value::Array(a) if (2..=3).contains(&a.len()) => (a.first(), a.get(1), a.get(2)),
            _ => return Err(format!("a keyframe is [time, value], [time, value, \"easing\"] or {{\"time\", \"value\", \"easing\"}}, not {v}")),
        };
        let time = time.and_then(Value::as_f64).ok_or_else(|| format!("keyframe {v} needs a time in seconds"))?;
        let value = value.ok_or_else(|| format!("keyframe {v} needs a value"))?;
        let value: KeyValue = serde_json::from_value(value.clone()).map_err(|_| format!("keyframe value {value} should be a number, [numbers] or a string"))?;
        let easing = match easing {
            None | Some(Value::Null) => Easing::Linear,
            Some(Value::String(s)) => Easing::parse(s)?,
            Some(other) => return Err(format!("easing {other} should be a name like \"easeOut\"")),
        };
        Ok(Keyframe { time, value, easing })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(json: &str) -> Vec<Keyframe> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn interpolates_and_holds_the_ends() {
        let k = keys(r#"[{"time": 1, "value": 0}, {"t": 3, "v": 100, "ease": "linear"}]"#);
        assert_eq!(value_at(&k, 0.0), Some(KeyValue::Number(0.0)));
        assert_eq!(value_at(&k, 2.0), Some(KeyValue::Number(50.0)));
        assert_eq!(value_at(&k, 9.0), Some(KeyValue::Number(100.0)));
        assert_eq!(value_at(&[], 1.0), None);
    }

    #[test]
    fn easing_belongs_to_the_arriving_keyframe() {
        let k = keys(r#"[[0, 0], [1, 1, "hold"], [2, 0, "easeOut"]]"#);
        assert_eq!(value_at(&k, 0.99).unwrap().as_f64(), Some(0.0));
        assert_eq!(value_at(&k, 1.0).unwrap().as_f64(), Some(1.0));
        // easeOut is past half way at half time.
        assert!(value_at(&k, 1.5).unwrap().as_f64().unwrap() < 0.5);
    }

    #[test]
    fn parses_easing_names() {
        for (s, e) in [
            ("easeOutBack", Easing::Curve(Curve::Back, Mode::Out)),
            ("ease-in-out-expo", Easing::Curve(Curve::Expo, Mode::InOut)),
            ("outBounce", Easing::Curve(Curve::Bounce, Mode::Out)),
            ("easeOut", Easing::EASE_OUT),
            ("cubic-bezier(0.2, 0.8, 0.2, 1)", Easing::Bezier(0.2, 0.8, 0.2, 1.0)),
            ("spring(0.7)", Easing::Spring(0.7)),
        ] {
            assert_eq!(Easing::parse(s).unwrap(), e, "{s}");
            assert_eq!(Easing::parse(&e.to_string()).unwrap(), e, "round trip {e}");
        }
        assert!(Easing::parse("wobbly").is_err());
        for name in Easing::names() {
            let e = Easing::parse(&name).unwrap();
            assert!((e.apply(0.0)).abs() < 1e-9 || e == Easing::Hold, "{name} starts at 0");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-6, "{name} ends at 1");
        }
    }

    #[test]
    fn back_overshoots_and_bezier_matches_css() {
        assert!(Easing::parse("easeOutBack").unwrap().apply(0.7) > 1.0);
        let ease = Easing::parse("ease").unwrap();
        // CSS `ease` at 50% time is about 80% of the way.
        assert!((ease.apply(0.5) - 0.8024).abs() < 0.002, "{}", ease.apply(0.5));
        let s = Easing::Spring(0.0);
        assert!((0..=20).all(|i| s.apply(i as f64 / 20.0) <= 1.0 + 1e-9));
        assert!((0..=100).any(|i| Easing::Spring(0.8).apply(i as f64 / 100.0) > 1.05));
    }

    #[test]
    fn blends_colours_vectors_and_paths() {
        let c = KeyValue::from("#000000").lerp(&KeyValue::from("#ffffff"), 0.5);
        assert_eq!(c, KeyValue::from("#808080"));
        let v = KeyValue::from([0.0, 10.0, 0.0]).lerp(&KeyValue::from([10.0, 10.0, 20.0]), 0.5);
        assert_eq!(v, KeyValue::Vector(vec![5.0, 10.0, 10.0]));
        let p = KeyValue::from("M0 0 L10 -10").lerp(&KeyValue::from("M10 10 L20 10"), 0.5);
        assert_eq!(p, KeyValue::from("M5 5 L15 0"));
        let words = KeyValue::from("Hello").lerp(&KeyValue::from("World"), 0.5);
        assert_eq!(words, KeyValue::from("Hello"));
    }

    #[test]
    fn set_remove_and_shift() {
        let mut k = Keyframes::new();
        set_key(&mut k, "x", Keyframe::new(2.0, 10.0, Easing::Linear));
        set_key(&mut k, "x", Keyframe::new(0.0, 0.0, Easing::Linear));
        set_key(&mut k, "x", Keyframe::new(2.0, 20.0, Easing::EASE_OUT));
        assert_eq!(k["x"].len(), 2);
        assert_eq!(k["x"][1].value.as_f64(), Some(20.0));
        shift(&mut k, -1.0);
        assert_eq!(k["x"][0].time, -1.0);
        assert!(remove_key(&mut k, "x", -1.0));
        assert!(remove_key(&mut k, "x", 1.0));
        assert!(k.is_empty());
        let json = serde_json::to_value(Keyframe::new(1.0, 2.0, Easing::EASE_OUT)).unwrap();
        assert_eq!(json, serde_json::json!({"time": 1.0, "value": 2.0, "easing": "easeOut"}));
    }
}
