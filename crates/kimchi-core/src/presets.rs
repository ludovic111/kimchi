//! Ready-made clip animations ("pop in", "Ken Burns"…) written as ordinary keyframes, so they
//! can be tweaked afterwards like any others.

use crate::anim::{Easing, Keyframe, Keyframes, set_key};
use crate::model::Clip;

pub struct Preset {
    pub id: &'static str,
    /// `"in"` (at the start), `"out"` (at the end) or `"whole"` (over the clip).
    pub at: &'static str,
    pub doc: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "fadeIn", at: "in", doc: "Opacity 0 → 1." },
    Preset { id: "riseIn", at: "in", doc: "Comes up a little while fading in." },
    Preset { id: "slideInLeft", at: "in", doc: "Slides in from off the left edge." },
    Preset { id: "slideInRight", at: "in", doc: "Slides in from off the right edge." },
    Preset { id: "slideInUp", at: "in", doc: "Slides up from below the frame." },
    Preset { id: "slideInDown", at: "in", doc: "Slides down from above the frame." },
    Preset { id: "popIn", at: "in", doc: "Scales up from 50% with a little overshoot." },
    Preset { id: "zoomIn", at: "in", doc: "Settles from 130% while fading in." },
    Preset { id: "spinIn", at: "in", doc: "Spins half a turn while growing from nothing." },
    Preset { id: "dropIn", at: "in", doc: "Falls from above and bounces." },
    Preset { id: "blurIn", at: "in", doc: "Comes into focus while fading in." },
    Preset { id: "fadeOut", at: "out", doc: "Opacity 1 → 0." },
    Preset { id: "sinkOut", at: "out", doc: "Goes down a little while fading out." },
    Preset { id: "slideOutLeft", at: "out", doc: "Slides off the left edge." },
    Preset { id: "slideOutRight", at: "out", doc: "Slides off the right edge." },
    Preset { id: "slideOutUp", at: "out", doc: "Slides off the top." },
    Preset { id: "slideOutDown", at: "out", doc: "Slides off the bottom." },
    Preset { id: "popOut", at: "out", doc: "Overshoots then shrinks away." },
    Preset { id: "zoomOut", at: "out", doc: "Grows to 130% while fading out." },
    Preset { id: "spinOut", at: "out", doc: "Spins half a turn while shrinking to nothing." },
    Preset { id: "blurOut", at: "out", doc: "Goes out of focus while fading out." },
    Preset { id: "kenBurns", at: "whole", doc: "Slow push in (100% → 115%) with a slight drift." },
    Preset { id: "kenBurnsOut", at: "whole", doc: "Slow pull out (115% → 100%)." },
    Preset { id: "panLeft", at: "whole", doc: "Pans across at 115% towards the left." },
    Preset { id: "panRight", at: "whole", doc: "Pans across at 115% towards the right." },
    Preset { id: "pulse", at: "whole", doc: "Gentle beat: 100% → 106% → 100%, every `length` seconds." },
    Preset { id: "float", at: "whole", doc: "Bobs up and down." },
    Preset { id: "wiggle", at: "whole", doc: "Small rocking rotation." },
    Preset { id: "shake", at: "whole", doc: "Quick horizontal shake (an impact), over `length`." },
    Preset { id: "spin", at: "whole", doc: "Full turns, one every `length` seconds." },
];

/// The keyframes `preset` adds to `clip` on a `canvas` (project width, height). `length` is how
/// long the move takes (default 0.6 s for in/out, a cycle for repeating ones).
pub fn apply(preset: &str, clip: &Clip, canvas: (f64, f64), length: Option<f64>) -> Result<Keyframes, String> {
    let Some(p) = PRESETS.iter().find(|p| p.id.eq_ignore_ascii_case(preset)) else {
        let ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        let hint = crate::closest(preset, &ids).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
        return Err(format!("Unknown animation preset `{preset}`.{hint} Presets: {}.", ids.join(", ")));
    };
    let d = clip.duration.max(0.05);
    let len = length.filter(|l| *l > 0.0).unwrap_or(match p.at {
        "whole" => match p.id {
            "pulse" => 1.0,
            "float" => 3.0,
            "wiggle" => 1.2,
            "shake" => 0.5,
            "spin" => 4.0,
            _ => d,
        },
        _ => 0.6,
    });
    let len = len.min(d);
    let tf = &clip.transform;
    let (w, h) = canvas;
    let mut k = clip.keyframes.clone();
    let out_start = (d - len).max(0.0);
    let key = |k: &mut Keyframes, name: &str, t: f64, v: f64, e: Easing| set_key(k, name, Keyframe::new(t, v, e));
    // The values to come from or go to: what the clip shows just after an in-move, just before
    // an out-move, or at the start for the whole-clip ones (existing keyframes respected).
    let at = match p.at {
        "in" => len,
        "out" => out_start,
        _ => 0.0,
    };
    let base = |k: &Keyframes, name: &str, fallback: f64| crate::anim::number_at(k, name, at).unwrap_or(fallback);
    let (x0, y0, s0, r0, o0) =
        (base(&k, "x", tf.x), base(&k, "y", tf.y), base(&k, "scale", tf.scale), base(&k, "rotation", tf.rotation), base(&k, "opacity", tf.opacity));
    let out = Easing::EASE_OUT;
    let inn = Easing::EASE_IN;
    let in_move = |k: &mut Keyframes, name: &str, from: f64, to: f64, e: Easing| {
        key(k, name, 0.0, from, Easing::Linear);
        key(k, name, len, to, e);
    };
    let out_move = |k: &mut Keyframes, name: &str, from: f64, to: f64, e: Easing| {
        key(k, name, out_start, from, Easing::Linear);
        key(k, name, d, to, e);
    };
    match p.id {
        "fadeIn" => in_move(&mut k, "opacity", 0.0, o0, Easing::Linear),
        "riseIn" => {
            in_move(&mut k, "opacity", 0.0, o0, out);
            in_move(&mut k, "y", y0 + h * 0.06, y0, out);
        }
        "slideInLeft" => in_move(&mut k, "x", x0 - w * (0.5 + s0 * 0.5), x0, out),
        "slideInRight" => in_move(&mut k, "x", x0 + w * (0.5 + s0 * 0.5), x0, out),
        "slideInUp" => in_move(&mut k, "y", y0 + h * (0.5 + s0 * 0.5), y0, out),
        "slideInDown" => in_move(&mut k, "y", y0 - h * (0.5 + s0 * 0.5), y0, out),
        "popIn" => {
            in_move(&mut k, "scale", s0 * 0.5, s0, Easing::parse("easeOutBack").expect("named"));
            in_move(&mut k, "opacity", 0.0, o0, out);
        }
        "zoomIn" => {
            in_move(&mut k, "scale", s0 * 1.3, s0, out);
            in_move(&mut k, "opacity", 0.0, o0, out);
        }
        "spinIn" => {
            in_move(&mut k, "rotation", r0 - 180.0, r0, out);
            in_move(&mut k, "scale", 0.0, s0, out);
        }
        "dropIn" => in_move(&mut k, "y", y0 - h * (0.5 + s0 * 0.5), y0, Easing::parse("easeOutBounce").expect("named")),
        "blurIn" => {
            in_move(&mut k, "blur", 40.0, 0.0, out);
            in_move(&mut k, "opacity", 0.0, o0, out);
        }
        "fadeOut" => out_move(&mut k, "opacity", o0, 0.0, Easing::Linear),
        "sinkOut" => {
            out_move(&mut k, "opacity", o0, 0.0, inn);
            out_move(&mut k, "y", y0, y0 + h * 0.06, inn);
        }
        "slideOutLeft" => out_move(&mut k, "x", x0, x0 - w * (0.5 + s0 * 0.5), inn),
        "slideOutRight" => out_move(&mut k, "x", x0, x0 + w * (0.5 + s0 * 0.5), inn),
        "slideOutUp" => out_move(&mut k, "y", y0, y0 - h * (0.5 + s0 * 0.5), inn),
        "slideOutDown" => out_move(&mut k, "y", y0, y0 + h * (0.5 + s0 * 0.5), inn),
        "popOut" => {
            out_move(&mut k, "scale", s0, 0.0, Easing::parse("easeInBack").expect("named"));
            out_move(&mut k, "opacity", o0, 0.0, inn);
        }
        "zoomOut" => {
            out_move(&mut k, "scale", s0, s0 * 1.3, inn);
            out_move(&mut k, "opacity", o0, 0.0, inn);
        }
        "spinOut" => {
            out_move(&mut k, "rotation", r0, r0 + 180.0, inn);
            out_move(&mut k, "scale", s0, 0.0, inn);
        }
        "blurOut" => {
            out_move(&mut k, "blur", 0.0, 40.0, inn);
            out_move(&mut k, "opacity", o0, 0.0, inn);
        }
        "kenBurns" | "kenBurnsOut" => {
            let (a, b) = if p.id == "kenBurns" { (s0, s0 * 1.15) } else { (s0 * 1.15, s0) };
            let smooth = Easing::parse("easeInOutSine").expect("named");
            key(&mut k, "scale", 0.0, a, Easing::Linear);
            key(&mut k, "scale", d, b, smooth);
            key(&mut k, "x", 0.0, x0 - w * 0.015, Easing::Linear);
            key(&mut k, "x", d, x0 + w * 0.015, smooth);
        }
        "panLeft" | "panRight" => {
            let shift = w * 0.06 * s0;
            let (a, b) = if p.id == "panLeft" { (x0 + shift, x0 - shift) } else { (x0 - shift, x0 + shift) };
            key(&mut k, "scale", 0.0, s0 * 1.15, Easing::Linear);
            key(&mut k, "x", 0.0, a, Easing::Linear);
            key(&mut k, "x", d, b, Easing::parse("easeInOutSine").expect("named"));
        }
        "pulse" => cycles(&mut k, "scale", d, len, &[(0.0, s0), (0.5, s0 * 1.06), (1.0, s0)]),
        "float" => cycles(&mut k, "y", d, len, &[(0.0, y0), (0.5, y0 - h * 0.012), (1.0, y0)]),
        "wiggle" => cycles(&mut k, "rotation", d, len, &[(0.0, r0), (0.25, r0 + 3.0), (0.75, r0 - 3.0), (1.0, r0)]),
        "shake" => {
            let a = w * 0.01;
            let steps = [0.0, a, -a, a * 0.7, -a * 0.7, a * 0.4, -a * 0.2, 0.0];
            for (i, dx) in steps.iter().enumerate() {
                key(&mut k, "x", len * i as f64 / (steps.len() - 1) as f64, x0 + dx, Easing::parse("easeInOutSine").expect("named"));
            }
        }
        "spin" => {
            key(&mut k, "rotation", 0.0, r0, Easing::Linear);
            key(&mut k, "rotation", d, r0 + 360.0 * d / len, Easing::Linear);
        }
        _ => unreachable!("every preset is handled"),
    }
    Ok(k)
}

/// Repeats a little curve (`(fraction of a cycle, value)`) every `len` seconds over `d` seconds.
fn cycles(k: &mut Keyframes, name: &str, d: f64, len: f64, shape: &[(f64, f64)]) {
    let smooth = Easing::parse("easeInOutSine").expect("named");
    let n = (d / len).floor().max(1.0) as usize;
    for c in 0..n {
        for (f, v) in shape {
            if c > 0 && *f == 0.0 {
                continue;
            }
            set_key(k, name, Keyframe::new((c as f64 + f) * len, *v, if c == 0 && *f == 0.0 { Easing::Linear } else { smooth }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ClipContent;

    fn clip() -> Clip {
        Clip::new("t", 0.0, 4.0, ClipContent::Solid { color: "#ffffff".into() })
    }

    #[test]
    fn in_and_out_combine_on_one_property() {
        let mut c = clip();
        c.keyframes = apply("fadeIn", &c, (1920.0, 1080.0), None).unwrap();
        c.keyframes = apply("fadeOut", &c, (1920.0, 1080.0), Some(1.0)).unwrap();
        let o = &c.keyframes["opacity"];
        assert_eq!(o.iter().map(|k| (k.time, k.value.as_f64().unwrap())).collect::<Vec<_>>(), vec![(0.0, 0.0), (0.6, 1.0), (3.0, 1.0), (4.0, 0.0)]);
        assert_eq!(c.placement_at(2.0).opacity, 1.0);
        assert!(c.placement_at(3.5).opacity < 1.0);
    }

    #[test]
    fn every_preset_applies() {
        for p in PRESETS {
            let k = apply(p.id, &clip(), (1920.0, 1080.0), None).unwrap();
            assert!(!k.is_empty(), "{}", p.id);
        }
        assert!(apply("slideFromLeft", &clip(), (1.0, 1.0), None).unwrap_err().contains("Did you mean"));
        let c = clip();
        let k = apply("slideInLeft", &c, (1920.0, 1080.0), None).unwrap();
        assert_eq!(k["x"][0].value.as_f64(), Some(-1920.0));
        let pulse = apply("pulse", &c, (1920.0, 1080.0), None).unwrap();
        assert_eq!(pulse["scale"].len(), 9, "four beats of three keys sharing ends");
    }
}
