//! Number formats the inspector and the media panel share (as the Svelte UI of kimchi 0.1 wrote them).

use chrono::{DateTime, Utc};

/// Compact human duration: `4.2s`, `12s`, `1:05`.
pub fn short(t: f64) -> String {
    if t < 60.0 {
        return if t < 10.0 { format!("{t:.1}s") } else { format!("{}s", t.round()) };
    }
    let m = (t / 60.0).floor();
    format!("{m}:{:02}", (t % 60.0).round() as u64)
}

/// `mm:ss.ff` (or `h:mm:ss.ff`) with frames.
pub fn timecode(t: f64, fps: f64) -> String {
    let t = t.max(0.0);
    let frames = ((t % 1.0) * fps.max(1.0) + 1e-6).floor() as u64;
    let s = t.floor() as u64 % 60;
    let m = (t / 60.0).floor() as u64 % 60;
    let h = (t / 3600.0).floor() as u64;
    if h > 0 { format!("{h}:{m:02}:{s:02}.{frames:02}") } else { format!("{m:02}:{s:02}.{frames:02}") }
}

/// `840 B`, `3.2 MB`, `120 MB`.
pub fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let units = ["KB", "MB", "GB"];
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", units[i]) } else { format!("{v:.0} {}", units[i]) }
}

/// `just now`, `5 min ago`, `3 h ago`, `2 d ago`, else the date.
pub fn ago(when: DateTime<Utc>) -> String {
    let s = (Utc::now() - when).num_seconds();
    match s {
        ..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        86400..604800 => format!("{} d ago", s / 86400),
        _ => when.format("%b %-d").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(short(4.24), "4.2s");
        assert_eq!(short(42.4), "42s");
        assert_eq!(short(65.0), "1:05");
        assert_eq!(timecode(65.5, 30.0), "01:05.15");
        assert_eq!(timecode(3725.0, 25.0), "1:02:05.00");
        assert_eq!(bytes(800), "800 B");
        assert_eq!(bytes(3 * 1024 * 1024 + 300_000), "3.3 MB");
        assert_eq!(bytes(120 * 1024 * 1024), "120 MB");
    }
}
