//! Motion templates: a lower third, a title card, a counter, a 3D title… Each one turns a few
//! values (texts, colours, numbers) into an ordinary [`Scene`], so the result can be edited like
//! any other scene, or re-made with new values (the clip remembers its template and values).

use serde_json::{Map, Value, json};

use crate::motion::Scene;

pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub doc: &'static str,
    /// `"2d"` or `"3d"`.
    pub kind: &'static str,
    /// Seconds, when the clip's length isn't given.
    pub duration: f64,
    pub params: &'static [Param],
    build: fn(&Values, &Ctx) -> Value,
}

pub struct Param {
    pub name: &'static str,
    /// `"text"`, `"number"`, `"color"`, `"colorOrNone"`, `"list"`, `"media"`, `"choice:a|b"` or `"boolean"`.
    pub kind: &'static str,
    /// JSON.
    pub default: &'static str,
    pub doc: &'static str,
}

/// Canvas size and clip length a template lays itself out for.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    pub width: f64,
    pub height: f64,
    pub duration: f64,
}

pub fn find(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id.eq_ignore_ascii_case(id))
}

pub fn ids() -> Vec<&'static str> {
    TEMPLATES.iter().map(|t| t.id).collect()
}

impl Template {
    /// The values with every default filled in; unknown names and wrong types are errors.
    pub fn values(&self, given: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let mut out = Map::new();
        for (k, v) in given {
            let Some(p) = self.params.iter().find(|p| p.name == k) else {
                let names: Vec<&str> = self.params.iter().map(|p| p.name).collect();
                let hint = crate::closest(k, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
                return Err(format!("The {} template has no value `{k}`.{hint} Values: {}.", self.id, names.join(", ")));
            };
            check(p, v).map_err(|e| format!("{}.{}: {e}", self.id, k))?;
            out.insert(k.clone(), v.clone());
        }
        for p in self.params {
            if !out.contains_key(p.name) {
                out.insert(p.name.to_string(), serde_json::from_str(p.default).expect("template defaults are JSON"));
            }
        }
        Ok(out)
    }

    /// Builds the scene for `given` values on a canvas.
    pub fn build(&self, given: &Map<String, Value>, ctx: Ctx) -> Result<Scene, String> {
        let values = Values(self.values(given)?);
        let json = (self.build)(&values, &ctx);
        Scene::from_json(&json).map_err(|e| format!("template {} made a bad scene (a bug): {e}", self.id))
    }

    /// For listings: id, name, doc, kind, duration and the values it takes.
    pub fn describe(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "doc": self.doc,
            "kind": self.kind,
            "duration": self.duration,
            "values": self.params.iter().map(|p| json!({
                "name": p.name,
                "type": p.kind,
                "default": serde_json::from_str::<Value>(p.default).unwrap_or(Value::Null),
                "doc": p.doc,
            })).collect::<Vec<_>>(),
        })
    }
}

fn check(p: &Param, v: &Value) -> Result<(), String> {
    let ok = match p.kind {
        "text" | "media" => v.is_string() || (p.kind == "media" && v.is_null()),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "color" => v.as_str().is_some_and(|c| crate::anim::Rgba::parse(c).is_some()),
        "colorOrNone" => v.is_null() || v.as_str().is_some_and(|c| crate::anim::Rgba::parse(c).is_some()),
        "list" => v.is_array(),
        k if k.starts_with("choice:") => v.as_str().is_some_and(|s| k["choice:".len()..].split('|').any(|c| c == s)),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(match p.kind {
            "color" => "takes a colour like \"#ff5a36\"".into(),
            "colorOrNone" => "takes a colour like \"#ff5a36\", or null for none".into(),
            k if k.starts_with("choice:") => format!("is one of {}", k["choice:".len()..].replace('|', ", ")),
            k => format!("takes a {k}"),
        })
    }
}

struct Values(Map<String, Value>);

impl Values {
    fn s(&self, k: &str) -> String {
        self.0.get(k).and_then(Value::as_str).unwrap_or("").to_string()
    }
    fn n(&self, k: &str) -> f64 {
        self.0.get(k).and_then(Value::as_f64).unwrap_or(0.0)
    }
    fn b(&self, k: &str) -> bool {
        self.0.get(k).and_then(Value::as_bool).unwrap_or(false)
    }
    fn opt(&self, k: &str) -> Option<String> {
        self.0.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
    }
    fn list(&self, k: &str) -> Vec<Value> {
        self.0.get(k).and_then(Value::as_array).cloned().unwrap_or_default()
    }
}

/// Rough width of `text` at `size` in Manrope (for plates and underlines sized to the words).
fn text_width(text: &str, size: f64, weight: f64) -> f64 {
    let per = if weight >= 700.0 { 0.6 } else { 0.56 };
    text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * size * per
}

/// `[time, value, easing]` keyframe.
fn k(t: f64, v: impl Into<Value>, e: &str) -> Value {
    json!([round(t), v.into(), e])
}

fn round(t: f64) -> f64 {
    (t * 1000.0).round() / 1000.0
}

pub static TEMPLATES: &[Template] = &[
    Template {
        id: "lowerThird",
        name: "Lower third",
        doc: "Name and role in the lower corner: an accent bar grows, the name rises letter by letter over a plate, then everything slides out at the end.",
        kind: "2d",
        duration: 5.0,
        params: &[
            Param { name: "title", kind: "text", default: "\"Ada Lovelace\"", doc: "The name." },
            Param { name: "subtitle", kind: "text", default: "\"Mathematician\"", doc: "The role or place; empty for none." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Bar colour." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Text colour." },
            Param { name: "plate", kind: "colorOrNone", default: "\"#0b0b0fcc\"", doc: "Plate behind the text; null for none." },
            Param { name: "side", kind: "choice:left|right", default: "\"left\"", doc: "Which corner." },
        ],
        build: lower_third,
    },
    Template {
        id: "titleCard",
        name: "Title card",
        doc: "A big title whose words rise in, an underline that draws itself, a spaced-out subtitle, and a gentle fade out.",
        kind: "2d",
        duration: 4.0,
        params: &[
            Param { name: "title", kind: "text", default: "\"The Long Way Home\"", doc: "Title." },
            Param { name: "subtitle", kind: "text", default: "\"A FILM BY KIMCHI\"", doc: "Line under the title; empty for none." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Underline colour." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Text colour." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0e0e12\"", doc: "Card colour; null to put the title over what's below." },
        ],
        build: title_card,
    },
    Template {
        id: "kineticType",
        name: "Kinetic type",
        doc: "Words punch in one after another, big and centred. Wrap a word in *asterisks* to colour it with the accent.",
        kind: "2d",
        duration: 4.0,
        params: &[
            Param { name: "text", kind: "text", default: "\"Make *every* frame count\"", doc: "The words; *word* = accent." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Text colour." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Colour of *marked* words." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0e0e12\"", doc: "Background; null for none." },
        ],
        build: kinetic_type,
    },
    Template {
        id: "counter",
        name: "Counter",
        doc: "A number counts up inside a ring that fills, with a label underneath.",
        kind: "2d",
        duration: 4.0,
        params: &[
            Param { name: "from", kind: "number", default: "0", doc: "Start value." },
            Param { name: "to", kind: "number", default: "1000000", doc: "End value." },
            Param { name: "decimals", kind: "number", default: "0", doc: "Digits after the point." },
            Param { name: "prefix", kind: "text", default: "\"\"", doc: "Before the number, e.g. \"$\"." },
            Param { name: "suffix", kind: "text", default: "\"\"", doc: "After the number, e.g. \"%\"." },
            Param { name: "label", kind: "text", default: "\"VIEWS\"", doc: "Under the number; empty for none." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Ring colour." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Text colour." },
            Param { name: "ring", kind: "boolean", default: "true", doc: "Draw the ring." },
            Param { name: "background", kind: "colorOrNone", default: "null", doc: "Background; null for none." },
        ],
        build: counter,
    },
    Template {
        id: "barChart",
        name: "Bar chart",
        doc: "Bars grow one after another with their values counting up, labels underneath and a title.",
        kind: "2d",
        duration: 5.0,
        params: &[
            Param { name: "title", kind: "text", default: "\"Monthly visitors\"", doc: "Chart title; empty for none." },
            Param { name: "values", kind: "list", default: "[32, 45, 41, 60, 72, 95]", doc: "Bar values." },
            Param { name: "labels", kind: "list", default: "[\"Jan\", \"Feb\", \"Mar\", \"Apr\", \"May\", \"Jun\"]", doc: "One label per bar." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Bar colour." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Text colour." },
            Param { name: "suffix", kind: "text", default: "\"k\"", doc: "After each value." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0e0e12\"", doc: "Background; null for none." },
        ],
        build: bar_chart,
    },
    Template {
        id: "logoReveal",
        name: "Logo reveal",
        doc: "A burst ring, then the logo (a picture or a word) pops in with a glow and settles.",
        kind: "2d",
        duration: 3.0,
        params: &[
            Param { name: "text", kind: "text", default: "\"kimchi\"", doc: "The word mark (used when there is no image)." },
            Param { name: "image", kind: "media", default: "null", doc: "A logo picture: media id, name or file path." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Ring and glow colour." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Word mark colour." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0e0e12\"", doc: "Background; null for none." },
        ],
        build: logo_reveal,
    },
    Template {
        id: "callout",
        name: "Callout",
        doc: "Points at something in the shot: a pulsing dot at the target, a line that draws out to a label.",
        kind: "2d",
        duration: 4.0,
        params: &[
            Param { name: "label", kind: "text", default: "\"Look here\"", doc: "The label." },
            Param { name: "target", kind: "list", default: "[-200, 120]", doc: "[x, y] of the point, from the canvas centre in project pixels." },
            Param { name: "offset", kind: "list", default: "[320, -220]", doc: "[x, y] from the target to the label." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Dot and line colour." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Label colour." },
        ],
        build: callout,
    },
    Template {
        id: "quote",
        name: "Quote",
        doc: "A quotation appears word by word under a big quote mark, then its author.",
        kind: "2d",
        duration: 6.0,
        params: &[
            Param { name: "quote", kind: "text", default: "\"Simplicity is the ultimate sophistication.\"", doc: "The words." },
            Param { name: "author", kind: "text", default: "\"Leonardo da Vinci\"", doc: "Who said it; empty for none." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Quote mark colour." },
            Param { name: "textColor", kind: "color", default: "\"#ffffff\"", doc: "Text colour." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0e0e12\"", doc: "Background; null for none." },
        ],
        build: quote,
    },
    Template {
        id: "subscribe",
        name: "Subscribe button",
        doc: "A subscribe button pops up, a cursor clicks it and it turns into \"Subscribed\".",
        kind: "2d",
        duration: 3.5,
        params: &[
            Param { name: "label", kind: "text", default: "\"SUBSCRIBE\"", doc: "Button text." },
            Param { name: "done", kind: "text", default: "\"SUBSCRIBED\"", doc: "Text after the click." },
            Param { name: "accent", kind: "color", default: "\"#ff3b30\"", doc: "Button colour." },
            Param { name: "position", kind: "choice:bottom|center|top", default: "\"bottom\"", doc: "Where on screen." },
        ],
        build: subscribe,
    },
    Template {
        id: "aurora",
        name: "Aurora background",
        doc: "Soft blurred colour blobs drifting slowly: a moving backdrop for titles.",
        kind: "2d",
        duration: 8.0,
        params: &[
            Param { name: "colors", kind: "list", default: "[\"#ff5a36\", \"#7a3cff\", \"#00b3a4\"]", doc: "Blob colours." },
            Param { name: "background", kind: "color", default: "\"#0b0b10\"", doc: "Base colour." },
        ],
        build: aurora,
    },
    Template {
        id: "wipe",
        name: "Wipe transition",
        doc: "A full-screen wipe to hide a cut: two colour bands sweep across (slide), or a circle grows and shrinks (circle). Put it on a track above the cut, centred on it.",
        kind: "2d",
        duration: 1.2,
        params: &[
            Param { name: "style", kind: "choice:slide|circle", default: "\"slide\"", doc: "Shape of the wipe." },
            Param { name: "color", kind: "color", default: "\"#ff5a36\"", doc: "Front colour." },
            Param { name: "second", kind: "color", default: "\"#0e0e12\"", doc: "Second colour." },
            Param { name: "direction", kind: "choice:left|right|up|down", default: "\"right\"", doc: "Slide direction." },
        ],
        build: wipe,
    },
    Template {
        id: "title3d",
        name: "3D title",
        doc: "Extruded 3D letters swing into place while the camera pushes in, lit by a key, a fill and a rim light.",
        kind: "3d",
        duration: 4.0,
        params: &[
            Param { name: "text", kind: "text", default: "\"KIMCHI\"", doc: "The words." },
            Param { name: "color", kind: "color", default: "\"#ff5a36\"", doc: "Letter colour." },
            Param { name: "metallic", kind: "number", default: "0.2", doc: "0 = paint, 1 = metal." },
            Param { name: "depth", kind: "number", default: "0.35", doc: "Thickness of the letters." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0e0e12\"", doc: "Background; null for none." },
            Param { name: "floor", kind: "boolean", default: "true", doc: "A floor that catches the shadow." },
        ],
        build: title_3d,
    },
    Template {
        id: "logoSpin3d",
        name: "3D logo spin",
        doc: "A 3D word (or a picture on a card) spins a full turn inside a glowing ring.",
        kind: "3d",
        duration: 4.0,
        params: &[
            Param { name: "text", kind: "text", default: "\"K\"", doc: "The word, when there is no image." },
            Param { name: "image", kind: "media", default: "null", doc: "A logo picture: media id, name or file path." },
            Param { name: "color", kind: "color", default: "\"#ffffff\"", doc: "Letter colour." },
            Param { name: "accent", kind: "color", default: "\"#ff5a36\"", doc: "Ring colour." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0e0e12\"", doc: "Background; null for none." },
        ],
        build: logo_spin_3d,
    },
    Template {
        id: "turntable",
        name: "Turntable",
        doc: "A model turns on a pedestal while the camera looks down at it: a product shot. Without a model, a stack of shapes.",
        kind: "3d",
        duration: 6.0,
        params: &[
            Param { name: "model", kind: "media", default: "null", doc: "A glTF/GLB file path (or media item)." },
            Param { name: "color", kind: "color", default: "\"#ff5a36\"", doc: "Colour of the placeholder shapes." },
            Param { name: "pedestal", kind: "color", default: "\"#2a2a30\"", doc: "Pedestal colour." },
            Param { name: "background", kind: "colorOrNone", default: "\"#16161c\"", doc: "Background; null for none." },
        ],
        build: turntable,
    },
    Template {
        id: "shapes3d",
        name: "Floating shapes",
        doc: "Glossy spheres, rings and boxes floating and turning slowly: an abstract 3D backdrop.",
        kind: "3d",
        duration: 8.0,
        params: &[
            Param { name: "colors", kind: "list", default: "[\"#ff5a36\", \"#ffffff\", \"#7a3cff\", \"#00b3a4\"]", doc: "Shape colours." },
            Param { name: "background", kind: "colorOrNone", default: "\"#0b0b10\"", doc: "Background; null for none." },
        ],
        build: shapes_3d,
    },
];

// ---------------------------------------------------------------------------------------------
// 2D templates

fn lower_third(v: &Values, c: &Ctx) -> Value {
    let (w, h, d) = (c.width, c.height, c.duration);
    let s = h / 1080.0;
    let right = v.s("side") == "right";
    let (title, sub) = (v.s("title"), v.s("subtitle"));
    let (ts, ss) = (64.0 * s, 34.0 * s);
    let text_w = text_width(&title, ts, 800.0).max(text_width(&sub, ss, 500.0));
    let plate_w = text_w + 96.0 * s;
    let plate_h = if sub.is_empty() { 120.0 * s } else { 150.0 * s };
    let margin = 110.0 * s;
    let base_y = h / 2.0 - 190.0 * s;
    let dir = if right { -1.0 } else { 1.0 };
    let edge = if right { w / 2.0 - margin } else { -w / 2.0 + margin };
    let bar_x = edge;
    let text_x = edge + dir * 40.0 * s;
    let align = if right { "right" } else { "left" };
    let out = (d - 0.5).max(1.2);
    let mut layers = vec![];
    if let Some(plate) = v.opt("plate") {
        layers.push(json!({
            "id": "plate", "type": "rect", "width": plate_w, "height": plate_h, "radius": 6.0 * s,
            "x": edge, "y": base_y, "anchorX": -dir * plate_w / 2.0, "fill": plate,
            "keyframes": {"scaleX": [k(0.1, 0.0, "linear"), k(0.6, 1.0, "easeOutCubic")]}
        }));
    }
    layers.push(json!({
        "id": "bar", "type": "rect", "width": 8.0 * s, "height": plate_h, "x": bar_x, "y": base_y, "fill": v.s("accent"),
        "keyframes": {"scaleY": [k(0.0, 0.0, "linear"), k(0.4, 1.0, "easeOutCubic")]}
    }));
    let title_y = if sub.is_empty() { base_y } else { base_y - 24.0 * s };
    layers.push(json!({
        "id": "title", "type": "text", "text": title, "fontSize": ts, "fontWeight": 800, "align": align,
        "x": text_x, "y": title_y, "fill": v.s("textColor"), "letterSpacing": -0.5 * s,
        "reveal": {"by": "char", "style": "rise", "progress": 0, "overlap": 4},
        "keyframes": {"reveal": [k(0.25, 0.0, "linear"), k(1.0, 1.0, "easeOutCubic")]}
    }));
    if !sub.is_empty() {
        layers.push(json!({
            "id": "subtitle", "type": "text", "text": sub, "fontSize": ss, "fontWeight": 500, "align": align,
            "x": text_x, "y": base_y + 36.0 * s, "fill": v.s("textColor"),
            "keyframes": {
                "opacity": [k(0.55, 0.0, "linear"), k(0.95, 0.8, "easeOut")],
                "x": [k(0.55, text_x - dir * 24.0 * s, "linear"), k(0.95, text_x, "easeOutCubic")]
            }
        }));
    }
    json!({"layers": [{
        "id": "lowerThird", "type": "group", "layers": layers,
        "keyframes": {
            "opacity": [k(out, 1.0, "linear"), k(d, 0.0, "easeIn")],
            "x": [k(out, 0.0, "linear"), k(d, -dir * 60.0 * s, "easeInCubic")]
        }
    }]})
}

fn title_card(v: &Values, c: &Ctx) -> Value {
    let (h, d) = (c.height, c.duration);
    let s = h / 1080.0;
    let (title, sub) = (v.s("title"), v.s("subtitle"));
    let ts = 120.0 * s;
    let tw = text_width(&title, ts, 800.0).min(c.width * 0.85);
    let out = (d - 0.6).max(1.6);
    let mut layers = vec![json!({
        "id": "title", "type": "text", "text": title, "fontSize": ts, "fontWeight": 800, "fill": v.s("textColor"),
        "y": -30.0 * s, "letterSpacing": -2.0 * s,
        "reveal": {"by": "word", "style": "rise", "progress": 0, "overlap": 1.5, "distance": 60.0 * s},
        "keyframes": {"reveal": [k(0.1, 0.0, "linear"), k(1.1, 1.0, "easeOutCubic")]}
    })];
    layers.push(json!({
        "id": "underline", "type": "path", "d": format!("M{} 0 L{} 0", -tw / 2.0, tw / 2.0), "y": 60.0 * s,
        "stroke": {"color": v.s("accent"), "width": 10.0 * s, "cap": "round"}, "trimEnd": 0,
        "keyframes": {"trimEnd": [k(0.6, 0.0, "linear"), k(1.4, 1.0, "easeInOutCubic")]}
    }));
    if !sub.is_empty() {
        layers.push(json!({
            "id": "subtitle", "type": "text", "text": sub, "fontSize": 30.0 * s, "fontWeight": 600, "fill": v.s("textColor"),
            "y": 130.0 * s, "letterSpacing": 8.0 * s,
            "keyframes": {"opacity": [k(1.0, 0.0, "linear"), k(1.6, 0.75, "easeOut")], "letterSpacing": [k(1.0, 2.0 * s, "linear"), k(2.4, 8.0 * s, "easeOutCubic")]}
        }));
    }
    let mut scene = json!({"layers": [{
        "id": "card", "type": "group", "layers": layers,
        "keyframes": {"opacity": [k(out, 1.0, "linear"), k(d, 0.0, "easeIn")], "scale": [k(0.0, 1.0, "linear"), k(d, 1.04, "linear")]}
    }]});
    if let Some(bg) = v.opt("background") {
        scene["background"] = json!(bg);
    }
    scene
}

fn kinetic_type(v: &Values, c: &Ctx) -> Value {
    let (h, d) = (c.height, c.duration);
    let s = h / 1080.0;
    let words: Vec<String> = v.s("text").split_whitespace().map(str::to_string).collect();
    let n = words.len().max(1) as f64;
    let each = d / n;
    let mut layers = vec![];
    for (i, word) in words.iter().enumerate() {
        let marked = word.len() > 2 && word.starts_with('*') && word.ends_with('*');
        let text = if marked { word.trim_matches('*').to_string() } else { word.clone() };
        let size = (190.0 * s).min(c.width * 1.5 / text.chars().count().max(1) as f64);
        let t0 = i as f64 * each;
        let last = i + 1 == words.len();
        let end = if last { d } else { t0 + each };
        let mut kf = json!({
            "scale": [k(t0, 0.6, "linear"), k(t0 + 0.18, 1.0, "easeOutBack")],
            "opacity": [k(t0, 0.0, "linear"), k(t0 + 0.08, 1.0, "linear")],
            "rotation": [k(t0, if i % 2 == 0 { -6.0 } else { 6.0 }, "linear"), k(t0 + 0.2, 0.0, "easeOutCubic")]
        });
        if last {
            kf["opacity"].as_array_mut().expect("array").extend([k((d - 0.3).max(t0 + 0.2), 1.0, "linear"), k(d, 0.0, "easeIn")]);
        }
        layers.push(json!({
            "id": format!("word{}", i + 1), "type": "text", "text": text, "fontSize": size, "fontWeight": 800,
            "fill": if marked { v.s("accent") } else { v.s("textColor") }, "letterSpacing": -3.0 * s,
            "start": round(t0), "end": round(end), "keyframes": kf
        }));
    }
    let mut scene = json!({"layers": layers});
    if let Some(bg) = v.opt("background") {
        scene["background"] = json!(bg);
    }
    scene
}

fn counter(v: &Values, c: &Ctx) -> Value {
    let (h, d) = (c.height, c.duration);
    let s = h / 1080.0;
    let count_end = (d * 0.7).max(0.5);
    let r = 230.0 * s;
    let text = format!("{}{{value}}{}", v.s("prefix"), v.s("suffix"));
    // Size the number so its widest value fits inside the ring.
    let widest = crate::motion::TextLayer { value: Some(v.n("to").abs().max(v.n("from").abs())), decimals: v.n("decimals"), ..serde_json::from_value(json!({"text": text})).expect("text layer") }.shown();
    let size = (150.0 * s).min(2.0 * r * 0.78 / (widest.chars().count().max(1) as f64 * 0.62));
    let mut layers = vec![];
    if v.b("ring") {
        layers.push(json!({
            "id": "track", "type": "ellipse", "width": r * 2.0, "height": r * 2.0, "stroke": {"color": "#ffffff22", "width": 14.0 * s}
        }));
        layers.push(json!({
            "id": "ring", "type": "ellipse", "width": r * 2.0, "height": r * 2.0, "rotation": -90,
            "stroke": {"color": v.s("accent"), "width": 14.0 * s, "cap": "round"}, "trimEnd": 0,
            "keyframes": {"trimEnd": [k(0.2, 0.0, "linear"), k(count_end, 1.0, "easeOutExpo")]}
        }));
    }
    layers.push(json!({
        "id": "number", "type": "text", "text": text, "value": v.n("from"), "decimals": v.n("decimals"),
        "fontSize": size, "fontWeight": 800, "fill": v.s("textColor"), "y": if v.s("label").is_empty() { 0.0 } else { -14.0 * s },
        "keyframes": {"value": [k(0.2, v.n("from"), "linear"), k(count_end, v.n("to"), "easeOutExpo")], "scale": [k(0.0, 0.85, "linear"), k(0.5, 1.0, "easeOutBack")]}
    }));
    if !v.s("label").is_empty() {
        layers.push(json!({
            "id": "label", "type": "text", "text": v.s("label"), "fontSize": 30.0 * s, "fontWeight": 600, "fill": v.s("textColor"),
            "letterSpacing": 6.0 * s, "y": size * 0.62,
            "keyframes": {"opacity": [k(0.4, 0.0, "linear"), k(0.9, 0.7, "easeOut")]}
        }));
    }
    let mut scene = json!({"layers": [{"id": "counter", "type": "group", "layers": layers,
        "keyframes": {"opacity": [k((d - 0.4).max(count_end), 1.0, "linear"), k(d, 0.0, "easeIn")]}}]});
    if let Some(bg) = v.opt("background") {
        scene["background"] = json!(bg);
    }
    scene
}

fn bar_chart(v: &Values, c: &Ctx) -> Value {
    let (w, h, d) = (c.width, c.height, c.duration);
    let s = h / 1080.0;
    let values: Vec<f64> = v.list("values").iter().filter_map(Value::as_f64).collect();
    let labels: Vec<String> = v.list("labels").iter().map(|l| l.as_str().map(str::to_string).unwrap_or_else(|| l.to_string())).collect();
    let n = values.len().max(1) as f64;
    let max = values.iter().cloned().fold(1e-9, f64::max);
    let (cw, ch) = (w * 0.62, h * 0.48);
    let base_y = h * 0.26;
    let slot = cw / n;
    let bw = slot * 0.62;
    let mut layers = vec![];
    if !v.s("title").is_empty() {
        layers.push(json!({
            "id": "title", "type": "text", "text": v.s("title"), "fontSize": 54.0 * s, "fontWeight": 800, "fill": v.s("textColor"),
            "align": "left", "x": -cw / 2.0, "y": base_y - ch - 110.0 * s,
            "keyframes": {"opacity": [k(0.0, 0.0, "linear"), k(0.4, 1.0, "easeOut")], "y": [k(0.0, base_y - ch - 80.0 * s, "linear"), k(0.4, base_y - ch - 110.0 * s, "easeOutCubic")]}
        }));
    }
    layers.push(json!({
        "id": "axis", "type": "path", "d": format!("M{} {} L{} {}", -cw / 2.0 - 20.0 * s, base_y, cw / 2.0 + 20.0 * s, base_y),
        "stroke": {"color": "#ffffff55", "width": 3.0 * s, "cap": "round"}, "trimEnd": 0,
        "keyframes": {"trimEnd": [k(0.1, 0.0, "linear"), k(0.7, 1.0, "easeInOutCubic")]}
    }));
    let grow = (d * 0.45).clamp(0.6, 2.5);
    for (i, val) in values.iter().enumerate() {
        let bh = (val / max * ch).max(2.0);
        let x = -cw / 2.0 + slot * (i as f64 + 0.5);
        let t0 = 0.4 + i as f64 * (grow / n);
        let t1 = t0 + 0.7;
        layers.push(json!({
            "id": format!("bar{}", i + 1), "type": "rect", "width": bw, "height": bh, "radius": (bw * 0.12).min(14.0 * s),
            "x": x, "y": base_y, "anchorY": bh / 2.0, "fill": v.s("accent"),
            "keyframes": {"scaleY": [k(t0, 0.0, "linear"), k(t1, 1.0, "easeOutCubic")]}
        }));
        layers.push(json!({
            "id": format!("value{}", i + 1), "type": "text", "text": format!("{{value}}{}", v.s("suffix")), "value": 0, "decimals": if val.fract().abs() > 1e-9 { 1 } else { 0 },
            "fontSize": 34.0 * s, "fontWeight": 700, "fill": v.s("textColor"), "x": x,
            "keyframes": {
                "value": [k(t0, 0.0, "linear"), k(t1, *val, "easeOutCubic")],
                "y": [k(t0, base_y - 30.0 * s, "linear"), k(t1, base_y - bh - 30.0 * s, "easeOutCubic")],
                "opacity": [k(t0, 0.0, "linear"), k(t0 + 0.2, 1.0, "linear")]
            }
        }));
        if let Some(label) = labels.get(i) {
            layers.push(json!({
                "id": format!("label{}", i + 1), "type": "text", "text": label, "fontSize": 28.0 * s, "fontWeight": 500,
                "fill": v.s("textColor"), "x": x, "y": base_y + 40.0 * s,
                "keyframes": {"opacity": [k(t0, 0.0, "linear"), k(t0 + 0.3, 0.65, "easeOut")]}
            }));
        }
    }
    let mut scene = json!({"layers": [{"id": "chart", "type": "group", "layers": layers,
        "keyframes": {"opacity": [k((d - 0.4).max(1.0), 1.0, "linear"), k(d, 0.0, "easeIn")]}}]});
    if let Some(bg) = v.opt("background") {
        scene["background"] = json!(bg);
    }
    scene
}

fn logo_reveal(v: &Values, c: &Ctx) -> Value {
    let (h, d) = (c.height, c.duration);
    let s = h / 1080.0;
    let accent = v.s("accent");
    let mut layers = vec![
        json!({
            "id": "burst", "type": "ellipse", "width": 200.0 * s, "height": 200.0 * s,
            "stroke": {"color": accent, "width": 10.0 * s},
            "keyframes": {"scale": [k(0.0, 0.2, "linear"), k(0.9, 4.0, "easeOutCubic")], "opacity": [k(0.0, 1.0, "linear"), k(0.9, 0.0, "easeIn")], "strokeWidth": [k(0.0, 30.0 * s, "linear"), k(0.9, 2.0 * s, "easeOut")]}
        }),
        json!({
            "id": "flash", "type": "ellipse", "width": 160.0 * s, "height": 160.0 * s, "fill": accent, "blur": 30.0 * s,
            "keyframes": {"scale": [k(0.0, 0.0, "linear"), k(0.25, 2.2, "easeOutCubic"), k(0.7, 0.0, "easeInCubic")]}
        }),
    ];
    let logo = match v.opt("image") {
        Some(img) => json!({"id": "logo", "type": "image", "asset": img, "height": 300.0 * s}),
        None => json!({"id": "logo", "type": "text", "text": v.s("text"), "fontSize": 170.0 * s, "fontWeight": 800, "fill": v.s("textColor"), "letterSpacing": -4.0 * s}),
    };
    let mut logo = logo;
    logo["glow"] = json!({"color": accent, "radius": 40.0 * s, "strength": 1.0});
    logo["keyframes"] = json!({
        "scale": [k(0.2, 0.0, "linear"), k(0.75, 1.0, "easeOutBack"), k(d, 1.05, "linear")],
        "glowStrength": [k(0.4, 1.4, "linear"), k(1.5, 0.35, "easeOut")]
    });
    layers.push(logo);
    let mut scene = json!({"layers": [{"id": "reveal", "type": "group", "layers": layers,
        "keyframes": {"opacity": [k((d - 0.35).max(1.0), 1.0, "linear"), k(d, 0.0, "easeIn")]}}]});
    if let Some(bg) = v.opt("background") {
        scene["background"] = json!(bg);
    }
    scene
}

fn callout(v: &Values, c: &Ctx) -> Value {
    let (h, d) = (c.height, c.duration);
    let s = h / 1080.0;
    let pair = |key: &str, dflt: [f64; 2]| {
        let l = v.list(key);
        [l.first().and_then(Value::as_f64).unwrap_or(dflt[0]), l.get(1).and_then(Value::as_f64).unwrap_or(dflt[1])]
    };
    let t = pair("target", [-200.0, 120.0]);
    let o = pair("offset", [320.0, -220.0]);
    let elbow = [t[0] + o[0] * 0.45, t[1] + o[1]];
    let end = [t[0] + o[0], t[1] + o[1]];
    let right = o[0] >= 0.0;
    let accent = v.s("accent");
    let out = (d - 0.4).max(1.5);
    json!({"layers": [{"id": "callout", "type": "group", "layers": [
        {"id": "pulse", "type": "ellipse", "width": 40.0 * s, "height": 40.0 * s, "x": t[0], "y": t[1], "stroke": {"color": accent, "width": 4.0 * s},
         "keyframes": {"scale": [k(0.3, 1.0, "linear"), k(1.3, 3.0, "easeOut"), k(1.31, 1.0, "hold"), k(2.3, 3.0, "easeOut"), k(2.31, 1.0, "hold"), k(3.3, 3.0, "easeOut")],
                       "opacity": [k(0.3, 1.0, "linear"), k(1.3, 0.0, "easeOut"), k(1.31, 1.0, "hold"), k(2.3, 0.0, "easeOut"), k(2.31, 1.0, "hold"), k(3.3, 0.0, "easeOut")]}},
        {"id": "dot", "type": "ellipse", "width": 22.0 * s, "height": 22.0 * s, "x": t[0], "y": t[1], "fill": accent,
         "keyframes": {"scale": [k(0.0, 0.0, "linear"), k(0.35, 1.0, "easeOutBack")]}},
        {"id": "line", "type": "path", "d": format!("M{} {} L{} {} L{} {}", t[0], t[1], elbow[0], elbow[1], end[0], end[1]),
         "stroke": {"color": accent, "width": 4.0 * s, "cap": "round", "join": "round"}, "trimEnd": 0,
         "keyframes": {"trimEnd": [k(0.3, 0.0, "linear"), k(0.9, 1.0, "easeInOutCubic")]}},
        {"id": "label", "type": "text", "text": v.s("label"), "fontSize": 44.0 * s, "fontWeight": 700, "fill": v.s("textColor"),
         "align": if right { "left" } else { "right" }, "x": end[0] + if right { 18.0 * s } else { -18.0 * s }, "y": end[1],
         "shadow": {"color": "#00000099", "blur": 10.0 * s, "y": 3.0 * s},
         "reveal": {"by": "char", "style": "fade", "progress": 0, "overlap": 4},
         "keyframes": {"reveal": [k(0.8, 0.0, "linear"), k(1.4, 1.0, "linear")]}}
    ], "keyframes": {"opacity": [k(out, 1.0, "linear"), k(d, 0.0, "easeIn")]}}]})
}

fn quote(v: &Values, c: &Ctx) -> Value {
    let (w, h, d) = (c.width, c.height, c.duration);
    let s = h / 1080.0;
    let q = v.s("quote");
    // Break the quote into lines of about 28 characters.
    let mut lines: Vec<String> = vec![];
    for word in q.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + word.chars().count() < 28 => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    let text = lines.join("\n");
    let size = (78.0 * s).min(w * 0.8 / 28.0 / 0.55);
    let n_lines = lines.len().max(1) as f64;
    let words = q.split_whitespace().count().max(1) as f64;
    let reveal_end = (0.4 + words * 0.12).min(d * 0.6);
    let block_h = n_lines * size * 1.2;
    let mut layers = vec![
        json!({"id": "mark", "type": "text", "text": "\u{201C}", "fontFamily": "Instrument Serif", "fontSize": 300.0 * s, "fill": v.s("accent"),
               "y": -block_h / 2.0 - 70.0 * s,
               "keyframes": {"opacity": [k(0.0, 0.0, "linear"), k(0.4, 1.0, "easeOut")], "scale": [k(0.0, 0.6, "linear"), k(0.5, 1.0, "easeOutBack")]}}),
        json!({"id": "quote", "type": "text", "text": text, "fontSize": size, "fontWeight": 600, "fill": v.s("textColor"), "lineHeight": 1.2,
               "reveal": {"by": "word", "style": "fade", "progress": 0, "overlap": 2},
               "keyframes": {"reveal": [k(0.3, 0.0, "linear"), k(reveal_end, 1.0, "linear")]}}),
    ];
    if !v.s("author").is_empty() {
        layers.push(json!({"id": "author", "type": "text", "text": format!("— {}", v.s("author")), "fontSize": 36.0 * s, "fontWeight": 500,
            "fill": v.s("textColor"), "y": block_h / 2.0 + 70.0 * s,
            "keyframes": {"opacity": [k(reveal_end, 0.0, "linear"), k(reveal_end + 0.5, 0.7, "easeOut")]}}));
    }
    let mut scene = json!({"layers": [{"id": "card", "type": "group", "layers": layers,
        "keyframes": {"opacity": [k((d - 0.5).max(reveal_end + 0.5), 1.0, "linear"), k(d, 0.0, "easeIn")]}}]});
    if let Some(bg) = v.opt("background") {
        scene["background"] = json!(bg);
    }
    scene
}

fn subscribe(v: &Values, c: &Ctx) -> Value {
    let (h, d) = (c.height, c.duration);
    let s = h / 1080.0;
    let y = match v.s("position").as_str() {
        "top" => -h * 0.32,
        "center" => 0.0,
        _ => h * 0.32,
    };
    let (label, done) = (v.s("label"), v.s("done"));
    let bw = text_width(&label, 44.0 * s, 800.0).max(text_width(&done, 44.0 * s, 800.0)) + 120.0 * s;
    let bh = 100.0 * s;
    let click = (d * 0.45).max(1.0);
    let out = (d - 0.4).max(click + 0.6);
    let cursor = "M0 0 L0 46 L12 35 L20 54 L28 50 L20 32 L36 32 Z";
    json!({"layers": [{"id": "subscribe", "type": "group", "y": y, "layers": [
        {"id": "button", "type": "rect", "width": bw, "height": bh, "radius": bh / 2.0, "fill": v.s("accent"),
         "shadow": {"color": "#00000080", "blur": 24.0 * s, "y": 8.0 * s},
         "keyframes": {"fill": [k(click, v.s("accent"), "linear"), k(click + 0.15, "#3a3a40", "linear")],
                       "scale": [k(0.0, 0.0, "linear"), k(0.45, 1.0, "easeOutBack"), k(click, 1.0, "linear"), k(click + 0.08, 0.92, "easeOut"), k(click + 0.3, 1.0, "easeOutBack")]}},
        {"id": "label", "type": "text", "text": label, "fontSize": 44.0 * s, "fontWeight": 800, "fill": "#ffffff", "letterSpacing": 2.0 * s,
         "keyframes": {"text": [k(0.0, v.s("label"), "linear"), k(click + 0.1, done, "hold")],
                       "scale": [k(0.0, 0.0, "linear"), k(0.45, 1.0, "easeOutBack"), k(click, 1.0, "linear"), k(click + 0.08, 0.92, "easeOut"), k(click + 0.3, 1.0, "easeOutBack")]}},
        {"id": "cursor", "type": "path", "d": cursor, "fill": "#ffffff", "stroke": {"color": "#111111", "width": 3.0, "join": "round"},
         "scale": s * 1.4, "shadow": {"color": "#00000088", "blur": 8.0, "y": 3.0},
         "keyframes": {"x": [k(0.5, bw * 0.9, "linear"), k(click - 0.05, bw * 0.12, "easeInOutCubic")],
                       "y": [k(0.5, bh * 1.6, "linear"), k(click - 0.05, 0.0, "easeInOutCubic")],
                       "opacity": [k(0.45, 0.0, "linear"), k(0.6, 1.0, "linear"), k(click + 0.5, 1.0, "linear"), k(click + 0.8, 0.0, "linear")],
                       "scale": [k(click - 0.05, s * 1.4, "linear"), k(click + 0.05, s * 1.15, "easeOut"), k(click + 0.2, s * 1.4, "easeOut")]}}
    ], "keyframes": {"opacity": [k(out, 1.0, "linear"), k(d, 0.0, "easeIn")]}}]})
}

fn aurora(v: &Values, c: &Ctx) -> Value {
    let (w, h, d) = (c.width, c.height, c.duration);
    let colors: Vec<String> = v.list("colors").iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
    let colors = if colors.is_empty() { vec!["#ff5a36".to_string()] } else { colors };
    let mut layers = vec![];
    let spots = [(-0.28, -0.2, 0.22, 0.12), (0.25, 0.18, -0.18, -0.1), (0.05, -0.28, 0.1, 0.3), (-0.2, 0.3, 0.3, -0.2), (0.32, -0.25, -0.25, 0.2)];
    for (i, (x, y, dx, dy)) in spots.iter().enumerate() {
        let color = &colors[i % colors.len()];
        let size = w * (0.55 - i as f64 * 0.04);
        layers.push(json!({
            "id": format!("blob{}", i + 1), "type": "ellipse", "width": size, "height": size * 0.8, "fill": color, "blur": w * 0.08, "opacity": 0.75,
            "blend": "screen",
            "keyframes": {
                "x": [k(0.0, x * w, "linear"), k(d, (x + dx) * w, "easeInOutSine")],
                "y": [k(0.0, y * h, "linear"), k(d, (y + dy) * h, "easeInOutSine")],
                "scale": [k(0.0, 1.0, "linear"), k(d / 2.0, 1.15, "easeInOutSine"), k(d, 0.95, "easeInOutSine")]
            }
        }));
    }
    json!({"background": v.s("background"), "layers": layers})
}

fn wipe(v: &Values, c: &Ctx) -> Value {
    let (w, h, d) = (c.width, c.height, c.duration);
    let mid = d / 2.0;
    let (front, second) = (v.s("color"), v.s("second"));
    if v.s("style") == "circle" {
        let r = (w * w + h * h).sqrt();
        return json!({"layers": [
            {"id": "back", "type": "ellipse", "width": r, "height": r, "fill": second,
             "keyframes": {"scale": [k(0.0, 0.0, "linear"), k(mid, 1.05, "easeInCubic"), k(mid + 0.001, 1.05, "hold"), k(d, 0.0, "easeOutCubic")]}},
            {"id": "front", "type": "ellipse", "width": r, "height": r, "fill": front,
             "keyframes": {"scale": [k(0.0, 0.0, "linear"), k(mid * 0.8, 0.9, "easeInCubic"), k(mid + 0.1, 0.0, "easeOutCubic")]}}
        ]});
    }
    let (axis, span, sign) = match v.s("direction").as_str() {
        "left" => ("x", w, -1.0),
        "up" => ("y", h, -1.0),
        "down" => ("y", h, 1.0),
        _ => ("x", w, 1.0),
    };
    let band = |id: &str, color: &str, delay: f64| {
        json!({"id": id, "type": "rect", "width": w * 1.02, "height": h * 1.02, "fill": color,
            "keyframes": {axis: [k(delay, -sign * span, "linear"), k(mid + delay * 0.5, 0.0, "easeInOutCubic"), k(d - delay, sign * span, "easeInOutCubic")]}})
    };
    json!({"layers": [band("back", &second, 0.0), band("front", &front, (d * 0.08).min(0.12))]})
}

// ---------------------------------------------------------------------------------------------
// 3D templates

fn bg3d(scene: &mut Value, v: &Values) {
    if let Some(bg) = v.opt("background") {
        scene["background"] = json!(bg);
    }
}

fn studio_lights() -> Value {
    json!([
        {"id": "key", "type": "directional", "direction": [-0.6, -0.8, -0.7], "intensity": 1.6, "color": "#fff4e8"},
        {"id": "fill", "type": "directional", "direction": [0.8, -0.3, -0.5], "intensity": 0.45, "color": "#dfe8ff"},
        {"id": "rim", "type": "directional", "direction": [0.2, -0.4, 1.0], "intensity": 0.9, "color": "#ffffff"}
    ])
}

fn title_3d(v: &Values, c: &Ctx) -> Value {
    let d = c.duration;
    let text = v.s("text");
    let size = (6.5 / text.chars().count().max(1) as f64 * 1.6).clamp(0.4, 1.6);
    let mut objects = vec![json!({
        "id": "title", "type": "text", "text": text, "size": size, "depth": v.n("depth"), "position": [0, 0.15, 0],
        "material": {"color": v.s("color"), "metallic": v.n("metallic"), "roughness": 0.35},
        "keyframes": {
            "rotation.y": [k(0.0, -38.0, "linear"), k(1.6, 0.0, "easeOutCubic"), k(d, 6.0, "linear")],
            "rotation.x": [k(0.0, 18.0, "linear"), k(1.6, 0.0, "easeOutCubic")],
            "position.y": [k(0.0, -1.2, "linear"), k(1.2, 0.15, "easeOutBack")],
            "opacity": [k(0.0, 0.0, "linear"), k(0.4, 1.0, "linear")]
        }
    })];
    if v.b("floor") {
        objects.push(json!({"id": "floor", "type": "plane", "width": 40, "height": 40, "position": [0, -0.9, 0], "rotation": [-90, 0, 0],
            "material": {"color": v.opt("background").unwrap_or_else(|| "#1a1a1f".into()), "roughness": 0.9}}));
    }
    let mut scene = json!({
        "camera": {"position": [0, 0.6, 9], "target": [0, 0, 0], "fov": 35,
                   "keyframes": {"position": [k(0.0, json!([0.8, 1.2, 10.5]), "linear"), k(d, json!([0, 0.5, 7.5]), "easeOutCubic")]}},
        "ambient": 0.3,
        "lights": studio_lights(),
        "objects": objects,
    });
    bg3d(&mut scene, v);
    scene
}

fn logo_spin_3d(v: &Values, c: &Ctx) -> Value {
    let d = c.duration;
    let logo = match v.opt("image") {
        Some(img) => json!({"id": "logo", "type": "image", "asset": img, "width": 2.6}),
        None => json!({"id": "logo", "type": "text", "text": v.s("text"), "size": 2.2, "depth": 0.5,
                       "material": {"color": v.s("color"), "metallic": 0.6, "roughness": 0.25}}),
    };
    let mut logo = logo;
    logo["keyframes"] = json!({
        "rotation.y": [k(0.0, -180.0, "linear"), k(d * 0.75, 180.0, "easeInOutCubic")],
        "scale": [k(0.0, 0.0, "linear"), k(0.6, 1.0, "easeOutBack")]
    });
    let mut scene = json!({
        "camera": {"position": [0, 0.4, 7], "fov": 38},
        "ambient": 0.35,
        "lights": studio_lights(),
        "objects": [
            logo,
            {"id": "ring", "type": "torus", "radius": 2.0, "tube": 0.05, "rotation": [90, 0, 0],
             "material": {"color": v.s("accent"), "emissive": v.s("accent"), "emissiveIntensity": 1.2, "unlit": true},
             "keyframes": {"scale": [k(0.0, 0.2, "linear"), k(0.8, 1.0, "easeOutCubic")], "rotation.x": [k(0.0, 90.0, "linear"), k(d, 70.0, "easeInOutSine")], "rotation.y": [k(0.0, 0.0, "linear"), k(d, 40.0, "linear")]}}
        ]
    });
    bg3d(&mut scene, v);
    scene
}

fn turntable(v: &Values, c: &Ctx) -> Value {
    let d = c.duration;
    let subject = match v.opt("model") {
        Some(src) => json!({"id": "subject", "type": "model", "src": src, "position": [0, 1.0, 0]}),
        None => json!({"id": "subject", "type": "group", "position": [0, 0.55, 0], "children": [
            {"id": "base", "type": "box", "size": [1.4, 0.5, 1.4], "bevel": 0.08, "material": {"color": v.s("color"), "roughness": 0.4}},
            {"id": "middle", "type": "cylinder", "radius": 0.45, "height": 0.7, "position": [0, 0.6, 0], "material": {"color": "#f2f2f2", "roughness": 0.3}},
            {"id": "top", "type": "sphere", "radius": 0.42, "position": [0, 1.32, 0], "material": {"color": v.s("color"), "metallic": 0.7, "roughness": 0.2}}
        ]}),
    };
    let mut subject = subject;
    subject["keyframes"] = json!({"rotation.y": [k(0.0, 0.0, "linear"), k(d, 360.0, "linear")]});
    let mut scene = json!({
        "camera": {"position": [0, 3.2, 6.5], "target": [0, 1.0, 0], "fov": 36},
        "ambient": 0.3,
        "lights": studio_lights(),
        "objects": [
            {"id": "pedestal", "type": "cylinder", "radius": 1.6, "height": 0.3, "position": [0, 0.15, 0], "material": {"color": v.s("pedestal"), "roughness": 0.6}},
            subject,
            {"id": "floor", "type": "plane", "width": 40, "height": 40, "rotation": [-90, 0, 0], "material": {"color": v.opt("background").unwrap_or_else(|| "#1a1a1f".into()), "roughness": 0.95}}
        ]
    });
    bg3d(&mut scene, v);
    scene
}

fn shapes_3d(v: &Values, c: &Ctx) -> Value {
    let d = c.duration;
    let colors: Vec<String> = v.list("colors").iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
    let colors = if colors.is_empty() { vec!["#ff5a36".to_string()] } else { colors };
    let spots: [(&str, [f64; 3], f64); 8] = [
        ("sphere", [-3.2, 1.2, -1.0], 0.0),
        ("torus", [2.8, 1.5, -2.0], 0.7),
        ("box", [-1.4, -1.4, 0.5], 1.3),
        ("sphere", [1.6, -1.2, 1.0], 2.1),
        ("torus", [-3.6, -0.8, -3.0], 2.9),
        ("box", [3.8, -0.4, -1.5], 3.6),
        ("sphere", [0.2, 2.2, -3.5], 4.4),
        ("cone", [-0.6, 0.3, -1.8], 5.0),
    ];
    let mut objects = vec![];
    for (i, (shape, p, phase)) in spots.iter().enumerate() {
        let color = &colors[i % colors.len()];
        let mut o = match *shape {
            "sphere" => json!({"type": "sphere", "radius": 0.55 + (i % 3) as f64 * 0.15}),
            "torus" => json!({"type": "torus", "radius": 0.6, "tube": 0.22}),
            "box" => json!({"type": "box", "size": 0.9, "bevel": 0.12}),
            _ => json!({"type": "cone", "radius": 0.5, "height": 1.0}),
        };
        o["id"] = json!(format!("shape{}", i + 1));
        o["position"] = json!(p);
        o["material"] = json!({"color": color, "metallic": if i % 2 == 0 { 0.1 } else { 0.6 }, "roughness": 0.25});
        let bob = 0.35;
        let period = 3.0 + (i % 3) as f64;
        let mut ys = vec![];
        let mut t = 0.0;
        let mut up = i % 2 == 0;
        while t <= d + 1e-9 {
            ys.push(k(t, p[1] + if up { bob } else { -bob }, if t == 0.0 { "linear" } else { "easeInOutSine" }));
            up = !up;
            t += period / 2.0;
        }
        o["keyframes"] = json!({
            "position.y": ys,
            "rotation.y": [k(0.0, phase * 40.0, "linear"), k(d, phase * 40.0 + 120.0, "linear")],
            "rotation.x": [k(0.0, phase * 25.0, "linear"), k(d, phase * 25.0 + 60.0, "linear")]
        });
        objects.push(o);
    }
    let mut scene = json!({
        "camera": {"position": [0, 0, 9], "fov": 40, "keyframes": {"position.x": [k(0.0, -0.6, "linear"), k(d, 0.6, "easeInOutSine")]}},
        "ambient": 0.35,
        "lights": studio_lights(),
        "objects": objects,
    });
    bg3d(&mut scene, v);
    scene
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_builds_with_its_defaults() {
        for t in TEMPLATES {
            for (w, h) in [(1920.0, 1080.0), (1080.0, 1920.0)] {
                let scene = t.build(&Map::new(), Ctx { width: w, height: h, duration: t.duration }).unwrap_or_else(|e| panic!("{}: {e}", t.id));
                assert_eq!(scene.is_3d(), t.kind == "3d", "{}", t.id);
            }
            // Very short clips still make valid scenes.
            t.build(&Map::new(), Ctx { width: 1280.0, height: 720.0, duration: 0.5 }).unwrap_or_else(|e| panic!("{} short: {e}", t.id));
        }
    }

    #[test]
    fn values_are_checked() {
        let t = find("lowerThird").unwrap();
        let bad = |v: Value| t.values(v.as_object().unwrap()).unwrap_err();
        assert!(bad(json!({"titel": "x"})).contains("Did you mean `title`"));
        assert!(bad(json!({"accent": "orange"})).contains("colour"));
        assert!(bad(json!({"side": "middle"})).contains("left, right"));
        let ok = t.values(json!({"title": "Grace Hopper", "plate": null}).as_object().unwrap()).unwrap();
        assert_eq!(ok["subtitle"], "Mathematician");
        let scene = t.build(json!({"title": "Grace Hopper", "plate": null}).as_object().unwrap(), Ctx { width: 1920.0, height: 1080.0, duration: 5.0 }).unwrap();
        assert!(!scene.ids().contains(&"plate".to_string()));
    }
}
