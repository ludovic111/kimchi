//! Plugin parameter values: read from commands (JSON as people and agents write it), checked
//! against the plugin's [`ParamInfo`], and put in the one form each kind is stored and handed to
//! hosts in:
//!
//! | kind | stored as |
//! |---|---|
//! | number, integer | `Number`, clamped to min…max (integers rounded) |
//! | toggle | `Bool` |
//! | choice | `Text`: the choice's label |
//! | colour | `Vector [r, g, b, a]`, 0…1 straight sRGB (`#rrggbb[aa]` is read too) |
//! | point | `Vector [x, y]`, fractions of the picture from its top left |
//! | text, file | `Text` |

use std::collections::BTreeMap;

use kimchi_core::PluginValue;
use serde_json::Value;

use super::{ParamInfo, ParamKind, PluginInfo};

/// `#rgb`, `#rrggbb` or `#rrggbbaa` as `[r, g, b, a]` 0…1.
pub fn parse_color(s: &str) -> Option<[f64; 4]> {
    let h = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|b| b as f64 / 255.0);
    match h.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(h.get(i..i + 1)?, 16).ok().map(|b| (b * 17) as f64 / 255.0);
            Some([d(0)?, d(1)?, d(2)?, 1.0])
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 1.0]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// `[r, g, b, a]` as `#rrggbb`, or `#rrggbbaa` when not opaque.
pub fn color_hex(c: &[f64]) -> String {
    let b = |i: usize| (c.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
    if b(3) == 255 { format!("#{:02x}{:02x}{:02x}", b(0), b(1), b(2)) } else { format!("#{:02x}{:02x}{:02x}{:02x}", b(0), b(1), b(2), b(3)) }
}

fn clamp(p: &ParamInfo, n: f64) -> f64 {
    let n = if p.kind == ParamKind::Integer { n.round() } else { n };
    if p.max > p.min { n.clamp(p.min, p.max) } else { n }
}

/// The index of a choice value: its label (any case), or a number.
pub fn choice_index(p: &ParamInfo, v: &PluginValue) -> Option<usize> {
    match v {
        PluginValue::Text(t) => p.choices.iter().position(|c| c.eq_ignore_ascii_case(t.trim())).or_else(|| t.trim().parse::<usize>().ok().filter(|&i| i < p.choices.len())),
        PluginValue::Number(n) if n.is_finite() && *n >= 0.0 && (n.round() as usize) < p.choices.len() => Some(n.round() as usize),
        _ => None,
    }
}

/// A value read from a command for `p`, in its stored form; an error says what `p` takes.
pub fn from_json(p: &ParamInfo, v: &Value) -> Result<PluginValue, String> {
    let takes = |what: &str| format!("`{}` takes {what}, not {v}", p.name);
    let numbers = |n: usize| -> Option<Vec<f64>> {
        let a = v.as_array()?;
        (a.len() == n).then(|| a.iter().map(Value::as_f64).collect::<Option<Vec<f64>>>())?.filter(|v| v.iter().all(|x| x.is_finite()))
    };
    Ok(match p.kind {
        ParamKind::Number | ParamKind::Integer => {
            let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().trim_end_matches(p.unit.as_str()).trim().parse().ok())).filter(|n: &f64| n.is_finite());
            PluginValue::Number(clamp(p, n.ok_or_else(|| takes(&format!("a number from {} to {}{}", p.min, p.max, p.unit)))?))
        }
        ParamKind::Toggle => PluginValue::Bool(match v {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().is_some_and(|n| n >= 0.5),
            Value::String(s) if matches!(s.to_ascii_lowercase().as_str(), "on" | "true" | "yes") => true,
            Value::String(s) if matches!(s.to_ascii_lowercase().as_str(), "off" | "false" | "no") => false,
            _ => return Err(takes("true or false")),
        }),
        ParamKind::Choice => {
            let pv = match v {
                Value::String(s) => PluginValue::Text(s.clone()),
                Value::Number(n) => PluginValue::Number(n.as_f64().unwrap_or(-1.0)),
                _ => return Err(takes(&format!("one of {}", p.choices.join(", ")))),
            };
            let Some(i) = choice_index(p, &pv) else {
                let labels: Vec<&str> = p.choices.iter().map(String::as_str).collect();
                let hint = v.as_str().and_then(|s| kimchi_core::closest(s, &labels)).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
                return Err(format!("`{}` is one of {}, not {v}.{hint}", p.name, labels.join(", ")));
            };
            PluginValue::Text(p.choices[i].clone())
        }
        ParamKind::Color => {
            let c = match v {
                Value::String(s) => parse_color(s).map(|c| c.to_vec()),
                Value::Array(_) => numbers(4).or_else(|| numbers(3).map(|mut c| {
                    c.push(1.0);
                    c
                })),
                _ => None,
            };
            PluginValue::Vector(c.ok_or_else(|| takes("a colour: \"#rrggbb\", \"#rrggbbaa\" or [r, g, b, a] from 0 to 1"))?.into_iter().map(|x| x.clamp(0.0, 1.0)).collect())
        }
        ParamKind::Point => PluginValue::Vector(numbers(2).ok_or_else(|| takes("a point [x, y]: fractions of the picture from its top left (0.5, 0.5 is the centre)"))?),
        ParamKind::Text | ParamKind::File => PluginValue::Text(match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return Err(takes("text")),
        }),
    })
}

/// A stored (or keyframed) value in its form for `p`; a value of another kind gives the default.
pub fn normalize(p: &ParamInfo, v: &PluginValue) -> PluginValue {
    let fallback = || normalize_default(p);
    match p.kind {
        ParamKind::Number | ParamKind::Integer => match v.as_f64().filter(|n| n.is_finite()) {
            Some(n) => PluginValue::Number(clamp(p, n)),
            None => fallback(),
        },
        ParamKind::Toggle => match v {
            PluginValue::Bool(b) => PluginValue::Bool(*b),
            PluginValue::Number(n) => PluginValue::Bool(*n >= 0.5),
            _ => fallback(),
        },
        ParamKind::Choice => match choice_index(p, v) {
            Some(i) => PluginValue::Text(p.choices[i].clone()),
            None => fallback(),
        },
        ParamKind::Color => match v {
            PluginValue::Vector(c) if c.len() == 4 => PluginValue::Vector(c.iter().map(|x| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 }).collect()),
            PluginValue::Vector(c) if c.len() == 3 => PluginValue::Vector(c.iter().map(|x| x.clamp(0.0, 1.0)).chain([1.0]).collect()),
            PluginValue::Text(s) => parse_color(s).map(|c| PluginValue::Vector(c.to_vec())).unwrap_or_else(fallback),
            _ => fallback(),
        },
        ParamKind::Point => match v {
            PluginValue::Vector(c) if c.len() == 2 && c.iter().all(|x| x.is_finite()) => v.clone(),
            _ => fallback(),
        },
        ParamKind::Text | ParamKind::File => match v {
            PluginValue::Text(_) => v.clone(),
            _ => fallback(),
        },
    }
}

/// The default in its stored form (a plugin's own default may be written loosely, e.g. a choice
/// by index).
fn normalize_default(p: &ParamInfo) -> PluginValue {
    let d = &p.default;
    match p.kind {
        ParamKind::Number | ParamKind::Integer => PluginValue::Number(clamp(p, d.as_f64().unwrap_or(p.min))),
        ParamKind::Toggle => PluginValue::Bool(d.as_f64().is_some_and(|n| n >= 0.5)),
        ParamKind::Choice => PluginValue::Text(choice_index(p, d).and_then(|i| p.choices.get(i)).or(p.choices.first()).cloned().unwrap_or_default()),
        ParamKind::Color => match d {
            PluginValue::Vector(c) if c.len() >= 3 => PluginValue::Vector(c.iter().copied().chain([1.0]).take(4).collect()),
            PluginValue::Text(s) => PluginValue::Vector(parse_color(s).unwrap_or([0.0, 0.0, 0.0, 1.0]).to_vec()),
            _ => PluginValue::Vector(vec![0.0, 0.0, 0.0, 1.0]),
        },
        ParamKind::Point => match d {
            PluginValue::Vector(c) if c.len() == 2 => d.clone(),
            _ => PluginValue::Vector(vec![0.5, 0.5]),
        },
        ParamKind::Text | ParamKind::File => match d {
            PluginValue::Text(_) => d.clone(),
            _ => PluginValue::Text(String::new()),
        },
    }
}

/// The default of `p` in its stored form.
pub fn default_of(p: &ParamInfo) -> PluginValue {
    normalize_default(p)
}

/// Every parameter's value for a slot: its own values over the plugin's defaults, each in its
/// stored form. Values for parameters the plugin doesn't have (an older version's) are left out.
pub fn values(info: &PluginInfo, own: &BTreeMap<String, PluginValue>) -> BTreeMap<String, PluginValue> {
    info.params.iter().map(|p| (p.name.clone(), own.get(&p.name).map(|v| normalize(p, v)).unwrap_or_else(|| default_of(p)))).collect()
}

/// A parameter by name (or label, any case), with "did you mean" when there is none.
pub fn param<'a>(info: &'a PluginInfo, name: &str) -> Result<&'a ParamInfo, String> {
    if let Some(p) = info.params.iter().find(|p| p.name == name).or_else(|| info.params.iter().find(|p| p.name.eq_ignore_ascii_case(name) || p.label.eq_ignore_ascii_case(name))) {
        return Ok(p);
    }
    let names: Vec<&str> = info.params.iter().map(|p| p.name.as_str()).collect();
    if names.is_empty() {
        return Err(format!("{} has no parameters.", info.name));
    }
    let hint = kimchi_core::closest(name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("{} has no parameter `{name}`.{hint} Its parameters: {}.", info.name, names.join(", ")))
}

/// A value as people read it: `12.5 px`, `on`, `#ff8800`, `(0.50, 0.25)`, the choice's label.
pub fn describe(p: &ParamInfo, v: &PluginValue) -> String {
    match (p.kind, normalize(p, v)) {
        (ParamKind::Number | ParamKind::Integer, PluginValue::Number(n)) => {
            let n = if p.kind == ParamKind::Integer || n.fract() == 0.0 { format!("{n:.0}") } else { format!("{n:.2}") };
            if p.unit.is_empty() { n } else if p.unit == "°" || p.unit == "%" { format!("{n}{}", p.unit) } else { format!("{n} {}", p.unit) }
        }
        (_, PluginValue::Bool(b)) => (if b { "on" } else { "off" }).into(),
        (ParamKind::Color, PluginValue::Vector(c)) => color_hex(&c),
        (ParamKind::Point, PluginValue::Vector(c)) => format!("({:.2}, {:.2})", c[0], c[1]),
        (_, PluginValue::Text(t)) => t,
        (_, other) => serde_json::to_string(&other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn param(kind: ParamKind) -> ParamInfo {
        ParamInfo { name: "P".into(), label: "P".into(), kind, min: 0.0, max: 10.0, choices: vec!["Squares".into(), "Dots".into()], unit: "px".into(), ..Default::default() }
    }

    #[test]
    fn values_are_checked_and_stored_in_one_form() {
        let n = param(ParamKind::Number);
        assert_eq!(from_json(&n, &json!(42)), Ok(PluginValue::Number(10.0)), "clamped");
        assert_eq!(from_json(&n, &json!("4.5 px")), Ok(PluginValue::Number(4.5)));
        assert!(from_json(&n, &json!(true)).unwrap_err().contains("a number from 0 to 10px"));
        assert_eq!(from_json(&param(ParamKind::Integer), &json!(2.6)), Ok(PluginValue::Number(3.0)));
        let c = param(ParamKind::Choice);
        assert_eq!(from_json(&c, &json!("dots")), Ok(PluginValue::Text("Dots".into())));
        assert_eq!(from_json(&c, &json!(0)), Ok(PluginValue::Text("Squares".into())));
        assert!(from_json(&c, &json!("Dost")).unwrap_err().contains("Did you mean `Dots`"));
        let col = param(ParamKind::Color);
        assert_eq!(from_json(&col, &json!("#ff0000")), Ok(PluginValue::Vector(vec![1.0, 0.0, 0.0, 1.0])));
        assert_eq!(from_json(&col, &json!([0, 1, 0])), Ok(PluginValue::Vector(vec![0.0, 1.0, 0.0, 1.0])));
        assert!(from_json(&col, &json!("red")).is_err());
        assert!(from_json(&param(ParamKind::Point), &json!([0.5])).is_err());
        assert_eq!(from_json(&param(ParamKind::Toggle), &json!("on")), Ok(PluginValue::Bool(true)));
        assert_eq!(color_hex(&[1.0, 0.5, 0.0, 1.0]), "#ff8000");
        assert_eq!(parse_color("#f80"), Some([1.0, 136.0 / 255.0, 0.0, 1.0]));
    }

    #[test]
    fn stored_values_fill_in_and_normalise() {
        let info = PluginInfo {
            name: "X".into(),
            params: vec![
                ParamInfo { name: "Size".into(), kind: ParamKind::Number, default: PluginValue::Number(3.0), max: 10.0, ..Default::default() },
                ParamInfo { name: "Shape".into(), kind: ParamKind::Choice, default: PluginValue::Number(1.0), choices: vec!["A".into(), "B".into()], ..Default::default() },
            ],
            ..Default::default()
        };
        let own = BTreeMap::from([("Size".to_string(), PluginValue::Number(50.0)), ("Gone".to_string(), PluginValue::Bool(true))]);
        let v = values(&info, &own);
        assert_eq!(v.len(), 2);
        assert_eq!(v["Size"], PluginValue::Number(10.0));
        assert_eq!(v["Shape"], PluginValue::Text("B".into()));
        assert_eq!(describe(&info.params[0], &v["Size"]), "10");
        assert!(super::param(&info, "Sise").unwrap_err().contains("Did you mean `Size`"));
    }
}
