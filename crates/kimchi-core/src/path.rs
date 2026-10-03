//! SVG path data (`d`) → absolute segments: moves, lines, quadratic and cubic curves.
//! Every command of SVG 1.1 is understood (relative forms, H/V, smooth S/T, arcs as cubics).

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Move([f64; 2]),
    Line([f64; 2]),
    Quad([f64; 2], [f64; 2]),
    Cubic([f64; 2], [f64; 2], [f64; 2]),
    Close,
}

/// Parses path data; an error names what went wrong and where.
pub fn parse(d: &str) -> Result<Vec<Seg>, String> {
    let mut p = Parser { s: d.as_bytes(), i: 0 };
    let mut out = vec![];
    let mut cur = [0.0, 0.0];
    let mut start = [0.0, 0.0];
    // Last control point, for S/T reflections.
    let mut last_cubic: Option<[f64; 2]> = None;
    let mut last_quad: Option<[f64; 2]> = None;
    let mut cmd: Option<u8> = None;
    loop {
        p.skip_ws();
        if p.i >= p.s.len() {
            break;
        }
        let c = p.s[p.i];
        if c.is_ascii_alphabetic() {
            cmd = Some(c);
            p.i += 1;
        } else if cmd.is_none() {
            return Err(format!("expected a command (M, L, C…) at character {}", p.i + 1));
        }
        let c = cmd.expect("set above");
        let rel = c.is_ascii_lowercase();
        let base = if rel { cur } else { [0.0, 0.0] };
        let at = |x: f64, y: f64| [base[0] + x, base[1] + y];
        match c.to_ascii_uppercase() {
            b'M' => {
                let (x, y) = (p.num()?, p.num()?);
                cur = at(x, y);
                start = cur;
                out.push(Seg::Move(cur));
                // Further pairs after a move are lines.
                cmd = Some(if rel { b'l' } else { b'L' });
                last_cubic = None;
                last_quad = None;
                continue;
            }
            b'L' => {
                let (x, y) = (p.num()?, p.num()?);
                cur = at(x, y);
                out.push(Seg::Line(cur));
                last_cubic = None;
                last_quad = None;
            }
            b'H' => {
                let x = p.num()?;
                cur = [if rel { cur[0] + x } else { x }, cur[1]];
                out.push(Seg::Line(cur));
                last_cubic = None;
                last_quad = None;
            }
            b'V' => {
                let y = p.num()?;
                cur = [cur[0], if rel { cur[1] + y } else { y }];
                out.push(Seg::Line(cur));
                last_cubic = None;
                last_quad = None;
            }
            b'C' => {
                let c1 = at(p.num()?, p.num()?);
                let c2 = at(p.num()?, p.num()?);
                cur = at(p.num()?, p.num()?);
                out.push(Seg::Cubic(c1, c2, cur));
                last_cubic = Some(c2);
                last_quad = None;
            }
            b'S' => {
                let c1 = last_cubic.map_or(cur, |c| [2.0 * cur[0] - c[0], 2.0 * cur[1] - c[1]]);
                let c2 = at(p.num()?, p.num()?);
                cur = at(p.num()?, p.num()?);
                out.push(Seg::Cubic(c1, c2, cur));
                last_cubic = Some(c2);
                last_quad = None;
            }
            b'Q' => {
                let c1 = at(p.num()?, p.num()?);
                cur = at(p.num()?, p.num()?);
                out.push(Seg::Quad(c1, cur));
                last_quad = Some(c1);
                last_cubic = None;
            }
            b'T' => {
                let c1 = last_quad.map_or(cur, |c| [2.0 * cur[0] - c[0], 2.0 * cur[1] - c[1]]);
                cur = at(p.num()?, p.num()?);
                out.push(Seg::Quad(c1, cur));
                last_quad = Some(c1);
                last_cubic = None;
            }
            b'A' => {
                let (rx, ry, rot) = (p.num()?, p.num()?, p.num()?);
                let (large, sweep) = (p.flag()?, p.flag()?);
                let to = at(p.num()?, p.num()?);
                arc(cur, rx, ry, rot, large, sweep, to, &mut out);
                cur = to;
                last_cubic = None;
                last_quad = None;
            }
            b'Z' => {
                out.push(Seg::Close);
                cur = start;
                last_cubic = None;
                last_quad = None;
                // Z takes no numbers: a number after it is an error unless a command follows.
                cmd = None;
            }
            other => return Err(format!("unknown command `{}`", other as char)),
        }
    }
    if out.is_empty() {
        return Err("no segments".into());
    }
    if !matches!(out[0], Seg::Move(_)) {
        return Err("path data must start with M".into());
    }
    Ok(out)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn num(&mut self) -> Result<f64, String> {
        self.skip_ws();
        let start = self.i;
        let s = self.s;
        if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
            self.i += 1;
        }
        let mut dot = false;
        let mut digits = false;
        while self.i < s.len() {
            match s[self.i] {
                b'0'..=b'9' => {
                    digits = true;
                    self.i += 1;
                }
                b'.' if !dot => {
                    dot = true;
                    self.i += 1;
                }
                b'e' | b'E' if digits => {
                    self.i += 1;
                    if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
                        self.i += 1;
                    }
                    while self.i < s.len() && s[self.i].is_ascii_digit() {
                        self.i += 1;
                    }
                    break;
                }
                _ => break,
            }
        }
        if !digits {
            return Err(format!("expected a number at character {}", start + 1));
        }
        std::str::from_utf8(&s[start..self.i]).ok().and_then(|t| t.parse().ok()).ok_or_else(|| format!("bad number at character {}", start + 1))
    }

    fn flag(&mut self) -> Result<bool, String> {
        self.skip_ws();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(format!("expected an arc flag (0 or 1) at character {}", self.i + 1)),
        }
    }
}

/// SVG elliptical arc → cubic Béziers (endpoint to centre parameterisation, then ≤ 90° pieces).
#[allow(clippy::too_many_arguments)]
fn arc(from: [f64; 2], rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool, to: [f64; 2], out: &mut Vec<Seg>) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-9 || ry < 1e-9 || (from[0] - to[0]).abs() + (from[1] - to[1]).abs() < 1e-9 {
        out.push(Seg::Line(to));
        return;
    }
    let phi = rot_deg.to_radians();
    let (cos, sin) = (phi.cos(), phi.sin());
    let (dx, dy) = ((from[0] - to[0]) / 2.0, (from[1] - to[1]) / 2.0);
    let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = (num / den).max(0.0).sqrt();
    if large == sweep {
        co = -co;
    }
    let (cx1, cy1) = (co * rx * y1 / ry, -co * ry * x1 / rx);
    let (cx, cy) = (cos * cx1 - sin * cy1 + (from[0] + to[0]) / 2.0, sin * cx1 + cos * cy1 + (from[1] + to[1]) / 2.0);
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vx + uy * vy) / ((ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt());
        let a = a.clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let th1 = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dth = angle((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dth > 0.0 {
        dth -= std::f64::consts::TAU;
    } else if sweep && dth < 0.0 {
        dth += std::f64::consts::TAU;
    }
    let n = (dth.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = dth / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f64| {
        let (x, y) = (rx * t.cos(), ry * t.sin());
        [cos * x - sin * y + cx, sin * x + cos * y + cy]
    };
    let deriv = |t: f64| {
        let (x, y) = (-rx * t.sin(), ry * t.cos());
        [cos * x - sin * y, sin * x + cos * y]
    };
    for i in 0..n {
        let (a, b) = (th1 + step * i as f64, th1 + step * (i + 1) as f64);
        let (p0, p3) = (point(a), point(b));
        let (d0, d3) = (deriv(a), deriv(b));
        let c1 = [p0[0] + k * d0[0], p0[1] + k * d0[1]];
        let c2 = [p3[0] - k * d3[0], p3[1] - k * d3[1]];
        let end = if i + 1 == n { to } else { p3 };
        out.push(Seg::Cubic(c1, c2, end));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command() {
        let s = parse("M10 10 h 20 v20 H10 Z m 5,5 l5-5 C 0 0 1 1 2 2 S 4 4 5 5 Q 6 6 7 7 T 9 9 A 5 5 0 0 1 19 9").unwrap();
        assert_eq!(s[0], Seg::Move([10.0, 10.0]));
        assert_eq!(s[1], Seg::Line([30.0, 10.0]));
        assert_eq!(s[2], Seg::Line([30.0, 30.0]));
        assert_eq!(s[4], Seg::Close);
        assert_eq!(s[5], Seg::Move([15.0, 15.0]));
        assert_eq!(s[6], Seg::Line([20.0, 10.0]));
        // Smooth cubic reflects the previous control point.
        assert_eq!(s[8], Seg::Cubic([3.0, 3.0], [4.0, 4.0], [5.0, 5.0]));
        assert_eq!(s[10], Seg::Quad([8.0, 8.0], [9.0, 9.0]));
        let Seg::Cubic(_, _, end) = s.last().unwrap() else { panic!("arc as cubic") };
        assert_eq!(*end, [19.0, 9.0]);
        // Implicit lines after a move, and exponent/compact numbers.
        assert_eq!(parse("M0 0 10 10").unwrap()[1], Seg::Line([10.0, 10.0]));
        assert_eq!(parse("M1e1-.5").unwrap()[0], Seg::Move([10.0, -0.5]));
        assert!(parse("L 1 1").is_err());
        assert!(parse("M 0 0 Q 1").is_err());
        assert!(parse("").is_err());
    }
}
