//! The Lua table literals Lightroom Classic writes `.lrtemplate` presets in: `s = { key = value,
//! ["key"] = value, value, … }` with strings (`"…"` with escapes, `[[…]]`), numbers, booleans,
//! nested tables, and `ZSTR "$$$/key=Text"` (a translatable string: its text after `=`).
//! Comments (`--`, `--[[ ]]`) are skipped. Nothing is run.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Number(f64),
    Str(String),
    Table(Table),
}

/// A Lua table: named fields and the positional ones in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub fields: BTreeMap<String, Value>,
    pub list: Vec<Value>,
}

impl Value {
    pub fn table(&self) -> Option<&Table> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn err(&self, what: &str) -> String {
        let line = self.s[..self.i.min(self.s.len())].iter().filter(|b| **b == b'\n').count() + 1;
        format!("{what} on line {line}")
    }

    fn skip(&mut self) {
        loop {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                self.i += 2;
                if self.s[self.i..].starts_with(b"[[") {
                    match find(&self.s[self.i..], b"]]") {
                        Some(k) => self.i += k + 2,
                        None => self.i = self.s.len(),
                    }
                } else {
                    while self.i < self.s.len() && self.s[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                continue;
            }
            break;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn name(&mut self) -> Option<String> {
        self.skip();
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
            self.i += 1;
        }
        (self.i > start && !self.s[start].is_ascii_digit()).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn string(&mut self) -> Result<String, String> {
        let q = self.s[self.i];
        if q == b'[' {
            // [[long string]] (or [=[ ]=], rare in presets).
            let eqs = self.s[self.i + 1..].iter().take_while(|b| **b == b'=').count();
            let close: Vec<u8> = std::iter::once(b']').chain(std::iter::repeat_n(b'=', eqs)).chain(std::iter::once(b']')).collect();
            self.i += 2 + eqs;
            if self.s.get(self.i) == Some(&b'\n') {
                self.i += 1;
            }
            let k = find(&self.s[self.i..], &close).ok_or_else(|| self.err("unclosed long string"))?;
            let out = String::from_utf8_lossy(&self.s[self.i..self.i + k]).into_owned();
            self.i += k + close.len();
            return Ok(out);
        }
        self.i += 1;
        let mut out: Vec<u8> = vec![];
        while self.i < self.s.len() {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                _ if c == q => return Ok(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or_else(|| self.err("unclosed string"))?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'\n' => out.push(b'\n'),
                        b'0'..=b'9' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.s.get(self.i) {
                                    Some(d @ b'0'..=b'9') => {
                                        v = v * 10 + (d - b'0') as u32;
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v.min(255) as u8);
                        }
                        other => out.push(other),
                    }
                }
                _ => out.push(c),
            }
        }
        Err(self.err("unclosed string"))
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek().ok_or_else(|| self.err("the file ends early"))? {
            b'{' => self.table().map(Value::Table),
            b'"' | b'\'' => self.string().map(Value::Str),
            b'[' if matches!(self.s.get(self.i + 1), Some(b'[' | b'=')) => self.string().map(Value::Str),
            c if c == b'-' || c == b'.' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'.' | b'-' | b'+')) {
                    // A sign only right after an exponent.
                    if matches!(self.s[self.i], b'-' | b'+') && !matches!(self.s[self.i - 1], b'e' | b'E') {
                        break;
                    }
                    self.i += 1;
                }
                let t = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                let n = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    Some(h) => i64::from_str_radix(h, 16).map(|v| v as f64).ok(),
                    None => t.parse::<f64>().ok(),
                };
                n.map(Value::Number).ok_or_else(|| self.err(&format!("\"{t}\" isn't a number")))
            }
            _ => {
                let word = self.name().ok_or_else(|| self.err("unexpected character"))?;
                match word.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "nil" => Ok(Value::Nil),
                    // ZSTR "$$$/Path/Key=Default text": the text.
                    "ZSTR" | "LOC" => match self.value()? {
                        Value::Str(s) => Ok(Value::Str(zstr(&s))),
                        other => Ok(other),
                    },
                    other => Err(self.err(&format!("`{other}` isn't a value"))),
                }
            }
        }
    }

    fn table(&mut self) -> Result<Table, String> {
        self.eat(b'{');
        let mut t = Table::default();
        loop {
            if self.eat(b'}') {
                return Ok(t);
            }
            let key = match self.peek() {
                Some(b'[') if !matches!(self.s.get(self.i + 1), Some(b'[' | b'=')) => {
                    self.i += 1;
                    let k = self.value()?;
                    if !self.eat(b']') {
                        return Err(self.err("expected ]"));
                    }
                    if !self.eat(b'=') {
                        return Err(self.err("expected ="));
                    }
                    Some(match k {
                        Value::Str(s) => s,
                        Value::Number(n) => format!("{n}"),
                        other => format!("{other:?}"),
                    })
                }
                Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                    let save = self.i;
                    let n = self.name();
                    if n.is_some() && self.eat(b'=') {
                        n
                    } else {
                        self.i = save;
                        None
                    }
                }
                _ => None,
            };
            let v = self.value()?;
            match key {
                Some(k) => {
                    t.fields.insert(k, v);
                }
                None => t.list.push(v),
            }
            if !self.eat(b',') && !self.eat(b';') {
                if self.eat(b'}') {
                    return Ok(t);
                }
                return Err(self.err("expected , or }"));
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `$$$/AgDevelop/Presets/Warm=Warm film` → `Warm film`; other text as it is.
pub fn zstr(s: &str) -> String {
    match s.strip_prefix("$$$/") {
        Some(rest) => rest.split_once('=').map_or_else(|| rest.rsplit('/').next().unwrap_or(rest).to_string(), |(_, t)| t.to_string()),
        None => s.to_string(),
    }
}

/// Reads `s = { … }` (or a bare `{ … }`, or `return { … }`).
pub fn parse(text: &str) -> Result<Table, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut p = P { s: text.as_bytes(), i: 0 };
    p.skip();
    if p.peek() != Some(b'{') {
        let save = p.i;
        match p.name().as_deref() {
            Some("return") => {}
            Some(_) if p.eat(b'=') => {}
            _ => p.i = save,
        }
    }
    if p.peek() != Some(b'{') {
        return Err("not a Lightroom template (it should hold a Lua table: s = { … })".into());
    }
    p.table()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_template() {
        let t = parse(
            r#"s = {
	id = "8C5B",
	internalName = "Warm \"film\"",
	title = ZSTR "$$$/AgDevelop/Presets/Warm=Warm film",
	type = "Develop",
	-- a comment
	value = {
		settings = {
			Exposure2012 = 0.35,
			Contrast2012 = -12,
			ToneCurvePV2012 = { 0, 0, 255, 255, },
			ConvertToGrayscale = false,
			["Odd Key"] = [[long
text]],
		},
	},
	version = 0,
}"#,
        )
        .unwrap();
        assert_eq!(t.get("title"), Some(&Value::Str("Warm film".into())));
        assert_eq!(t.get("internalName"), Some(&Value::Str("Warm \"film\"".into())));
        let settings = t.get("value").and_then(Value::table).and_then(|v| v.get("settings")).and_then(Value::table).unwrap();
        assert_eq!(settings.get("Contrast2012"), Some(&Value::Number(-12.0)));
        assert_eq!(settings.get("ToneCurvePV2012").and_then(Value::table).unwrap().list.len(), 4);
        assert_eq!(settings.get("Odd Key"), Some(&Value::Str("long\ntext".into())));
        assert!(parse("hello").is_err());
        assert!(parse("s = { a = }").is_err());
    }
}
