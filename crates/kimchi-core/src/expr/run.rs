//! Running a formula: walks the tree once, no loops, so it always ends. Values are numbers,
//! vectors, strings (colours) and booleans; vectors do arithmetic element by element.

use std::sync::Arc;

use super::noise::{noise1, noise3};
use super::parse::{FUNCS, Func, Node, Op, Stmt, Var};
use super::{Context, Program, mix, place};
use crate::anim::{KeyValue, Rgba};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Val {
    Num(f64),
    Vec(Vec<f64>),
    Str(Arc<str>),
    Bool(bool),
}

impl Val {
    fn from_key(v: KeyValue) -> Val {
        match v {
            KeyValue::Number(n) => Val::Num(n),
            KeyValue::Vector(v) => Val::Vec(v),
            KeyValue::Text(s) => Val::Str(Arc::from(s.as_str())),
        }
    }

    fn to_key(&self) -> KeyValue {
        match self {
            Val::Num(n) => KeyValue::Number(*n),
            Val::Vec(v) => KeyValue::Vector(v.clone()),
            Val::Str(s) => KeyValue::Text(s.to_string()),
            Val::Bool(b) => KeyValue::Number(if *b { 1.0 } else { 0.0 }),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Val::Num(n) => *n != 0.0 && !n.is_nan(),
            Val::Vec(v) => !v.is_empty(),
            Val::Str(s) => !s.is_empty(),
            Val::Bool(b) => *b,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Val::Num(_) => "a number",
            Val::Vec(_) => "a vector",
            Val::Str(_) => "text",
            Val::Bool(_) => "true/false",
        }
    }

    fn scalar(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Val::Vec(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        }
    }

    /// The numbers in it (a number is one), or None for text.
    fn numbers(&self) -> Option<Vec<f64>> {
        match self {
            Val::Vec(v) => Some(v.clone()),
            Val::Str(_) => None,
            other => other.scalar().map(|n| vec![n]),
        }
    }

    fn shown(&self) -> String {
        match self {
            Val::Num(n) => fmt_num(*n),
            Val::Vec(v) => format!("[{}]", v.iter().map(|n| fmt_num(*n)).collect::<Vec<_>>().join(", ")),
            Val::Str(s) => s.to_string(),
            Val::Bool(b) => b.to_string(),
        }
    }
}

/// A number as people write it: `3`, `0.25`, `-1.5` (at most 6 decimals).
pub(super) fn fmt_num(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if n == n.trunc() && n.abs() < 1e15 {
        return format!("{n:.0}");
    }
    let s = format!("{n:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

struct Run<'a> {
    p: &'a Program,
    ctx: &'a dyn Context,
    slots: Vec<Option<Val>>,
    /// The formula's time (`posterizeTime` can step it).
    time: f64,
    seed_offset: u64,
    timeless: bool,
    draws: u64,
}

type R<T> = Result<T, String>;

pub(super) fn program(p: &Program, ctx: &dyn Context) -> R<KeyValue> {
    let mut r = Run { p, ctx, slots: vec![None; p.slots], time: ctx.time(), seed_offset: 0, timeless: false, draws: 0 };
    let mut last = None;
    for s in &p.body {
        match s {
            Stmt::Let(slot, n) => {
                let v = r.eval(n)?;
                r.slots[*slot] = Some(v);
            }
            Stmt::Expr(n) => last = Some(r.eval(n)?),
        }
    }
    let v = last.ok_or("the formula has no value")?;
    match &v {
        Val::Num(n) if !n.is_finite() => Err(format!("the formula gave {} (a division by zero, or the square root or log of a negative number?)", fmt_num(*n))),
        Val::Vec(v) if v.iter().any(|n| !n.is_finite()) => Err(format!("the formula gave {} (a division by zero?)", Val::Vec(v.clone()).shown())),
        _ => Ok(v.to_key()),
    }
}

impl Run<'_> {
    fn err(&self, at: usize, msg: impl std::fmt::Display) -> String {
        format!("{msg} ({})", place(&self.p.src, at))
    }

    fn num(&self, v: &Val, what: &str, at: usize) -> R<f64> {
        v.scalar().ok_or_else(|| self.err(at, format!("{what} takes a number, not {} ({})", v.kind(), v.shown())))
    }

    fn text<'v>(&self, v: &'v Val, what: &str, at: usize) -> R<&'v str> {
        match v {
            Val::Str(s) => Ok(s),
            other => Err(self.err(at, format!("{what} takes text in quotes, not {}", other.kind()))),
        }
    }

    fn eval(&mut self, n: &Node) -> R<Val> {
        Ok(match n {
            Node::Num(n) => Val::Num(*n),
            Node::Str(s) => Val::Str(s.clone()),
            Node::Bool(b) => Val::Bool(*b),
            Node::Var(v, at) => self.var(*v, *at)?,
            Node::Local(slot) => self.slots.get(*slot).cloned().flatten().unwrap_or(Val::Num(0.0)),
            Node::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match self.eval(item)? {
                        Val::Vec(v) => out.extend(v),
                        Val::Str(s) => return Err(format!("a vector holds numbers, not text (\"{s}\"); colours go on their own, like \"#ff5a36\"")),
                        v => out.push(v.scalar().unwrap_or(0.0)),
                    }
                }
                Val::Vec(out)
            }
            Node::Neg(v) => match self.eval(v)? {
                Val::Vec(v) => Val::Vec(v.iter().map(|x| -x).collect()),
                Val::Str(s) => return Err(format!("can't negate text (\"{s}\")")),
                v => Val::Num(-v.scalar().unwrap_or(0.0)),
            },
            Node::Not(v) => Val::Bool(!self.eval(v)?.truthy()),
            Node::And(a, b) => {
                let a = self.eval(a)?;
                if a.truthy() { self.eval(b)? } else { a }
            }
            Node::Or(a, b) => {
                let a = self.eval(a)?;
                if a.truthy() { a } else { self.eval(b)? }
            }
            Node::Cond(c, yes, no) => {
                if self.eval(c)?.truthy() {
                    self.eval(yes)?
                } else {
                    self.eval(no)?
                }
            }
            Node::Bin(op, a, b, at) => {
                let (a, b) = (self.eval(a)?, self.eval(b)?);
                self.binary(*op, a, b, *at)?
            }
            Node::Axis(v, i, at) => {
                let v = self.eval(v)?;
                self.element(v, *i, *at)?
            }
            Node::Index(v, i, at) => {
                let v = self.eval(v)?;
                let i = self.eval(i)?;
                let k = self.num(&i, "[…]", *at)?;
                if k < 0.0 || k.fract() != 0.0 {
                    return Err(self.err(*at, format!("[{}]: positions are whole numbers from 0", fmt_num(k))));
                }
                self.element(v, k as usize, *at)?
            }
            Node::Call(i, args, at) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a)?);
                }
                let (name, f, _, _) = FUNCS[*i];
                self.call(name, f, vals, *at)?
            }
        })
    }

    fn element(&self, v: Val, i: usize, at: usize) -> R<Val> {
        let axis = ["x", "y", "z", "w"].get(i).map_or_else(|| format!("[{i}]"), |a| format!(".{a}"));
        match &v {
            Val::Vec(items) => items
                .get(i)
                .map(|n| Val::Num(*n))
                .ok_or_else(|| self.err(at, format!("{axis}: this vector has {} number{} ({})", items.len(), if items.len() == 1 { "" } else { "s" }, v.shown()))),
            Val::Str(s) => match Rgba::parse(s) {
                Some(c) if i < 4 => Ok(Val::Num(c.0[i] / 255.0)),
                _ => Err(self.err(at, format!("{axis}: \"{s}\" is text, not a vector"))),
            },
            Val::Num(n) if i == 0 => Ok(Val::Num(*n)),
            other => Err(self.err(at, format!("{axis} needs a vector; this is {} ({})", other.kind(), other.shown()))),
        }
    }

    fn var(&mut self, v: Var, at: usize) -> R<Val> {
        let fps = self.fps();
        Ok(Val::Num(match v {
            Var::Time => self.time,
            Var::Value => return self.value(at),
            Var::Index => self.ctx.index(),
            Var::Fps => fps,
            Var::Frame => (self.time * fps + 1e-6).floor(),
            Var::Duration => self.ctx.duration(),
            Var::Pi => std::f64::consts::PI,
            Var::E => std::f64::consts::E,
            Var::Seed => (self.ctx.seed() % 1_000_000) as f64,
            Var::Velocity => return self.velocity(self.time, at),
            Var::Speed => {
                let v = self.velocity(self.time, at)?;
                length(&v.numbers().unwrap_or_default())
            }
            Var::NumKeys => self.ctx.keys().len() as f64,
        }))
    }

    fn fps(&self) -> f64 {
        let f = self.ctx.fps();
        if f.is_finite() && f > 0.0 { f } else { 30.0 }
    }

    fn value(&self, at: usize) -> R<Val> {
        let v = if self.time != self.ctx.time() { self.ctx.value_at(self.time) } else { self.ctx.value() };
        v.map(Val::from_key).ok_or_else(|| self.err(at, "`value`: this property has no value to start from (give it one, or keyframes)"))
    }

    fn value_at(&self, t: f64, at: usize) -> R<Val> {
        self.ctx.value_at(t).map(Val::from_key).ok_or_else(|| self.err(at, "this property has no value to read at another time"))
    }

    fn velocity(&self, t: f64, at: usize) -> R<Val> {
        const H: f64 = 1e-3;
        let (a, b) = (self.value_at(t - H, at)?, self.value_at(t + H, at)?);
        let d = self.binary(Op::Sub, b, a, at)?;
        self.binary(Op::Div, d, Val::Num(2.0 * H), at)
    }

    /// A random number 0..1: the same for the same property, frame, `seedRandom` and draw.
    fn draw(&mut self) -> f64 {
        let mut h = mix(self.ctx.seed(), self.seed_offset);
        if !self.timeless {
            let frame = (self.time * self.fps() + 1e-6).floor() as i64;
            h = mix(h, frame as u64);
        }
        h = mix(h, self.draws);
        self.draws += 1;
        (h >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let u1 = self.draw().max(1e-12);
        let u2 = self.draw();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    fn binary(&self, op: Op, a: Val, b: Val, at: usize) -> R<Val> {
        let sym = op.symbol();
        match op {
            Op::Eq | Op::Ne => {
                let same = match (&a, &b) {
                    (Val::Str(x), Val::Str(y)) => x == y,
                    (Val::Vec(x), Val::Vec(y)) => x == y,
                    (x, y) => match (x.scalar(), y.scalar()) {
                        (Some(p), Some(q)) => p == q,
                        _ => false,
                    },
                };
                Ok(Val::Bool(same == (op == Op::Eq)))
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let (Some(x), Some(y)) = (a.scalar(), b.scalar()) else {
                    return Err(self.err(at, format!("`{sym}` compares numbers, not {} and {}; compare .x or length(…)", a.kind(), b.kind())));
                };
                Ok(Val::Bool(match op {
                    Op::Lt => x < y,
                    Op::Le => x <= y,
                    Op::Gt => x > y,
                    _ => x >= y,
                }))
            }
            Op::Add if matches!(a, Val::Str(_)) || matches!(b, Val::Str(_)) => Ok(Val::Str(Arc::from(format!("{}{}", a.shown(), b.shown()).as_str()))),
            _ => {
                let f = |x: f64, y: f64| match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div => x / y,
                    Op::Rem => x % y,
                    _ => x.powf(y),
                };
                self.zip(&a, &b, matches!(op, Op::Add | Op::Sub), f).map_err(|e| self.err(at, format!("`{sym}`: {e}")))
            }
        }
    }

    /// Element by element; a number goes with every element. Vectors of different lengths add
    /// (the missing numbers count as 0) but don't multiply.
    fn zip(&self, a: &Val, b: &Val, pad: bool, f: impl Fn(f64, f64) -> f64) -> Result<Val, String> {
        match (a, b) {
            (Val::Vec(x), Val::Vec(y)) => {
                if x.len() != y.len() && !pad {
                    return Err(format!("the vectors have {} and {} numbers", x.len(), y.len()));
                }
                let n = x.len().max(y.len());
                Ok(Val::Vec((0..n).map(|i| f(x.get(i).copied().unwrap_or(0.0), y.get(i).copied().unwrap_or(0.0))).collect()))
            }
            (Val::Vec(x), y) => {
                let y = y.scalar().ok_or_else(|| format!("can't do maths with {}", y.kind()))?;
                Ok(Val::Vec(x.iter().map(|x| f(*x, y)).collect()))
            }
            (x, Val::Vec(y)) => {
                let x = x.scalar().ok_or_else(|| format!("can't do maths with {}", x.kind()))?;
                Ok(Val::Vec(y.iter().map(|y| f(x, *y)).collect()))
            }
            (x, y) => match (x.scalar(), y.scalar()) {
                (Some(x), Some(y)) => Ok(Val::Num(f(x, y))),
                _ => Err(format!("can't do maths with {} and {}", x.kind(), y.kind())),
            },
        }
    }

    fn map(&self, v: &Val, what: &str, at: usize, f: impl Fn(f64) -> f64) -> R<Val> {
        match v {
            Val::Vec(items) => Ok(Val::Vec(items.iter().map(|x| f(*x)).collect())),
            other => Ok(Val::Num(f(self.num(other, what, at)?))),
        }
    }

    fn vector(&self, v: &Val, what: &str, at: usize) -> R<Vec<f64>> {
        v.numbers().ok_or_else(|| self.err(at, format!("{what} takes numbers or vectors, not text")))
    }

    fn call(&mut self, name: &str, f: Func, a: Vec<Val>, at: usize) -> R<Val> {
        let opt = |i: usize| a.get(i);
        match f {
            Func::Math(g) => self.map(&a[0], name, at, g),
            Func::Round => {
                let d = match opt(1) {
                    Some(v) => self.num(v, "round's decimals", at)?.clamp(0.0, 12.0),
                    None => 0.0,
                };
                let k = 10f64.powi(d as i32);
                self.map(&a[0], name, at, |x| (x * k + 0.5).floor() / k)
            }
            Func::Atan2 => Ok(Val::Num(self.num(&a[0], name, at)?.atan2(self.num(&a[1], name, at)?))),
            Func::Pow => self.zip(&a[0], &a[1], false, f64::powf).map_err(|e| self.err(at, format!("pow: {e}"))),
            Func::Min | Func::Max => {
                let pick = |x: f64, y: f64| if matches!(f, Func::Min) { x.min(y) } else { x.max(y) };
                if a.len() == 1 {
                    let v = self.vector(&a[0], name, at)?;
                    return v.into_iter().reduce(pick).map(Val::Num).ok_or_else(|| self.err(at, format!("{name} of an empty vector")));
                }
                let mut acc = a[0].clone();
                for v in &a[1..] {
                    acc = self.zip(&acc, v, false, pick).map_err(|e| self.err(at, format!("{name}: {e}")))?;
                }
                Ok(acc)
            }
            Func::Clamp => {
                let lo = self.zip(&a[0], &a[1], false, f64::max).map_err(|e| self.err(at, format!("clamp: {e}")))?;
                self.zip(&lo, &a[2], false, f64::min).map_err(|e| self.err(at, format!("clamp: {e}")))
            }
            Func::Mod => {
                if a[1].scalar() == Some(0.0) {
                    return Err(self.err(at, "mod(a, 0): can't divide by 0"));
                }
                self.zip(&a[0], &a[1], false, f64::rem_euclid).map_err(|e| self.err(at, format!("mod: {e}")))
            }
            Func::Length => {
                let v = match opt(1) {
                    Some(b) => self.binary(Op::Sub, a[0].clone(), b.clone(), at)?,
                    None => a[0].clone(),
                };
                Ok(Val::Num(length(&self.vector(&v, name, at)?)))
            }
            Func::Distance => {
                let d = self.binary(Op::Sub, a[0].clone(), a[1].clone(), at)?;
                Ok(Val::Num(length(&self.vector(&d, name, at)?)))
            }
            Func::Normalize => {
                let v = self.vector(&a[0], name, at)?;
                let l = length(&v);
                Ok(match &a[0] {
                    Val::Vec(_) => Val::Vec(v.iter().map(|x| if l > 1e-12 { x / l } else { 0.0 }).collect()),
                    _ => Val::Num(if l > 1e-12 { v[0] / l } else { 0.0 }),
                })
            }
            Func::Dot => {
                let (x, y) = (self.vector(&a[0], name, at)?, self.vector(&a[1], name, at)?);
                if x.len() != y.len() {
                    return Err(self.err(at, format!("dot: the vectors have {} and {} numbers", x.len(), y.len())));
                }
                Ok(Val::Num(x.iter().zip(&y).map(|(p, q)| p * q).sum()))
            }
            Func::Cross => {
                let (x, y) = (self.vector(&a[0], name, at)?, self.vector(&a[1], name, at)?);
                match (x.len(), y.len()) {
                    (3, 3) => Ok(Val::Vec(vec![x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]])),
                    (2, 2) => Ok(Val::Num(x[0] * y[1] - x[1] * y[0])),
                    _ => Err(self.err(at, "cross takes two 3D vectors (or two 2D ones, giving a number)")),
                }
            }
            Func::Linear | Func::Ease | Func::EaseIn | Func::EaseOut => {
                let t = self.num(&a[0], name, at)?;
                let (p, from, to) = if a.len() == 3 {
                    (t.clamp(0.0, 1.0), &a[1], &a[2])
                } else {
                    let (t0, t1) = (self.num(&a[1], name, at)?, self.num(&a[2], name, at)?);
                    let p = if (t1 - t0).abs() < 1e-12 {
                        if t < t0 { 0.0 } else { 1.0 }
                    } else {
                        ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
                    };
                    (p, &a[3], &a[4])
                };
                // After Effects' shapes: ease = smoothstep, easeIn slow at the start, easeOut slow at the end.
                let p = match f {
                    Func::Ease => p * p * (3.0 - 2.0 * p),
                    Func::EaseIn => p * p * (2.0 - p),
                    Func::EaseOut => p + p * p - p * p * p,
                    _ => p,
                };
                Ok(blend(from, to, p))
            }
            Func::Smoothstep => {
                let (e0, e1, x) = (self.num(&a[0], name, at)?, self.num(&a[1], name, at)?, self.num(&a[2], name, at)?);
                let p = if (e1 - e0).abs() < 1e-12 { if x < e0 { 0.0 } else { 1.0 } } else { ((x - e0) / (e1 - e0)).clamp(0.0, 1.0) };
                Ok(Val::Num(p * p * (3.0 - 2.0 * p)))
            }
            Func::Step => {
                let (edge, x) = (self.num(&a[0], name, at)?, self.num(&a[1], name, at)?);
                Ok(Val::Num(if x < edge { 0.0 } else { 1.0 }))
            }
            Func::Mix => {
                let p = self.num(&a[2], name, at)?;
                Ok(blend(&a[0], &a[1], p))
            }
            Func::Random | Func::GaussRandom => {
                let gauss = matches!(f, Func::GaussRandom);
                let (lo, hi) = match a.len() {
                    0 => (Val::Num(0.0), Val::Num(1.0)),
                    1 => (zero_like(&a[0]), a[0].clone()),
                    _ => (a[0].clone(), a[1].clone()),
                };
                let lo_n = self.vector(&lo, name, at)?;
                let hi_n = self.vector(&hi, name, at)?;
                let n = lo_n.len().max(hi_n.len());
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    let (l, h) = (pick(&lo_n, i), pick(&hi_n, i));
                    let u = if gauss { 0.5 + self.gauss() * 0.5 / 1.645 } else { self.draw() };
                    out.push(l + (h - l) * u);
                }
                Ok(if matches!(lo, Val::Vec(_)) || matches!(hi, Val::Vec(_)) { Val::Vec(out) } else { Val::Num(out[0]) })
            }
            Func::SeedRandom => {
                let n = self.num(&a[0], name, at)?;
                self.seed_offset = n.to_bits();
                self.timeless = opt(1).is_some_and(Val::truthy);
                self.draws = 0;
                Ok(Val::Num(n))
            }
            Func::Noise => {
                let mut p = vec![];
                for v in &a {
                    p.extend(self.vector(v, name, at)?);
                }
                Ok(Val::Num(match p.len() {
                    1 => noise1(p[0], NOISE_SEED),
                    2 => noise3(p[0], p[1], 0.0, NOISE_SEED),
                    3 => noise3(p[0], p[1], p[2], NOISE_SEED),
                    n => return Err(self.err(at, format!("noise takes 1 to 3 numbers, not {n}"))),
                }))
            }
            Func::Wiggle => self.wiggle(&a, at),
            Func::LoopOut | Func::LoopIn => self.looped(matches!(f, Func::LoopOut), &a, name, at),
            Func::ValueAtTime => {
                let t = self.num(&a[0], name, at)?;
                self.value_at(t, at)
            }
            Func::VelocityAtTime => {
                let t = self.num(&a[0], name, at)?;
                self.velocity(t, at)
            }
            Func::KeyTime | Func::KeyValue => {
                let n = self.num(&a[0], name, at)?;
                let keys = self.ctx.keys();
                if n < 1.0 || n.fract() != 0.0 || n as usize > keys.len() {
                    let have = match keys.len() {
                        0 => "no keyframes".to_string(),
                        1 => "1 keyframe".to_string(),
                        k => format!("{k} keyframes (1–{k})"),
                    };
                    return Err(self.err(at, format!("{name}({}): this property has {have}", fmt_num(n))));
                }
                let k = &keys[n as usize - 1];
                Ok(if matches!(f, Func::KeyTime) { Val::Num(k.time) } else { Val::from_key(k.value.clone()) })
            }
            Func::Prop => {
                let id = self.text(&a[0], "prop's first argument (an id)", at)?.to_string();
                let prop = self.text(&a[1], "prop's second argument (a property name)", at)?.to_string();
                let t = match opt(2) {
                    Some(v) => Some(self.num(v, "prop's time", at)?),
                    None if self.time != self.ctx.time() => Some(self.time),
                    None => None,
                };
                self.ctx.prop(&id, &prop, t).map(Val::from_key).map_err(|e| self.err(at, format!("prop(\"{id}\", \"{prop}\"): {e}")))
            }
            Func::Rgb | Func::Rgba => {
                let mut c = [0.0, 0.0, 0.0, 1.0];
                for (i, v) in a.iter().enumerate() {
                    c[i] = self.num(v, name, at)?;
                }
                Ok(color([c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, c[3]]))
            }
            Func::Hsl | Func::Hsla => {
                let h = self.num(&a[0], name, at)?;
                let pct = |x: f64| if x > 1.0 { x / 100.0 } else { x };
                let s = pct(self.num(&a[1], name, at)?);
                let l = pct(self.num(&a[2], name, at)?);
                let alpha = match opt(3) {
                    Some(v) => self.num(v, name, at)?,
                    None => 1.0,
                };
                let [r, g, b] = hsl(h, s, l);
                Ok(color([r, g, b, alpha]))
            }
            Func::Hex => match &a[0] {
                Val::Str(s) => Rgba::parse(s).map(|c| Val::Str(Arc::from(c.to_hex().as_str()))).ok_or_else(|| self.err(at, format!("hex: \"{s}\" isn't a colour"))),
                v => {
                    let n = self.vector(v, name, at)?;
                    if !(3..=4).contains(&n.len()) {
                        return Err(self.err(at, "hex takes [r, g, b] or [r, g, b, a] with channels 0–1"));
                    }
                    Ok(color([n[0], n[1], n[2], n.get(3).copied().unwrap_or(1.0)]))
                }
            },
            Func::HexToRgb => {
                let s = self.text(&a[0], name, at)?;
                let c = Rgba::parse(s).ok_or_else(|| self.err(at, format!("hexToRgb: \"{s}\" isn't a colour like \"#ff5a36\"")))?;
                Ok(Val::Vec(c.0.iter().map(|x| x / 255.0).collect()))
            }
            Func::PosterizeTime => {
                let f = self.num(&a[0], name, at)?;
                if f > 0.0 && f.is_finite() {
                    self.time = (self.time * f + 1e-6).floor() / f;
                }
                Ok(Val::Num(f))
            }
            Func::TimeToFrames => {
                let t = match opt(0) {
                    Some(v) => self.num(v, name, at)?,
                    None => self.time,
                };
                Ok(Val::Num((t * self.fps() + 1e-6).floor()))
            }
            Func::FramesToTime => Ok(Val::Num(self.num(&a[0], name, at)? / self.fps())),
            Func::ToFixed => {
                let x = self.num(&a[0], name, at)?;
                let d = match opt(1) {
                    Some(v) => self.num(v, name, at)?.clamp(0.0, 12.0) as usize,
                    None => 0,
                };
                // Halves round up, like JavaScript (Rust's formatting rounds them to even).
                let k = 10f64.powi(d as i32);
                let x = if x.is_finite() && (x * k).abs() < 1e15 { (x * k + 0.5).floor() / k } else { x };
                Ok(Val::Str(Arc::from(format!("{x:.d$}").as_str())))
            }
        }
    }

    /// Smooth random motion around the value: `freq` wiggles a second, about `amp` away.
    fn wiggle(&mut self, a: &[Val], at: usize) -> R<Val> {
        let freq = self.num(&a[0], "wiggle's frequency", at)?.max(0.0);
        let amp = self.vector(&a[1], "wiggle's amount", at)?;
        let octaves = match a.get(2) {
            Some(v) => self.num(v, "wiggle's octaves", at)?.round().clamp(1.0, 10.0) as u32,
            None => 1,
        };
        let amp_mult = match a.get(3) {
            Some(v) => self.num(v, "wiggle's ampMult", at)?,
            None => 0.5,
        };
        let t = match a.get(4) {
            Some(v) => self.num(v, "wiggle's time", at)?,
            None => self.time,
        };
        let base = self.value(at)?;
        let colour = match &base {
            Val::Str(s) => Some(Rgba::parse(s).ok_or_else(|| self.err(at, format!("wiggle moves numbers, vectors and colours, not \"{s}\"")))?),
            _ => None,
        };
        let mut v = match (&base, colour) {
            (_, Some(c)) => c.0.iter().map(|x| x / 255.0).collect(),
            (b, None) => self.vector(b, "wiggle", at)?,
        };
        let seed = mix(self.ctx.seed(), self.seed_offset ^ 0x5767_6c65);
        let moving = if colour.is_some() { 3 } else { v.len() };
        for (i, x) in v.iter_mut().enumerate().take(moving) {
            let s = mix(seed, i as u64);
            let mut sum = 0.0;
            let (mut f, mut k) = (freq, 1.0);
            for o in 0..octaves {
                // Each axis and octave starts somewhere else along the noise.
                let offset = (mix(s, o as u64 + 100) % 10_000) as f64 + 0.37;
                sum += noise1(t * f + offset, mix(s, o as u64)) * k;
                f *= 2.0;
                k *= amp_mult;
            }
            *x += pick(&amp, i) * sum;
        }
        Ok(match (base, colour) {
            (_, Some(_)) => color([v[0], v[1], v[2], v[3]]),
            (Val::Vec(_), _) => Val::Vec(v),
            _ => Val::Num(v[0]),
        })
    }

    /// `loopOut`/`loopIn`: repeat the keyframes after the last (before the first).
    fn looped(&mut self, out: bool, a: &[Val], name: &str, at: usize) -> R<Val> {
        const KINDS: &[&str] = &["cycle", "pingpong", "offset", "continue"];
        let kind = match a.first() {
            Some(v) => self.text(v, &format!("{name}'s type"), at)?.to_string(),
            None => "cycle".to_string(),
        };
        if !KINDS.contains(&kind.as_str()) {
            let hint = crate::closest(&kind, KINDS).map(|c| format!(" Did you mean \"{c}\"?")).unwrap_or_default();
            return Err(self.err(at, format!("{name}(\"{kind}\"): the types are cycle, pingpong, offset and continue.{hint}")));
        }
        let n = match a.get(1) {
            Some(v) => self.num(v, &format!("{name}'s numKeyframes"), at)?.max(0.0) as usize,
            None => 0,
        };
        let keys = self.ctx.keys();
        let len = keys.len();
        if len < 2 {
            return self.value(at);
        }
        let (first, last) = match (out, n) {
            (true, n) if n > 0 && n < len - 1 => (len - 1 - n, len - 1),
            (false, n) if n > 0 && n < len - 1 => (0, n),
            _ => (0, len - 1),
        };
        let (t0, t1) = (keys[first].time, keys[last].time);
        let span = t1 - t0;
        let t = self.time;
        if span <= 1e-9 || (out && t <= t1) || (!out && t >= t0) {
            return self.value(at);
        }
        let cycles = ((t - t0) / span).floor();
        let rest = (t - t0) - cycles * span;
        match kind.as_str() {
            "cycle" => self.value_at(t0 + rest, at),
            "pingpong" => {
                let odd = (cycles as i64).rem_euclid(2) == 1;
                self.value_at(if odd { t1 - rest } else { t0 + rest }, at)
            }
            "offset" => {
                let base = self.value_at(t0 + rest, at)?;
                let (v0, v1) = (self.value_at(t0, at)?, self.value_at(t1, at)?);
                if matches!(base, Val::Str(_)) {
                    return Ok(base);
                }
                let step = self.binary(Op::Sub, v1, v0, at)?;
                let shift = self.binary(Op::Mul, step, Val::Num(cycles), at)?;
                self.binary(Op::Add, base, shift, at)
            }
            _ => {
                // continue: carry on at the speed it had at the end (start).
                let edge = if out { t1 } else { t0 };
                let h = (span * 0.5).min(1e-3);
                let (p, q) = if out { (self.value_at(t1 - h, at)?, self.value_at(t1, at)?) } else { (self.value_at(t0, at)?, self.value_at(t0 + h, at)?) };
                if matches!(q, Val::Str(_)) {
                    return Ok(if out { q } else { p });
                }
                let d = self.binary(Op::Sub, q.clone(), p.clone(), at)?;
                let speed = self.binary(Op::Div, d, Val::Num(h), at)?;
                let moved = self.binary(Op::Mul, speed, Val::Num(t - edge), at)?;
                self.binary(Op::Add, if out { q } else { p }, moved, at)
            }
        }
    }
}

/// `noise()` doesn't depend on the property: `noise(time)` is the same everywhere (add `index`
/// or `seed` to make it differ).
const NOISE_SEED: u64 = 0x6b69_6d63_6869;

fn pick(v: &[f64], i: usize) -> f64 {
    match v.len() {
        0 => 0.0,
        1 => v[0],
        _ => v.get(i).copied().unwrap_or(0.0),
    }
}

fn zero_like(v: &Val) -> Val {
    match v {
        Val::Vec(x) => Val::Vec(vec![0.0; x.len()]),
        _ => Val::Num(0.0),
    }
}

fn length(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

/// From `a` towards `b` by `p` (numbers, vectors, colours; other text switches at the end).
fn blend(a: &Val, b: &Val, p: f64) -> Val {
    Val::from_key(a.to_key().lerp(&b.to_key(), p))
}

/// A colour from 0–1 channels.
fn color(c: [f64; 4]) -> Val {
    let c = Rgba(c.map(|x| (if x.is_finite() { x } else { 0.0 }).clamp(0.0, 1.0) * 255.0));
    Val::Str(Arc::from(c.to_hex().as_str()))
}

/// Hue in degrees, saturation and lightness 0–1 → red, green, blue 0–1.
fn hsl(h: f64, s: f64, l: f64) -> [f64; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}
