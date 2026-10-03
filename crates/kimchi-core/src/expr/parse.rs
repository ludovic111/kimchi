//! Reading a formula: characters → tokens → a tree, with every name checked and every function
//! resolved (and its number of arguments checked) before the formula ever runs.

use std::sync::Arc;

use super::{MAX_LEN, Program, place};

/// How deep brackets and calls may nest (keeps the reader and the runner off deep recursion).
const MAX_DEPTH: u32 = 64;

// ---------------------------------------------------------------------------------------------
// The tree

#[derive(Debug, Clone)]
pub(super) enum Stmt {
    /// `let x = …` (also `var`, `const`, or a plain `x = …`).
    Let(usize, Node),
    Expr(Node),
}

#[derive(Debug, Clone)]
pub(super) enum Node {
    Num(f64),
    Str(Arc<str>),
    Bool(bool),
    Var(Var, usize),
    Local(usize),
    Array(Vec<Node>),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>, usize),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Cond(Box<Node>, Box<Node>, Box<Node>),
    /// `.x .y .z .w` (0–3).
    Axis(Box<Node>, usize, usize),
    Index(Box<Node>, Box<Node>, usize),
    /// A function by its place in [`FUNCS`], its arguments, where it is.
    Call(usize, Vec<Node>, usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
            Op::Rem => "%",
            Op::Pow => "**",
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        }
    }
}

/// Names a formula can read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Var {
    Time,
    Value,
    Index,
    Fps,
    Frame,
    Duration,
    Pi,
    E,
    Seed,
    Velocity,
    Speed,
    NumKeys,
}

pub(super) const VARS: &[(&str, Var)] = &[
    ("time", Var::Time),
    ("t", Var::Time),
    ("value", Var::Value),
    ("index", Var::Index),
    ("fps", Var::Fps),
    ("frame", Var::Frame),
    ("duration", Var::Duration),
    ("pi", Var::Pi),
    ("PI", Var::Pi),
    ("e", Var::E),
    ("E", Var::E),
    ("seed", Var::Seed),
    ("velocity", Var::Velocity),
    ("speed", Var::Speed),
    ("numKeys", Var::NumKeys),
];

/// Functions, resolved when the formula is read.
#[derive(Debug, Clone, Copy)]
pub(super) enum Func {
    /// One-number maths, applied to each element of a vector.
    Math(fn(f64) -> f64),
    Round,
    Atan2,
    Pow,
    Min,
    Max,
    Clamp,
    Mod,
    Length,
    Normalize,
    Dot,
    Cross,
    Distance,
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    Smoothstep,
    Step,
    Mix,
    Random,
    GaussRandom,
    SeedRandom,
    Noise,
    Wiggle,
    LoopOut,
    LoopIn,
    ValueAtTime,
    VelocityAtTime,
    KeyTime,
    KeyValue,
    Prop,
    Rgb,
    Rgba,
    Hsl,
    Hsla,
    Hex,
    HexToRgb,
    PosterizeTime,
    TimeToFrames,
    FramesToTime,
    ToFixed,
}

fn deg(x: f64) -> f64 {
    x.to_degrees()
}
fn rad(x: f64) -> f64 {
    x.to_radians()
}
fn fract(x: f64) -> f64 {
    x - x.floor()
}
fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Name, function, fewest and most arguments.
pub(super) const FUNCS: &[(&str, Func, usize, usize)] = &[
    ("sin", Func::Math(f64::sin), 1, 1),
    ("cos", Func::Math(f64::cos), 1, 1),
    ("tan", Func::Math(f64::tan), 1, 1),
    ("asin", Func::Math(f64::asin), 1, 1),
    ("acos", Func::Math(f64::acos), 1, 1),
    ("atan", Func::Math(f64::atan), 1, 1),
    ("sqrt", Func::Math(f64::sqrt), 1, 1),
    ("exp", Func::Math(f64::exp), 1, 1),
    ("log", Func::Math(f64::ln), 1, 1),
    ("abs", Func::Math(f64::abs), 1, 1),
    ("sign", Func::Math(sign), 1, 1),
    ("floor", Func::Math(f64::floor), 1, 1),
    ("ceil", Func::Math(f64::ceil), 1, 1),
    ("trunc", Func::Math(f64::trunc), 1, 1),
    ("fract", Func::Math(fract), 1, 1),
    ("deg", Func::Math(deg), 1, 1),
    ("rad", Func::Math(rad), 1, 1),
    ("radiansToDegrees", Func::Math(deg), 1, 1),
    ("degreesToRadians", Func::Math(rad), 1, 1),
    ("round", Func::Round, 1, 2),
    ("atan2", Func::Atan2, 2, 2),
    ("pow", Func::Pow, 2, 2),
    ("min", Func::Min, 1, 32),
    ("max", Func::Max, 1, 32),
    ("clamp", Func::Clamp, 3, 3),
    ("mod", Func::Mod, 2, 2),
    ("length", Func::Length, 1, 2),
    ("normalize", Func::Normalize, 1, 1),
    ("dot", Func::Dot, 2, 2),
    ("cross", Func::Cross, 2, 2),
    ("distance", Func::Distance, 2, 2),
    ("linear", Func::Linear, 3, 5),
    ("ease", Func::Ease, 3, 5),
    ("easeIn", Func::EaseIn, 3, 5),
    ("easeOut", Func::EaseOut, 3, 5),
    ("smoothstep", Func::Smoothstep, 3, 3),
    ("step", Func::Step, 2, 2),
    ("mix", Func::Mix, 3, 3),
    ("lerp", Func::Mix, 3, 3),
    ("random", Func::Random, 0, 2),
    ("gaussRandom", Func::GaussRandom, 0, 2),
    ("seedRandom", Func::SeedRandom, 1, 2),
    ("noise", Func::Noise, 1, 3),
    ("wiggle", Func::Wiggle, 2, 5),
    ("loopOut", Func::LoopOut, 0, 2),
    ("loopIn", Func::LoopIn, 0, 2),
    ("valueAtTime", Func::ValueAtTime, 1, 1),
    ("velocityAtTime", Func::VelocityAtTime, 1, 1),
    ("keyTime", Func::KeyTime, 1, 1),
    ("keyValue", Func::KeyValue, 1, 1),
    ("prop", Func::Prop, 2, 3),
    ("rgb", Func::Rgb, 3, 3),
    ("rgba", Func::Rgba, 4, 4),
    ("hsl", Func::Hsl, 3, 3),
    ("hsla", Func::Hsla, 4, 4),
    ("hex", Func::Hex, 1, 1),
    ("hexToRgb", Func::HexToRgb, 1, 1),
    ("posterizeTime", Func::PosterizeTime, 1, 1),
    ("timeToFrames", Func::TimeToFrames, 0, 1),
    ("framesToTime", Func::FramesToTime, 1, 1),
    ("toFixed", Func::ToFixed, 1, 2),
];

/// The place of `keyValue` in [`FUNCS`] (`key(n)` is `keyValue(n)`).
fn key_value_index() -> usize {
    FUNCS.iter().position(|(n, ..)| *n == "keyValue").unwrap_or(0)
}

fn key_time_index() -> usize {
    FUNCS.iter().position(|(n, ..)| *n == "keyTime").unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------
// Tokens

#[derive(Debug, Clone, PartialEq)]
enum Tok<'s> {
    Num(f64),
    Str(String),
    Ident(&'s str),
    P(&'static str),
    End,
}

const PUNCT: &[&str] = &[
    "===", "!==", "**", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "^", "(", ")", "[", "]", ",", ";", ".", "?", ":",
    "<", ">", "!", "=",
];

fn lex(src: &str) -> Result<Vec<(Tok<'_>, usize)>, String> {
    let b = src.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if src[i..].starts_with("//") {
            i = src[i..].find('\n').map_or(b.len(), |n| i + n);
            continue;
        }
        if src[i..].starts_with("/*") {
            let end = src[i + 2..].find("*/").ok_or_else(|| format!("a comment `/*` isn't closed with `*/` ({})", place(src, i)))?;
            i += end + 4;
            continue;
        }
        let start = i;
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text = &src[start..i];
            let n: f64 = text.parse().map_err(|_| format!("`{text}` isn't a number ({})", place(src, start)))?;
            out.push((Tok::Num(n), start));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            out.push((Tok::Ident(&src[start..i]), start));
            continue;
        }
        if c == b'"' || c == b'\'' || c == b'`' {
            let mut s = String::new();
            let mut chars = src[i + 1..].char_indices();
            let mut closed = None;
            while let Some((k, ch)) = chars.next() {
                match ch {
                    '\\' => match chars.next() {
                        Some((_, 'n')) => s.push('\n'),
                        Some((_, 't')) => s.push('\t'),
                        Some((_, other)) => s.push(other),
                        None => break,
                    },
                    ch if ch as u32 == c as u32 => {
                        closed = Some(i + 1 + k + 1);
                        break;
                    }
                    ch => s.push(ch),
                }
            }
            let Some(end) = closed else {
                return Err(format!("a text in quotes isn't closed ({})", place(src, start)));
            };
            out.push((Tok::Str(s), start));
            i = end;
            continue;
        }
        let Some(p) = PUNCT.iter().find(|p| src[i..].starts_with(**p)) else {
            let ch = src[i..].chars().next().unwrap_or('?');
            let hint = match ch {
                '{' | '}' => " Formulas are one expression: no blocks, `if` or loops (use `a ? b : c`).",
                '&' | '|' => " Use `&&` and `||`.",
                _ => "",
            };
            return Err(format!("unexpected `{ch}` ({}).{hint}", place(src, i)));
        };
        out.push((Tok::P(p), start));
        i += p.len();
    }
    out.push((Tok::End, src.len()));
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// The parser

struct Parser<'s> {
    src: &'s str,
    toks: Vec<(Tok<'s>, usize)>,
    i: usize,
    locals: Vec<&'s str>,
    depth: u32,
}

pub(super) fn program(src: &str) -> Result<Program, String> {
    if src.trim().is_empty() {
        return Err("the formula is empty".into());
    }
    if src.len() > MAX_LEN {
        return Err(format!("the formula is too long ({} characters; at most {MAX_LEN})", src.len()));
    }
    let mut p = Parser { src, toks: lex(src)?, i: 0, locals: vec![], depth: 0 };
    let mut body = vec![];
    loop {
        while p.eat(";") {}
        if p.peek() == &Tok::End {
            break;
        }
        body.push(p.statement()?);
        if p.peek() == &Tok::End {
            break;
        }
        if !p.eat(";") {
            return Err(p.unexpected("`;` between steps, or the end of the formula"));
        }
    }
    match body.last() {
        Some(Stmt::Expr(_)) => {}
        Some(Stmt::Let(..)) => return Err("the formula ends with `let …`; end it with the value, like `let a = time * 2; a + 1`".into()),
        None => return Err("the formula is empty".into()),
    }
    Ok(Program { src: Arc::from(src), body, slots: p.locals.len() })
}

impl<'s> Parser<'s> {
    fn peek(&self) -> &Tok<'s> {
        self.peek_at(0)
    }

    fn peek_at(&self, k: usize) -> &Tok<'s> {
        &self.toks[(self.i + k).min(self.toks.len() - 1)].0
    }

    fn pos(&self) -> usize {
        self.toks[self.i.min(self.toks.len() - 1)].1
    }

    /// The next token (the end, again and again, once there).
    fn bump(&mut self) -> Tok<'s> {
        let t = self.peek().clone();
        self.i += 1;
        t
    }

    fn eat(&mut self, p: &str) -> bool {
        if matches!(self.peek(), Tok::P(q) if *q == p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str, what: &str) -> Result<(), String> {
        if self.eat(p) { Ok(()) } else { Err(self.unexpected(what)) }
    }

    fn unexpected(&self, wanted: &str) -> String {
        let at = place(self.src, self.pos());
        match self.peek() {
            Tok::End => format!("the formula ends too early: expected {wanted} ({at})"),
            Tok::Num(n) => format!("expected {wanted}, found the number {n} ({at})"),
            Tok::Str(s) => format!("expected {wanted}, found \"{s}\" ({at})"),
            Tok::Ident(n) => format!("expected {wanted}, found `{n}` ({at})"),
            Tok::P(p) => format!("expected {wanted}, found `{p}` ({at})"),
        }
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(format!("the formula nests too deeply (more than {MAX_DEPTH} levels) ({})", place(self.src, self.pos())));
        }
        Ok(())
    }

    fn statement(&mut self) -> Result<Stmt, String> {
        let keyword = matches!(self.peek(), Tok::Ident("let" | "var" | "const"));
        let plain = matches!((self.peek(), self.peek_at(1)), (Tok::Ident(_), Tok::P("=")));
        if keyword || plain {
            if keyword {
                self.bump();
            }
            let at = self.pos();
            let Tok::Ident(name) = self.bump() else {
                self.i -= 1;
                return Err(self.unexpected("a name after `let`"));
            };
            if is_reserved(name) {
                return Err(format!("`{name}` can't be used as a name ({})", place(self.src, at)));
            }
            self.expect("=", "`=` after the name")?;
            let value = self.expr()?;
            let slot = match self.locals.iter().position(|l| *l == name) {
                Some(s) => s,
                None => {
                    self.locals.push(name);
                    self.locals.len() - 1
                }
            };
            return Ok(Stmt::Let(slot, value));
        }
        Ok(Stmt::Expr(self.expr()?))
    }

    fn expr(&mut self) -> Result<Node, String> {
        self.deeper()?;
        let cond = self.or()?;
        let out = if self.eat("?") {
            let yes = self.expr()?;
            self.expect(":", "`:` (a ? b : c)")?;
            let no = self.expr()?;
            Node::Cond(Box::new(cond), Box::new(yes), Box::new(no))
        } else {
            cond
        };
        self.depth -= 1;
        Ok(out)
    }

    fn or(&mut self) -> Result<Node, String> {
        let mut a = self.and()?;
        while self.eat("||") {
            let b = self.and()?;
            a = Node::Or(Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn and(&mut self) -> Result<Node, String> {
        let mut a = self.equality()?;
        while self.eat("&&") {
            let b = self.equality()?;
            a = Node::And(Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn equality(&mut self) -> Result<Node, String> {
        let mut a = self.comparison()?;
        loop {
            let at = self.pos();
            let op = match self.peek() {
                Tok::P("==" | "===") => Op::Eq,
                Tok::P("!=" | "!==") => Op::Ne,
                _ => return Ok(a),
            };
            self.bump();
            let b = self.comparison()?;
            a = Node::Bin(op, Box::new(a), Box::new(b), at);
        }
    }

    fn comparison(&mut self) -> Result<Node, String> {
        let mut a = self.additive()?;
        loop {
            let at = self.pos();
            let op = match self.peek() {
                Tok::P("<") => Op::Lt,
                Tok::P("<=") => Op::Le,
                Tok::P(">") => Op::Gt,
                Tok::P(">=") => Op::Ge,
                _ => return Ok(a),
            };
            self.bump();
            let b = self.additive()?;
            a = Node::Bin(op, Box::new(a), Box::new(b), at);
        }
    }

    fn additive(&mut self) -> Result<Node, String> {
        let mut a = self.term()?;
        loop {
            let at = self.pos();
            let op = match self.peek() {
                Tok::P("+") => Op::Add,
                Tok::P("-") => Op::Sub,
                _ => return Ok(a),
            };
            self.bump();
            let b = self.term()?;
            a = Node::Bin(op, Box::new(a), Box::new(b), at);
        }
    }

    fn term(&mut self) -> Result<Node, String> {
        let mut a = self.unary()?;
        loop {
            let at = self.pos();
            let op = match self.peek() {
                Tok::P("*") => Op::Mul,
                Tok::P("/") => Op::Div,
                Tok::P("%") => Op::Rem,
                _ => return Ok(a),
            };
            self.bump();
            let b = self.unary()?;
            a = Node::Bin(op, Box::new(a), Box::new(b), at);
        }
    }

    fn unary(&mut self) -> Result<Node, String> {
        if self.eat("-") {
            self.deeper()?;
            let v = self.unary()?;
            self.depth -= 1;
            return Ok(match v {
                Node::Num(n) => Node::Num(-n),
                v => Node::Neg(Box::new(v)),
            });
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            self.deeper()?;
            let v = self.unary()?;
            self.depth -= 1;
            return Ok(Node::Not(Box::new(v)));
        }
        self.power()
    }

    /// `a ** b` and `a ^ b`, right to left; `-2 ** 2` is −4.
    fn power(&mut self) -> Result<Node, String> {
        let base = self.postfix()?;
        let at = self.pos();
        if self.eat("**") || self.eat("^") {
            self.deeper()?;
            let exp = self.unary()?;
            self.depth -= 1;
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(exp), at));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Node, String> {
        let mut v = self.primary()?;
        loop {
            let at = self.pos();
            if self.eat("[") {
                let i = self.expr()?;
                self.expect("]", "`]`")?;
                v = Node::Index(Box::new(v), Box::new(i), at);
            } else if self.eat(".") {
                let name_at = self.pos();
                let Tok::Ident(name) = self.bump() else {
                    self.i -= 1;
                    return Err(self.unexpected("a name after `.`"));
                };
                if matches!(self.peek(), Tok::P("(")) {
                    // `value.toFixed(1)` is `toFixed(value, 1)`.
                    let f = self.func(name, name_at)?;
                    self.bump();
                    let mut args = vec![v];
                    args.extend(self.args(")")?);
                    v = self.call(f, args, name_at)?;
                    continue;
                }
                let key = matches!(&v, Node::Call(f, _, _) if *f == key_value_index());
                v = match name {
                    "x" | "r" => Node::Axis(Box::new(v), 0, name_at),
                    "y" | "g" => Node::Axis(Box::new(v), 1, name_at),
                    "z" | "b" => Node::Axis(Box::new(v), 2, name_at),
                    "w" | "a" => Node::Axis(Box::new(v), 3, name_at),
                    // `key(2).time`, `key(2).value`
                    "time" if key => match v {
                        Node::Call(_, args, at) => Node::Call(key_time_index(), args, at),
                        other => other,
                    },
                    "value" if key => v,
                    _ => return Err(format!("`.{name}` isn't something a value has; vectors have .x, .y, .z and .w ({})", place(self.src, name_at))),
                };
            } else {
                return Ok(v);
            }
        }
    }

    fn args(&mut self, close: &str) -> Result<Vec<Node>, String> {
        let mut out = vec![];
        if self.eat(close) {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if self.eat(close) {
                return Ok(out);
            }
            self.expect(",", &format!("`,` or `{close}`"))?;
        }
    }

    /// A function's place in [`FUNCS`].
    fn func(&self, name: &str, at: usize) -> Result<usize, String> {
        if name == "key" {
            return Ok(key_value_index());
        }
        if let Some(i) = FUNCS.iter().position(|(n, ..)| *n == name) {
            return Ok(i);
        }
        let names: Vec<&str> = FUNCS.iter().map(|(n, ..)| *n).collect();
        let hint = crate::closest(name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        let var = if VARS.iter().any(|(v, _)| *v == name) || self.locals.contains(&name) { format!(" `{name}` is a value, not a function.") } else { String::new() };
        Err(format!("unknown function `{name}` ({}).{hint}{var}", place(self.src, at)))
    }

    fn call(&self, i: usize, args: Vec<Node>, at: usize) -> Result<Node, String> {
        let (name, f, lo, hi) = FUNCS[i];
        let n = args.len();
        if n < lo || n > hi {
            let want = if lo == hi { format!("{lo}") } else if hi >= 32 { format!("{lo} or more") } else { format!("{lo} to {hi}") };
            let usage = usage(f);
            return Err(format!("`{name}` takes {want} argument{}, not {n} ({}). {usage}", if hi == 1 && lo == 1 { "" } else { "s" }, place(self.src, at)));
        }
        if matches!(f, Func::Linear | Func::Ease | Func::EaseIn | Func::EaseOut) && n == 4 {
            return Err(format!("`{name}` takes 3 arguments (t, from, to) or 5 (t, tMin, tMax, from, to), not 4 ({})", place(self.src, at)));
        }
        Ok(Node::Call(i, args, at))
    }

    fn primary(&mut self) -> Result<Node, String> {
        let at = self.pos();
        match self.bump() {
            Tok::Num(n) => Ok(Node::Num(n)),
            Tok::Str(s) => Ok(Node::Str(Arc::from(s.as_str()))),
            Tok::P("(") => {
                let v = self.expr()?;
                self.expect(")", "`)`")?;
                Ok(v)
            }
            Tok::P("[") => {
                self.deeper()?;
                let items = self.args("]")?;
                self.depth -= 1;
                if items.is_empty() {
                    return Err(format!("an empty list `[]` isn't a value ({})", place(self.src, at)));
                }
                Ok(Node::Array(items))
            }
            Tok::Ident(name) => self.name(name, at),
            _ => {
                self.i -= 1;
                Err(self.unexpected("a value (a number, a name, a function call, `(` or `[`)"))
            }
        }
    }

    fn name(&mut self, name: &'s str, at: usize) -> Result<Node, String> {
        // `Math.sin(x)`, `Math.PI`: JavaScript habits read as the plain names.
        let name = if name == "Math" && self.eat(".") {
            match self.bump() {
                Tok::Ident(n) => n,
                _ => {
                    self.i -= 1;
                    return Err(self.unexpected("a name after `Math.`"));
                }
            }
        } else {
            name
        };
        match name {
            "true" => return Ok(Node::Bool(true)),
            "false" => return Ok(Node::Bool(false)),
            _ => {}
        }
        if self.eat("(") {
            let f = self.func(name, at)?;
            self.deeper()?;
            let args = self.args(")")?;
            self.depth -= 1;
            return self.call(f, args, at);
        }
        if let Some(slot) = self.locals.iter().position(|l| *l == name) {
            return Ok(Node::Local(slot));
        }
        if let Some((_, v)) = VARS.iter().find(|(n, _)| *n == name) {
            return Ok(Node::Var(*v, at));
        }
        if FUNCS.iter().any(|(n, ..)| *n == name) {
            return Err(format!("`{name}` is a function: call it, like `{name}(…)` ({})", place(self.src, at)));
        }
        let mut names: Vec<&str> = VARS.iter().map(|(n, _)| *n).filter(|n| n.len() > 1).collect();
        names.extend(self.locals.iter().copied());
        let hint = crate::closest(name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        let known = "time, value, index, fps, frame, duration, pi, e, seed, velocity, speed, numKeys";
        Err(format!("unknown name `{name}` ({}).{hint} Names: {known}, and your own from `let`.", place(self.src, at)))
    }
}

fn is_reserved(name: &str) -> bool {
    matches!(name, "true" | "false" | "let" | "var" | "const" | "Math")
}

/// How to call a function, for errors.
pub(super) fn usage(f: Func) -> &'static str {
    match f {
        Func::Math(_) => "Use it like `sin(x)`.",
        Func::Round => "round(x) or round(x, decimals).",
        Func::Atan2 => "atan2(y, x).",
        Func::Pow => "pow(base, exponent).",
        Func::Min | Func::Max => "min(a, b, …) / max(a, b, …).",
        Func::Clamp => "clamp(x, low, high).",
        Func::Mod => "mod(a, b).",
        Func::Length => "length(v) or length(a, b).",
        Func::Normalize => "normalize(v).",
        Func::Dot => "dot(a, b).",
        Func::Cross => "cross(a, b).",
        Func::Distance => "distance(a, b).",
        Func::Linear | Func::Ease | Func::EaseIn | Func::EaseOut => "linear(t, tMin, tMax, from, to) or linear(t, from, to) with t 0–1.",
        Func::Smoothstep => "smoothstep(edge0, edge1, x).",
        Func::Step => "step(edge, x).",
        Func::Mix => "mix(a, b, amount).",
        Func::Random | Func::GaussRandom => "random(), random(max) or random(min, max).",
        Func::SeedRandom => "seedRandom(n) or seedRandom(n, timeless).",
        Func::Noise => "noise(x), noise(x, y) or noise(x, y, z).",
        Func::Wiggle => "wiggle(freq, amp), wiggle(freq, amp, octaves, ampMult, t).",
        Func::LoopOut | Func::LoopIn => "loopOut(), loopOut(\"pingpong\"), loopOut(\"cycle\", numKeyframes).",
        Func::ValueAtTime => "valueAtTime(t).",
        Func::VelocityAtTime => "velocityAtTime(t).",
        Func::KeyTime => "keyTime(n), n from 1.",
        Func::KeyValue => "keyValue(n) (or key(n).value), n from 1.",
        Func::Prop => "prop(\"id\", \"name\") or prop(\"id\", \"name\", time).",
        Func::Rgb => "rgb(r, g, b) with 0–255 channels.",
        Func::Rgba => "rgba(r, g, b, alpha) with 0–255 channels and alpha 0–1.",
        Func::Hsl => "hsl(hue°, saturation 0–1, lightness 0–1).",
        Func::Hsla => "hsla(hue°, saturation 0–1, lightness 0–1, alpha 0–1).",
        Func::Hex => "hex([r, g, b]) with 0–1 channels, or hex(\"#f53\").",
        Func::HexToRgb => "hexToRgb(\"#ff5a36\") → [r, g, b, a] 0–1.",
        Func::PosterizeTime => "posterizeTime(framesPerSecond).",
        Func::TimeToFrames => "timeToFrames() or timeToFrames(t).",
        Func::FramesToTime => "framesToTime(frames).",
        Func::ToFixed => "toFixed(x, decimals) or x.toFixed(decimals).",
    }
}
