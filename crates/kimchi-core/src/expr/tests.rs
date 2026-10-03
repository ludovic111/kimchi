use std::cell::Cell;

use super::*;
use crate::anim::{Easing, Keyframe};

fn at(time: f64) -> Fixed {
    Fixed::at(time)
}

fn run(src: &str, ctx: &Fixed) -> KeyValue {
    eval_str(src, ctx).unwrap_or_else(|e| panic!("{src}: {e}"))
}

fn n(src: &str) -> f64 {
    n_at(src, &at(0.0))
}

fn n_at(src: &str, ctx: &Fixed) -> f64 {
    match run(src, ctx) {
        KeyValue::Number(x) => x,
        other => panic!("{src} gave {other:?}"),
    }
}

fn v(src: &str, ctx: &Fixed) -> Vec<f64> {
    match run(src, ctx) {
        KeyValue::Vector(x) => x,
        other => panic!("{src} gave {other:?}"),
    }
}

fn s(src: &str) -> String {
    match run(src, &at(0.0)) {
        KeyValue::Text(x) => x,
        other => panic!("{src} gave {other:?}"),
    }
}

fn err(src: &str) -> String {
    match compile(src) {
        Err(e) => e,
        Ok(p) => eval(&p, &at(0.0)).expect_err(src),
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn precedence_and_operators() {
    assert_eq!(n("1 + 2 * 3"), 7.0);
    assert_eq!(n("(1 + 2) * 3"), 9.0);
    assert_eq!(n("2 ** 3 ** 2"), 512.0, "right to left");
    assert_eq!(n("2 ^ 3"), 8.0);
    assert_eq!(n("-2 ** 2"), -4.0);
    assert_eq!(n("2 ** -1"), 0.5);
    assert_eq!(n("10 - 4 - 3"), 3.0);
    assert_eq!(n("7 % 3"), 1.0);
    assert_eq!(n("-7 % 3"), -1.0, "JavaScript's remainder");
    assert_eq!(n("mod(-7, 3)"), 2.0);
    assert_eq!(n("1 < 2 && 3 > 2 ? 10 : 20"), 10.0);
    assert_eq!(n("false || 5"), 5.0);
    assert_eq!(n("0 && 5"), 0.0);
    assert_eq!(n("!0 ? 1 : 2"), 1.0);
    assert_eq!(n("1 == 1 ? 1 : 0"), 1.0);
    assert_eq!(n("1 === 2 ? 1 : 0"), 0.0);
    assert_eq!(n("\"a\" != \"b\" ? 1 : 0"), 1.0);
    assert_eq!(n("true ? 1 : 0 ? 2 : 3"), 1.0);
    assert_eq!(n("let a = 2; let b = a * 3; a + b"), 8.0);
    assert_eq!(n("x = 4; x = x * 2; x"), 8.0);
    assert_eq!(n("1.5e2 + .5"), 150.5);
    assert_eq!(n("Math.max(1, 4) + Math.PI - pi"), 4.0);
    assert_eq!(n("/* a */ 3 // b"), 3.0);
    assert_eq!(n("3;"), 3.0);
}

#[test]
fn vectors() {
    let c = at(0.0);
    assert_eq!(v("[1, 2] + [3, 4]", &c), vec![4.0, 6.0]);
    assert_eq!(v("[1, 2] * 3", &c), vec![3.0, 6.0]);
    assert_eq!(v("2 * [1, 2, 3]", &c), vec![2.0, 4.0, 6.0]);
    assert_eq!(v("[1, 2] + [1, 1, 1]", &c), vec![2.0, 3.0, 1.0], "shorter one padded with 0");
    assert_eq!(v("-[1, 2]", &c), vec![-1.0, -2.0]);
    assert_eq!(v("[[1, 2], 3]", &c), vec![1.0, 2.0, 3.0]);
    assert_eq!(v("abs([-1, 2])", &c), vec![1.0, 2.0]);
    assert_eq!(n("[4, 5, 6].z"), 6.0);
    assert_eq!(n("[4, 5, 6][1]"), 5.0);
    assert_eq!(n("let p = [3, 4]; length(p)"), 5.0);
    assert_eq!(n("distance([0, 0, 0], [0, 3, 4])"), 5.0);
    assert_eq!(n("length([1, 1], [4, 5])"), 5.0);
    assert_eq!(v("normalize([0, 0, 2])", &c), vec![0.0, 0.0, 1.0]);
    assert_eq!(n("dot([1, 2, 3], [4, 5, 6])"), 32.0);
    assert_eq!(v("cross([1, 0, 0], [0, 1, 0])", &c), vec![0.0, 0.0, 1.0]);
    assert_eq!(v("clamp([5, -5], 0, 1)", &c), vec![1.0, 0.0]);
    assert_eq!(v("max([1, 5], [3, 2])", &c), vec![3.0, 5.0]);
    assert_eq!(n("max([1, 9, 3])"), 9.0);
    assert_eq!(n("min(4, 2, 8)"), 2.0);
    assert_eq!(n("[1, 2] == [1, 2] ? 1 : 0"), 1.0);
    assert!(err("[1, 2] * [1, 2, 3]").contains("2 and 3"));
    assert!(err("[1, 2].z").contains("has 2 numbers"));
    assert!(err("[1, 2] < 3").contains("compares numbers"));
}

#[test]
fn math_functions() {
    let pi = std::f64::consts::PI;
    assert!(close(n("sin(pi / 2)"), 1.0));
    assert!(close(n("cos(0)"), 1.0));
    assert!(close(n("tan(pi / 4)"), 1.0));
    assert!(close(n("asin(1)"), pi / 2.0));
    assert!(close(n("acos(1)"), 0.0));
    assert!(close(n("atan(1)"), pi / 4.0));
    assert!(close(n("atan2(1, 0)"), pi / 2.0));
    assert_eq!(n("sqrt(16)"), 4.0);
    assert_eq!(n("pow(2, 10)"), 1024.0);
    assert!(close(n("exp(1)"), std::f64::consts::E));
    assert!(close(n("log(e)"), 1.0));
    assert_eq!(n("abs(-3)"), 3.0);
    assert_eq!(n("sign(-3) + sign(0) + sign(2)"), 0.0);
    assert_eq!(n("floor(1.7) + ceil(1.2) + trunc(-1.5)"), 2.0);
    assert!(close(n("fract(2.25)"), 0.25));
    assert_eq!(n("round(2.5)"), 3.0);
    assert_eq!(n("round(1.2345, 2)"), 1.23);
    assert!(close(n("deg(pi)"), 180.0));
    assert!(close(n("rad(180)"), pi));
    assert!(close(n("radiansToDegrees(degreesToRadians(30))"), 30.0));
    assert_eq!(n("clamp(5, 0, 1)"), 1.0);
    assert_eq!(n("smoothstep(0, 1, 0.5)"), 0.5);
    assert_eq!(n("step(0.5, 0.7)"), 1.0);
    assert_eq!(n("mix(10, 20, 0.25)"), 12.5);
    assert_eq!(n("lerp(10, 20, 2)"), 30.0, "mix isn't clamped");
    assert!(err("sqrt(-1)").contains("NaN"));
    assert!(err("1 / 0").contains("Infinity"));
    assert!(err("mod(1, 0)").contains("divide by 0"));
}

#[test]
fn mapping_and_easing() {
    assert_eq!(n("linear(0.5, 0, 1, 0, 100)"), 50.0);
    assert_eq!(n("linear(5, 0, 1, 0, 100)"), 100.0, "clamped");
    assert_eq!(n("linear(-1, 0, 1, 0, 100)"), 0.0);
    assert_eq!(n("linear(0.25, 10, 20)"), 12.5);
    assert_eq!(n("linear(1.5, 2, 1, 0, 100)"), 50.0, "reversed range");
    assert_eq!(n("ease(0.5, 0, 1, 0, 100)"), 50.0);
    assert!(close(n("ease(0.25, 0, 1, 0, 1)"), 0.15625));
    assert!(n("easeIn(0.25, 0, 1, 0, 1)") < 0.25 && n("easeOut(0.25, 0, 1, 0, 1)") > 0.25);
    assert_eq!(n("easeIn(1, 0, 1, 0, 1)"), 1.0);
    assert_eq!(n("easeOut(1, 0, 1, 0, 1)"), 1.0);
    assert_eq!(v("linear(0.5, [0, 0], [10, 20])", &at(0.0)), vec![5.0, 10.0]);
    assert_eq!(s("linear(0.5, \"#000000\", \"#ffffff\")"), "#808080");
    assert!(err("linear(1, 2, 3, 4)").contains("not 4"));
}

#[test]
fn colours_and_text() {
    assert_eq!(s("rgb(255, 90, 54)"), "#ff5a36");
    assert_eq!(s("rgba(255, 0, 0, 0.5)"), "#ff000080");
    assert_eq!(s("hsl(0, 1, 0.5)"), "#ff0000");
    assert_eq!(s("hsl(120, 100, 50)"), "#00ff00", "percent too");
    assert_eq!(s("hsla(240, 1, 0.5, 0)"), "#0000ff00");
    assert_eq!(s("hex([1, 0.5, 0])"), "#ff8000");
    assert_eq!(s("hex(\"#F53\")"), "#ff5533");
    assert_eq!(v("hexToRgb(\"#ff0000\")", &at(0.0)), vec![1.0, 0.0, 0.0, 1.0]);
    assert_eq!(n("\"#ff0000\".r"), 1.0);
    assert_eq!(s("mix(\"#000000\", \"#ffffff\", 1)"), "#ffffff");
    assert_eq!(s("\"Score \" + 3"), "Score 3");
    assert_eq!(s("\"x\" + 0.1 + [1, 2]"), "x0.1[1, 2]");
    assert_eq!(s("(2 / 3).toFixed(2)"), "0.67");
    assert_eq!(s("toFixed(1234.5)"), "1235");
    assert_eq!(n("true + 1"), 2.0);
}

#[test]
fn time_value_and_names() {
    let mut c = at(2.0);
    c.value = Some(KeyValue::Vector(vec![10.0, 20.0]));
    c.index = 3.0;
    c.fps = 25.0;
    c.duration = 6.0;
    assert_eq!(n_at("time + t", &c), 4.0);
    assert_eq!(v("value + [0, 5]", &c), vec![10.0, 25.0]);
    assert_eq!(n_at("value.y", &c), 20.0);
    assert_eq!(n_at("index", &c), 3.0);
    assert_eq!(n_at("frame", &c), 50.0);
    assert_eq!(n_at("fps + duration", &c), 31.0);
    assert_eq!(n_at("timeToFrames()", &c), 50.0);
    assert_eq!(n_at("framesToTime(50)", &c), 2.0);
    let mut c = at(1.3);
    c.fps = 30.0;
    assert_eq!(n_at("posterizeTime(2); time", &c), 1.0);
    assert!(err("value").contains("no value"));
}

#[test]
fn errors_explain_and_point() {
    let e = err("wigle(2, 30)");
    assert!(e.contains("unknown function `wigle`") && e.contains("Did you mean `wiggle`?") && e.contains("column 1"), "{e}");
    let e = err("value + tiem");
    assert!(e.contains("unknown name `tiem`") && e.contains("Did you mean `time`?") && e.contains("column 9"), "{e}");
    let e = err("1 +\n  * 2");
    assert!(e.contains("line 2, column 3"), "{e}");
    assert!(err("sin(1, 2)").contains("takes 1 argument"));
    assert!(err("wiggle(1)").contains("2 to 5 arguments"));
    assert!(err("(1 + 2").contains("`)`"));
    assert!(err("1 2").contains("`;`"));
    assert!(err("").contains("empty"));
    assert!(err("let a = 2").contains("ends with `let"));
    assert!(err("if (time > 1) { 1 }").contains("unknown function `if`") || err("if (time > 1) { 1 }").contains("no blocks"));
    assert!(err("{ 1 }").contains("no blocks"));
    assert!(err("wiggle").contains("is a function"));
    assert!(err("\"abc").contains("isn't closed"));
    assert!(err("time(2)").contains("is a value, not a function"));
    assert!(err("1 @ 2").contains("unexpected `@`"));
    assert!(err("loopOut(\"cycel\")").contains("Did you mean \"cycle\"?"));
    assert!(err("[1, 2].q").contains(".x, .y"));
    let deep = format!("{}1{}", "(".repeat(200), ")".repeat(200));
    assert!(err(&deep).contains("nests too deeply"));
    assert!(err(&"1+".repeat(5000)).contains("too long"));
    let chain = format!("1{}", "+1".repeat(600));
    assert!(err(&chain).contains("more than 256 operations"), "a long chain is refused before it runs");
    assert_eq!(n(&format!("1{}", "+1".repeat(200))), 201.0);
    assert!(check("time * 90").is_ok());
    assert!(check("wiggle(2, 30) + loopOut('pingpong')").is_ok());
}

#[test]
fn random_and_wiggle_are_deterministic() {
    let mut c = at(1.0);
    c.seed = 42;
    let a = n_at("random()", &c);
    assert_eq!(a, n_at("random()", &c), "same frame, same value");
    assert!((0.0..1.0).contains(&a));
    let two = v("[random(), random()]", &c);
    assert_ne!(two[0], two[1], "successive draws differ");
    let mut other = c.clone();
    other.seed = 43;
    assert_ne!(a, n_at("random()", &other), "another property, another value");
    let mut later = c.clone();
    later.time = 1.5;
    assert_ne!(a, n_at("random()", &later), "changes over time");
    assert_eq!(n_at("seedRandom(5, true); random()", &c), n_at("seedRandom(5, true); random()", &later), "timeless");
    assert_ne!(n_at("seedRandom(5); random()", &c), a, "another seed");
    let r = n_at("random(10, 20)", &c);
    assert!((10.0..20.0).contains(&r));
    let r = v("random([10, 100])", &c);
    assert!(r.len() == 2 && r[0] < 10.0 && r[1] < 100.0);
    let g: Vec<f64> = (0..400)
        .map(|i| {
            let mut c = at(i as f64 / 30.0);
            c.seed = 7;
            n_at("gaussRandom()", &c)
        })
        .collect();
    let mean = g.iter().sum::<f64>() / g.len() as f64;
    assert!((mean - 0.5).abs() < 0.05, "{mean}");
    // wiggle: around value, smooth, the same each time, per axis.
    c.value = Some(KeyValue::Vector(vec![100.0, 100.0]));
    let w = v("wiggle(2, 30)", &c);
    assert_eq!(w, v("wiggle(2, 30)", &c));
    assert!(w.iter().all(|x| (x - 100.0).abs() <= 30.0 * 1.0 + 1e-9));
    assert_ne!(w[0], w[1], "axes move apart");
    let mut prev: Option<Vec<f64>> = None;
    let mut moved = 0.0f64;
    for i in 0..300 {
        let mut k = c.clone();
        k.time = i as f64 / 100.0;
        let w = v("wiggle(2, 30)", &k);
        if let Some(p) = prev {
            assert!((w[0] - p[0]).abs() < 3.0, "smooth");
            moved = moved.max((w[0] - 100.0).abs());
        }
        prev = Some(w);
    }
    assert!(moved > 5.0, "actually wiggles: {moved}");
    c.value = Some(KeyValue::Number(5.0));
    assert_eq!(n_at("wiggle(0, 10)", &c), n_at("wiggle(0, 10)", &c));
    c.value = Some(KeyValue::from("#808080"));
    assert!(matches!(run("wiggle(3, 0.2)", &c), KeyValue::Text(t) if t.starts_with('#')));
    assert!(err("noise()").contains("takes 1 to 3"));
    let n1 = n("noise(0.5)");
    assert!(n1 != 0.0 && n1.abs() <= 1.0 && n1 == n("noise(0.5)"));
    assert!(n("noise(0.5, 0.25, 0.1)").abs() <= 1.0);
    assert_eq!(n("noise([0.5, 0.25])"), n("noise(0.5, 0.25)"));
}

fn keyed(time: f64) -> Fixed {
    let mut c = at(time);
    c.keys = vec![Keyframe::new(0.0, 0.0, Easing::Linear), Keyframe::new(1.0, 10.0, Easing::Linear), Keyframe::new(2.0, 30.0, Easing::Linear)];
    c
}

#[test]
fn loops_and_keyframes() {
    // cycle: 0 → 10 → 30, then again from 0
    assert_eq!(n_at("loopOut()", &keyed(1.5)), 20.0, "before the end it is just the value");
    assert_eq!(n_at("loopOut()", &keyed(2.5)), 5.0);
    assert_eq!(n_at("loopOut(\"cycle\")", &keyed(4.5)), 5.0);
    assert_eq!(n_at("loopOut(\"pingpong\")", &keyed(2.5)), 20.0, "comes back");
    assert_eq!(n_at("loopOut(\"pingpong\")", &keyed(4.5)), 5.0, "and goes again");
    assert_eq!(n_at("loopOut(\"offset\")", &keyed(2.5)), 35.0, "adds a round");
    assert_eq!(n_at("loopOut(\"offset\")", &keyed(4.5)), 65.0);
    assert!(close(n_at("loopOut(\"continue\")", &keyed(3.0)), 50.0), "keeps the last speed");
    assert_eq!(n_at("loopOut(\"cycle\", 1)", &keyed(2.5)), 20.0, "only the last segment");
    assert_eq!(n_at("loopIn()", &keyed(-0.5)), 20.0);
    assert_eq!(n_at("loopIn(\"offset\")", &keyed(-0.5)), -10.0);
    assert!(close(n_at("loopIn(\"continue\")", &keyed(-1.0)), -10.0));
    assert_eq!(n_at("valueAtTime(0.5)", &keyed(0.0)), 5.0);
    assert!(close(n_at("velocity", &keyed(1.5)), 20.0));
    assert!(close(n_at("velocityAtTime(0.5)", &keyed(1.5)), 10.0));
    assert!(close(n_at("speed", &keyed(1.5)), 20.0));
    assert_eq!(n_at("numKeys", &keyed(0.0)), 3.0);
    assert_eq!(n_at("keyTime(2) + keyValue(3)", &keyed(0.0)), 31.0);
    assert_eq!(n_at("key(2).time + key(2).value", &keyed(0.0)), 11.0);
    assert!(eval_str("keyTime(4)", &keyed(0.0)).unwrap_err().contains("3 keyframes"));
    // vectors loop too
    let mut c = at(2.5);
    c.keys = vec![Keyframe::new(0.0, [0.0, 0.0], Easing::Linear), Keyframe::new(2.0, [10.0, 20.0], Easing::Linear)];
    assert_eq!(v("loopOut(\"offset\")", &c), vec![12.5, 25.0]);
    // a still property doesn't loop
    let mut c = at(5.0);
    c.value = Some(KeyValue::Number(3.0));
    assert_eq!(n_at("loopOut()", &c), 3.0);
}

/// prop() through a context that counts its calls and resolves formulas of other "things".
struct Scene {
    calls: Cell<u32>,
}

impl Context for Scene {
    fn time(&self) -> f64 {
        1.0
    }
    fn value(&self) -> Option<KeyValue> {
        Some(KeyValue::Number(0.0))
    }
    fn value_at(&self, _t: f64) -> Option<KeyValue> {
        self.value()
    }
    fn keys(&self) -> &[Keyframe] {
        &[]
    }
    fn prop(&self, id: &str, name: &str, t: Option<f64>) -> Result<KeyValue, String> {
        self.calls.set(self.calls.get() + 1);
        match (id, name) {
            ("ball", "x") => Ok(KeyValue::Number(100.0 * t.unwrap_or(1.0))),
            _ => Err(format!("no \"{id}\"")),
        }
    }
    fn index(&self) -> f64 {
        1.0
    }
    fn fps(&self) -> f64 {
        30.0
    }
    fn duration(&self) -> f64 {
        5.0
    }
    fn seed(&self) -> u64 {
        1
    }
}

#[test]
fn reads_other_things() {
    let s = Scene { calls: Cell::new(0) };
    assert_eq!(eval_str("prop(\"ball\", \"x\") + 1", &s).unwrap(), KeyValue::Number(101.0));
    assert_eq!(eval_str("prop(\"ball\", \"x\", 0.5)", &s).unwrap(), KeyValue::Number(50.0));
    assert_eq!(eval_str("posterizeTime(1); prop('ball', 'x')", &s).unwrap(), KeyValue::Number(100.0));
    let e = eval_str("prop(\"cat\", \"x\")", &s).unwrap_err();
    assert!(e.contains("prop(\"cat\", \"x\"): no \"cat\""), "{e}");
    assert!(eval_str("prop(1, \"x\")", &s).unwrap_err().contains("text in quotes"));
}

#[test]
fn compiled_once() {
    let a = compile("time * 3 + 1").unwrap();
    let b = compile("time * 3 + 1").unwrap();
    assert!(Arc::ptr_eq(&a, &b), "cached by text");
    assert_eq!(a.source(), "time * 3 + 1");
    // Lots of formulas don't grow the cache forever.
    for i in 0..(CACHE_SIZE + 10) {
        compile(&format!("{i} + time")).unwrap();
    }
    let c = CACHE.get().unwrap().lock().unwrap();
    assert!(c.map.len() <= CACHE_SIZE);
}

#[test]
fn guide_examples_read() {
    let block = GUIDE.split("```js\n").nth(1).unwrap().split("```").next().unwrap();
    let mut count = 0;
    for line in block.lines().filter(|l| !l.trim().is_empty()) {
        let src = line.split("  //").next().unwrap().trim();
        check(src).unwrap_or_else(|e| panic!("guide example `{src}`: {e}"));
        count += 1;
    }
    assert!(count >= 10, "{count} examples");
}
