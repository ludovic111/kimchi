//! Exact time for the formats: frame rates as rationals (NTSC's 30000/1001), times as frames at
//! a rate or as rational seconds, and SMPTE timecode (drop frame and non drop frame).
//!
//! kimchi keeps seconds as `f64` on the timeline. Everything read from a file goes through
//! [`Rate::seconds`] (frames → seconds) and everything written through [`Rate::frames`]
//! (seconds → the nearest frame), so a cut made on a frame lands on the same frame after a
//! round trip, NTSC rates included.

use std::fmt;

/// A frame rate as a fraction: 30000/1001 is NTSC's 29.97.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rate {
    pub num: i64,
    pub den: i64,
}

/// The NTSC rates and the timebase other apps write them with.
const NTSC: &[(i64, f64)] = &[(24, 24000.0 / 1001.0), (30, 30000.0 / 1001.0), (48, 48000.0 / 1001.0), (60, 60000.0 / 1001.0), (120, 120000.0 / 1001.0)];

impl Rate {
    pub const fn new(num: i64, den: i64) -> Rate {
        Rate { num, den }
    }

    /// The rate kimchi's `fps` means: NTSC rates from their rounded values (29.97, 23.976,
    /// 59.94), whole rates as they are, others to the nearest thousandth.
    pub fn from_fps(fps: f64) -> Rate {
        let fps = if fps.is_finite() && fps > 0.0 { fps } else { 30.0 };
        if let Some((base, _)) = NTSC.iter().find(|(_, v)| (fps - v).abs() < 2e-3) {
            return Rate::new(base * 1000, 1001);
        }
        if (fps - fps.round()).abs() < 1e-6 {
            return Rate::new(fps.round() as i64, 1);
        }
        Rate::new((fps * 1000.0).round() as i64, 1000).reduced()
    }

    /// A timebase and NTSC flag (Final Cut 7 XML, Premiere): 30 + ntsc is 29.97.
    pub fn from_timebase(timebase: i64, ntsc: bool) -> Rate {
        let tb = timebase.max(1);
        if ntsc { Rate::new(tb * 1000, 1001) } else { Rate::new(tb, 1) }
    }

    /// One frame's length as rational seconds (`1001/30000s` in FCPXML).
    pub fn from_frame_duration(num: i64, den: i64) -> Option<Rate> {
        (num > 0 && den > 0).then(|| Rate::new(den, num).reduced())
    }

    pub fn fps(self) -> f64 {
        self.num as f64 / self.den.max(1) as f64
    }

    /// Is it one of the NTSC rates (a 1001 denominator)?
    pub fn is_ntsc(self) -> bool {
        self.den == 1001
    }

    /// The whole number of frames a timecode counts per second (30 for 29.97).
    pub fn timebase(self) -> i64 {
        (self.fps()).round().max(1.0) as i64
    }

    /// Frames → seconds.
    pub fn seconds(self, frames: f64) -> f64 {
        frames * self.den as f64 / self.num.max(1) as f64
    }

    /// Seconds → the nearest frame.
    pub fn frames(self, seconds: f64) -> i64 {
        (seconds * self.num as f64 / self.den.max(1) as f64).round() as i64
    }

    /// Seconds snapped onto this rate's frames.
    pub fn snap(self, seconds: f64) -> f64 {
        self.seconds(self.frames(seconds) as f64)
    }

    fn reduced(self) -> Rate {
        let g = gcd(self.num.abs(), self.den.abs()).max(1);
        Rate::new(self.num / g, self.den / g)
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 { write!(f, "{}", self.num) } else { write!(f, "{}/{}", self.num, self.den) }
    }
}

pub fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Rational seconds as FCPXML writes them: `"1001/30000s"`, `"5s"`, `"0s"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RationalSeconds {
    pub num: i64,
    pub den: i64,
}

impl RationalSeconds {
    /// `frames` frames at `rate`: frames × den / num seconds.
    pub fn of_frames(frames: i64, rate: Rate) -> RationalSeconds {
        RationalSeconds { num: frames * rate.den, den: rate.num }.reduced()
    }

    pub fn seconds(self) -> f64 {
        self.num as f64 / self.den.max(1) as f64
    }

    /// `"3003/30000s"` or `"5s"`; also takes plain numbers of seconds ("2.5").
    pub fn parse(s: &str) -> Option<RationalSeconds> {
        let s = s.trim();
        let s = s.strip_suffix('s').unwrap_or(s).trim();
        if s.is_empty() {
            return None;
        }
        if let Some((n, d)) = s.split_once('/') {
            let (n, d) = (n.trim().parse::<i64>().ok()?, d.trim().parse::<i64>().ok()?);
            return (d != 0).then_some(RationalSeconds { num: n, den: d });
        }
        if let Ok(n) = s.parse::<i64>() {
            return Some(RationalSeconds { num: n, den: 1 });
        }
        // Decimal seconds (some tools write "1.5s"): kept to the microsecond.
        let v: f64 = s.parse().ok().filter(|v: &f64| v.is_finite())?;
        Some(RationalSeconds { num: (v * 1_000_000.0).round() as i64, den: 1_000_000 }.reduced())
    }

    fn reduced(self) -> RationalSeconds {
        if self.num == 0 {
            return RationalSeconds { num: 0, den: 1 };
        }
        let g = gcd(self.num.abs(), self.den.abs()).max(1);
        let (n, d) = (self.num / g, self.den / g);
        if d < 0 { RationalSeconds { num: -n, den: -d } } else { RationalSeconds { num: n, den: d } }
    }
}

impl fmt::Display for RationalSeconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = self.reduced();
        if r.den == 1 { write!(f, "{}s", r.num) } else { write!(f, "{}/{}s", r.num, r.den) }
    }
}

/// FCPXML-style rational seconds for a kimchi time at `rate`, on the nearest frame.
pub fn rational(seconds: f64, rate: Rate) -> String {
    RationalSeconds::of_frames(rate.frames(seconds), rate).to_string()
}

/// Seconds of a rational time string, or `None` when it doesn't read.
pub fn parse_rational(s: &str) -> Option<f64> {
    RationalSeconds::parse(s).map(RationalSeconds::seconds)
}

// ---------------------------------------------------------------------------------------------
// Timecode

/// SMPTE timecode `HH:MM:SS:FF` (`;` before the frames for drop frame) for a frame count.
/// Drop frame (29.97 and 59.94 only) skips frame numbers 0 and 1 (0–3 at 59.94) at the start of
/// every minute except each tenth, so the clock keeps up with real time.
pub fn timecode(frames: i64, rate: Rate, drop: bool) -> String {
    let fps = rate.timebase();
    let drop = drop && rate.is_ntsc() && fps % 30 == 0;
    let neg = frames < 0;
    let mut f = frames.abs();
    if drop {
        let dropped = fps / 15; // 2 at 29.97, 4 at 59.94
        let per_10min = fps * 600 - dropped * 9;
        let per_min = fps * 60 - dropped;
        let tens = f / per_10min;
        let rest = f % per_10min;
        f += dropped * 9 * tens;
        if rest > dropped {
            f += dropped * ((rest - dropped) / per_min);
        }
    }
    let (h, m, s, ff) = (f / (fps * 3600), f / (fps * 60) % 60, f / fps % 60, f % fps);
    format!("{}{h:02}:{m:02}:{s:02}{}{ff:02}", if neg { "-" } else { "" }, if drop { ';' } else { ':' })
}

/// Frames from SMPTE timecode (`01:00:00:00`, `00:59:59;29`, with `.` or `,` also accepted
/// before the frames). `drop` is used when the separators don't say (`;` always means drop
/// frame).
pub fn parse_timecode(tc: &str, rate: Rate, drop: bool) -> Option<i64> {
    let tc = tc.trim();
    let drop = (drop || tc.contains(';')) && rate.is_ntsc() && rate.timebase() % 30 == 0;
    let parts: Vec<&str> = tc.split([':', ';', '.', ',']).collect();
    let [h, m, s, f] = parts[..] else { return None };
    let (h, m, s, f) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?, s.parse::<i64>().ok()?, f.parse::<i64>().ok()?);
    if !(0..60).contains(&m) || !(0..60).contains(&s) || f < 0 || h < 0 {
        return None;
    }
    let fps = rate.timebase();
    let mut frames = ((h * 60 + m) * 60 + s) * fps + f;
    if drop {
        let dropped = fps / 15;
        let minutes = h * 60 + m;
        frames -= dropped * (minutes - minutes / 10);
    }
    Some(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_from_fps() {
        assert_eq!(Rate::from_fps(29.97), Rate::new(30000, 1001));
        assert_eq!(Rate::from_fps(30000.0 / 1001.0), Rate::new(30000, 1001));
        assert_eq!(Rate::from_fps(23.976), Rate::new(24000, 1001));
        assert_eq!(Rate::from_fps(25.0), Rate::new(25, 1));
        assert_eq!(Rate::from_fps(12.5), Rate::new(25, 2));
        assert_eq!(Rate::from_timebase(30, true), Rate::new(30000, 1001));
        assert_eq!(Rate::from_frame_duration(1001, 30000), Some(Rate::new(30000, 1001)));
        assert_eq!(Rate::from_frame_duration(100, 2500), Some(Rate::new(25, 1)));
    }

    #[test]
    fn frames_round_trip_at_ntsc() {
        let r = Rate::new(30000, 1001);
        for f in [0, 1, 29, 30, 1799, 107892, 1_000_003] {
            assert_eq!(r.frames(r.seconds(f as f64)), f);
        }
        assert_eq!(rational(r.seconds(1.0), r), "1001/30000s");
        assert_eq!(rational(2.0, Rate::new(25, 1)), "2s");
        assert_eq!(parse_rational("1001/30000s"), Some(1001.0 / 30000.0));
        assert_eq!(parse_rational("3600s"), Some(3600.0));
        assert_eq!(parse_rational("1.5s"), Some(1.5));
        assert_eq!(parse_rational("x"), None);
        assert_eq!(parse_rational("1/0s"), None);
    }

    #[test]
    fn drop_frame_timecode() {
        let r = Rate::new(30000, 1001);
        // The first frame of minute 1 is 00:01:00;02 in drop frame.
        assert_eq!(timecode(1800, r, true), "00:01:00;02");
        assert_eq!(timecode(17982, r, true), "00:10:00;00");
        assert_eq!(timecode(107892, r, true), "01:00:00;00");
        assert_eq!(timecode(108000, r, false), "01:00:00:00");
        for f in [0, 1799, 1800, 1801, 17981, 17982, 107892, 123456] {
            assert_eq!(parse_timecode(&timecode(f, r, true), r, true), Some(f), "{f}");
        }
        assert_eq!(parse_timecode("01:00:00:00", Rate::new(25, 1), false), Some(90000));
        assert_eq!(parse_timecode("01:00:00;00", r, false), Some(107892));
        let r60 = Rate::new(60000, 1001);
        for f in [0, 3599, 3600, 3604, 35964, 215784] {
            assert_eq!(parse_timecode(&timecode(f, r60, true), r60, true), Some(f), "{f}");
        }
        assert_eq!(parse_timecode("garbage", r, false), None);
        assert_eq!(parse_timecode("00:61:00:00", r, false), None);
    }
}
