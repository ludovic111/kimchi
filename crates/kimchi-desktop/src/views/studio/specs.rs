//! What the Properties panel shows for each kind of thing: its fields, in sections, with the
//! widget each needs. A field's name is its keyframe name when it is animatable, or a path into
//! the thing's JSON (`stroke.cap`, `material.flat`) for settings that hold.

use kimchi_core::motion::particles::{EMITTERS, SHAPES_2D, SHAPES_3D};

/// How a field is edited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fk {
    /// step, decimals, min, max
    Num(f64, usize, f64, f64),
    /// A number that may be unset (switch + number).
    OptNum(f64, usize, f64, f64),
    Color,
    /// A colour that may be unset (switch + colour).
    OptColor,
    Text,
    /// Several lines of words.
    Long,
    /// SVG path data (sent when done typing).
    Path,
    Choice(&'static [&'static str]),
    Bool,
    /// Three numbers (step, decimals).
    Vec3(f64, usize),
    /// Two numbers (step, decimals).
    Vec2(f64, usize),
    /// A media item (picture, video, model).
    Asset,
    /// Another thing of the scene (layers next to it).
    Sibling,
    /// A composition of the scene.
    Composition,
    /// A camera of the scene.
    Camera,
    /// A list of points (3D curve points, lathe profile [r, h]).
    Points(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct F {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: Fk,
    /// The value when the thing doesn't say (JSON).
    pub def: &'static str,
}

const fn f(name: &'static str, label: &'static str, kind: Fk, def: &'static str) -> F {
    F { name, label, kind, def }
}

pub struct Section {
    pub title: &'static str,
    pub fields: Vec<F>,
}

fn sec(title: &'static str, fields: Vec<F>) -> Section {
    Section { title, fields }
}

const POS: Fk = Fk::Num(0.01, 2, 0.0, 100_000.0);
const UNIT: Fk = Fk::Num(0.01, 2, 0.0, 1.0);
const PXN: Fk = Fk::Num(1.0, 0, -100_000.0, 100_000.0);
const PXP: Fk = Fk::Num(1.0, 0, 0.0, 100_000.0);
const DEG: Fk = Fk::Num(1.0, 1, -36_000.0, 36_000.0);
const SEC: Fk = Fk::Num(0.01, 2, 0.0, 100_000.0);
const INT: fn(f64, f64) -> Fk = |a, b| Fk::Num(1.0, 0, a, b);
const ALIGN: &[&str] = &["left", "center", "right"];
pub const BLENDS: &[&str] = &[
    "normal", "multiply", "screen", "overlay", "add", "darken", "lighten", "difference", "colorDodge", "colorBurn", "softLight", "hardLight", "exclusion", "hue",
    "saturation", "color", "luminosity",
];

/// 3D object: the transform.
pub fn object_transform() -> Section {
    sec("Transform", vec![f("position", "Location", Fk::Vec3(0.05, 2), "[0,0,0]"), f("rotation", "Rotation", Fk::Vec3(1.0, 1), "[0,0,0]"), f("scale", "Scale", Fk::Vec3(0.01, 2), "[1,1,1]")])
}

/// 3D object: its shape's own fields.
pub fn shape_fields(shape: &str) -> Vec<F> {
    match shape {
        "box" => vec![f("size", "Size", Fk::Vec3(0.05, 2), "[1,1,1]"), f("bevel", "Rounded edges", Fk::Num(0.005, 3, 0.0, 0.5), "0")],
        "sphere" => vec![f("radius", "Radius", POS, "0.5"), f("segments", "Segments (0 = smooth)", INT(0.0, 256.0), "0")],
        "icosphere" => vec![f("radius", "Radius", POS, "0.5"), f("detail", "Detail", INT(0.0, 6.0), "2")],
        "cylinder" | "cone" => vec![f("radius", "Radius", POS, "0.5"), f("height", "Height", POS, "1"), f("segments", "Sides (0 = smooth)", INT(0.0, 256.0), "0")],
        "capsule" => vec![f("radius", "Radius", POS, "0.3"), f("height", "Height", POS, "1")],
        "torus" => vec![f("radius", "Radius", POS, "0.5"), f("tube", "Tube", POS, "0.2")],
        "plane" => vec![f("width", "Width", POS, "1"), f("height", "Height", POS, "1")],
        "grid" => vec![f("width", "Width", POS, "2"), f("height", "Depth", POS, "2"), f("rows", "Rows", INT(1.0, 512.0), "32"), f("cols", "Columns", INT(1.0, 512.0), "32")],
        "text" => vec![
            f("text", "Words", Fk::Long, "\"\""),
            f("fontFamily", "Font", Fk::Choice(FONTS), "\"Manrope\""),
            f("fontWeight", "Weight", Fk::Num(100.0, 0, 100.0, 900.0), "700"),
            f("size", "Size", POS, "1"),
            f("depth", "Depth", POS, "0.2"),
            f("align", "Align", Fk::Choice(ALIGN), "\"center\""),
            f("letterSpacing", "Letter spacing", Fk::Num(0.005, 3, -1.0, 5.0), "0"),
            f("bevel", "Rounded edges", Fk::Num(0.005, 3, 0.0, 1.0), "0"),
        ],
        "extrude" => vec![f("d", "Outline (SVG path)", Fk::Path, "\"\""), f("size", "Size", POS, "2"), f("depth", "Depth", POS, "0.2"), f("bevel", "Rounded edges", Fk::Num(0.005, 3, 0.0, 1.0), "0")],
        "lathe" => vec![f("profile", "Profile (radius, height)", Fk::Points(2), "[]"), f("segments", "Segments", INT(3.0, 512.0), "48"), f("angle", "Angle", Fk::Num(1.0, 0, 0.0, 360.0), "360")],
        "curve" => vec![
            f("points", "Points", Fk::Points(3), "[]"),
            f("closed", "Closed", Fk::Bool, "false"),
            f("smooth", "Smooth", Fk::Bool, "true"),
            f("radius", "Tube radius (0 = invisible path)", Fk::Num(0.005, 3, 0.0, 100.0), "0"),
            f("sides", "Tube sides", INT(3.0, 64.0), "8"),
            f("trimStart", "Draw from", UNIT, "0"),
            f("trimEnd", "Draw to", UNIT, "1"),
        ],
        "mesh" => vec![f("autoSmooth", "Smooth below (degrees)", Fk::Num(1.0, 0, 0.0, 180.0), "30")],
        "particles" => particle_fields(true),
        "model" => vec![f("src", "File", Fk::Asset, "\"\"")],
        "image" => vec![f("asset", "Picture", Fk::Asset, "\"\""), f("width", "Width", POS, "2")],
        _ => vec![],
    }
}

pub const FONTS: &[&str] = &["Manrope", "IBM Plex Mono", "Inter", "Georgia", "Helvetica", "Arial", "Times New Roman", "Courier New"];

pub fn particle_fields(three: bool) -> Vec<F> {
    let size = if three { "0.08" } else { "12" };
    vec![
        f("rate", "Per second", Fk::Num(1.0, 0, 0.0, 100_000.0), "20"),
        f("burst", "Burst", Fk::Num(1.0, 0, 0.0, 100_000.0), "0"),
        f("max", "At most", Fk::Num(1.0, 0, 1.0, 100_000.0), "500"),
        f("lifetime", "Lifetime", SEC, "2"),
        f("lifetimeRandom", "Lifetime varies", UNIT, "0.3"),
        f("emitter", "Born from", Fk::Choice(EMITTERS), "\"point\""),
        f("emitterSize", "Emitter size", Fk::Vec3(if three { 0.05 } else { 1.0 }, 2), "[0,0,0]"),
        f("direction", "Direction", Fk::Vec3(0.05, 2), if three { "[0,1,0]" } else { "[0,-1,0]" }),
        f("spread", "Spread", Fk::Num(1.0, 0, 0.0, 360.0), "30"),
        f("speed", "Speed", Fk::Num(if three { 0.05 } else { 1.0 }, 2, 0.0, 100_000.0), if three { "2" } else { "200" }),
        f("speedRandom", "Speed varies", UNIT, "0.3"),
        f("gravity", "Gravity", Fk::Vec3(0.05, 2), "[0,0,0]"),
        f("drag", "Drag", Fk::Num(0.01, 2, 0.0, 10.0), "0"),
        f("turbulence", "Turbulence", Fk::Num(0.05, 2, 0.0, 10_000.0), "0"),
        f("size", "Size", Fk::Num(if three { 0.005 } else { 0.5 }, 3, 0.0, 10_000.0), size),
        f("sizeEnd", "Size at the end", Fk::Num(0.01, 2, 0.0, 100.0), "1"),
        f("sizeRandom", "Size varies", UNIT, "0.3"),
        f("spin", "Spin (°/s)", DEG, "0"),
        f("spinRandom", "Spin varies", UNIT, "0"),
        f("color", "Colour", Fk::Color, "\"#ffffff\""),
        f("colorEnd", "Colour at the end", Fk::OptColor, "null"),
        f("fadeIn", "Fade in", UNIT, "0.1"),
        f("fadeOut", "Fade out", UNIT, "0.3"),
        f("shape", "Shape", Fk::Choice(if three { SHAPES_3D } else { SHAPES_2D }), if three { "\"sphere\"" } else { "\"circle\"" }),
        f("asset", "Picture (image shape)", Fk::Asset, "null"),
        f("stretch", "Spark length", Fk::Num(0.005, 3, 0.0, 10.0), "0.05"),
        f("emitFrom", "Starts at", SEC, "0"),
        f("emitUntil", "Stops at", Fk::OptNum(0.05, 2, 0.0, 100_000.0), "null"),
        f("trail", "Leave a trail", Fk::Bool, "true"),
        f("prewarm", "Prewarm", SEC, "0"),
        f("seed", "Seed", Fk::Num(1.0, 0, 0.0, 1e6), "0"),
    ]
}

pub fn object_visibility() -> Section {
    sec("Visibility", vec![
        f("hidden", "Hidden", Fk::Bool, "false"),
        f("castShadow", "Casts shadows", Fk::Bool, "true"),
        f("start", "Appears at", SEC, "0"),
        f("end", "Disappears at", Fk::OptNum(0.05, 2, 0.0, 100_000.0), "null"),
    ])
}

pub fn light_fields(kind: &str) -> Vec<Section> {
    let mut main = vec![f("type", "Type", Fk::Choice(&["directional", "point", "spot", "area"]), "\"directional\""), f("color", "Colour", Fk::Color, "\"#ffffff\""), f("intensity", "Intensity", Fk::Num(0.05, 2, 0.0, 1000.0), "1")];
    if kind != "directional" {
        main.push(f("position", "Location", Fk::Vec3(0.05, 2), "[3,4,5]"));
    }
    if kind != "point" {
        main.push(f("direction", "Shines towards", Fk::Vec3(0.05, 2), "[-0.5,-1,-0.7]"));
    }
    if matches!(kind, "point" | "spot") {
        main.push(f("range", "Range (0 = endless)", Fk::Num(0.1, 1, 0.0, 10_000.0), "0"));
    }
    if kind == "spot" {
        main.push(f("angle", "Cone angle", Fk::Num(1.0, 0, 1.0, 179.0), "45"));
        main.push(f("blend", "Soft edge", UNIT, "0.2"));
    }
    main.push(f("size", if kind == "area" { "Size" } else { "Softness" }, Fk::Vec2(0.05, 2), "[0,0]"));
    vec![sec("Light", main), sec("Visibility", vec![f("castShadows", "Casts shadows", Fk::Bool, "true"), f("hidden", "Off", Fk::Bool, "false")])]
}

pub fn camera_fields() -> Vec<Section> {
    vec![
        sec("Camera", vec![
            f("position", "Location", Fk::Vec3(0.05, 2), "[0,0,8]"),
            f("target", "Looks at", Fk::Vec3(0.05, 2), "[0,0,0]"),
            f("roll", "Roll", DEG, "0"),
            f("projection", "Projection", Fk::Choice(&["perspective", "orthographic"]), "\"perspective\""),
            f("fov", "Field of view", Fk::Num(0.5, 1, 1.0, 170.0), "40"),
            f("orthoSize", "Orthographic size", POS, "5"),
        ]),
        sec("Depth of field", vec![f("fStop", "f-stop (0 = off)", Fk::Num(0.1, 1, 0.0, 64.0), "0"), f("focusDistance", "Focus distance (0 = the target)", POS, "0")]),
    ]
}

pub fn world_fields() -> Vec<Section> {
    vec![
        sec("World", vec![
            f("background", "Background", Fk::OptColor, "null"),
            f("ambient", "Ambient light", UNIT, "0.25"),
            f("ambientColor", "Ambient colour", Fk::Color, "\"#ffffff\""),
            f("activeCamera", "Active camera", Fk::Camera, "\"camera\""),
            f("shadows", "Shadows", Fk::Bool, "true"),
            f("fog", "Fade into the background", Fk::Bool, "true"),
        ]),
        sec("Environment", vec![
            f("environment.type", "Type", Fk::Choice(&["none", "color", "gradient", "sky", "image"]), "\"none\""),
            f("environment.color", "Colour", Fk::Color, "\"#808080\""),
            f("environment.top", "Sky", Fk::Color, "\"#5b7fb8\""),
            f("environment.horizon", "Horizon", Fk::Color, "\"#d8e2ee\""),
            f("environment.bottom", "Ground", Fk::Color, "\"#3a3632\""),
            f("environment.image", "Panorama", Fk::Asset, "null"),
            f("environment.strength", "Strength", Fk::Num(0.05, 2, 0.0, 100.0), "1"),
            f("environment.rotation", "Rotation", DEG, "0"),
            f("environment.visible", "Show behind objects", Fk::Bool, "false"),
        ]),
    ]
}

pub fn render_fields() -> Vec<Section> {
    vec![
        sec("Engine", vec![
            f("render.engine", "Engine", Fk::Choice(&["standard", "path"]), "\"standard\""),
            f("render.samples", "Samples (path)", INT(1.0, 100_000.0), "64"),
            f("render.bounces", "Bounces (path)", INT(0.0, 64.0), "4"),
            f("render.denoise", "Denoise (path)", Fk::Bool, "true"),
        ]),
        sec("Look", vec![
            f("render.exposure", "Exposure", Fk::Num(0.05, 2, -10.0, 10.0), "0"),
            f("render.toneMapping", "Tone mapping", Fk::Choice(&["standard", "filmic"]), "\"standard\""),
            f("render.bloom", "Bloom", Fk::Num(0.01, 2, 0.0, 10.0), "0"),
            f("render.bloomThreshold", "Bloom threshold", Fk::Num(0.01, 2, 0.0, 10.0), "0.8"),
            f("render.bloomRadius", "Bloom size", Fk::Num(0.005, 3, 0.0, 1.0), "0.05"),
            f("render.ambientOcclusion", "Ambient occlusion", Fk::Num(0.01, 2, 0.0, 1.0), "0"),
            f("render.motionBlur", "Motion blur (shutter)", UNIT, "0"),
            f("render.motionBlurSamples", "Motion blur samples", INT(1.0, 64.0), "8"),
        ]),
    ]
}

/// A 3D object's own material (shared ones use the same fields without `material.`).
pub fn material_fields(prefix: &str) -> Vec<F> {
    // Animatable names are bare on objects; settings live under `material.`.
    let p = |n: &'static str| -> &'static str {
        if prefix.is_empty() {
            n.strip_prefix("material.").unwrap_or(n)
        } else {
            n
        }
    };
    vec![
        f("color", "Colour", Fk::Color, "\"#d9d9d9\""),
        f("metallic", "Metallic", UNIT, "0"),
        f("roughness", "Roughness", UNIT, "0.5"),
        f("emissive", "Glow colour", Fk::OptColor, "null"),
        f("emissiveIntensity", "Glow strength", Fk::Num(0.05, 2, 0.0, 1000.0), "1"),
        f("opacity", "Opacity", UNIT, "1"),
        f("transmission", "Glass (transmission)", UNIT, "0"),
        f("ior", "Index of refraction", Fk::Num(0.01, 2, 1.0, 3.0), "1.45"),
        f("clearcoat", "Clear coat", UNIT, "0"),
        f(p("material.texture"), "Texture", Fk::Asset, "null"),
        f("textureScale", "Texture repeats", Fk::Vec2(0.1, 2), "[1,1]"),
        f(p("material.flat"), "Flat (faceted)", Fk::Bool, "false"),
        f(p("material.unlit"), "Unlit", Fk::Bool, "false"),
    ]
}

/// 2D layer: the transform.
pub fn layer_transform() -> Section {
    sec("Transform", vec![
        f("x", "X", PXN, "0"),
        f("y", "Y", PXN, "0"),
        f("anchorX", "Anchor X", PXN, "0"),
        f("anchorY", "Anchor Y", PXN, "0"),
        f("scale", "Scale", Fk::Num(0.01, 2, -1000.0, 1000.0), "1"),
        f("scaleX", "Scale X", Fk::Num(0.01, 2, -1000.0, 1000.0), "1"),
        f("scaleY", "Scale Y", Fk::Num(0.01, 2, -1000.0, 1000.0), "1"),
        f("rotation", "Rotation", DEG, "0"),
        f("skewX", "Skew", Fk::Num(0.5, 1, -85.0, 85.0), "0"),
        f("opacity", "Opacity", UNIT, "1"),
    ])
}

/// 2D layer: its kind's own fields.
pub fn kind_fields(kind: &str) -> Vec<F> {
    match kind {
        "rect" => vec![f("width", "Width", PXP, "100"), f("height", "Height", PXP, "100"), f("radius", "Corners", PXP, "0")],
        "ellipse" => vec![f("width", "Width", PXP, "100"), f("height", "Height", PXP, "100")],
        "polygon" => vec![f("sides", "Sides", INT(3.0, 64.0), "3"), f("radius", "Radius", PXP, "50"), f("roundness", "Roundness", UNIT, "0")],
        "star" => vec![f("points", "Points", INT(2.0, 64.0), "5"), f("radius", "Radius", PXP, "50"), f("innerRadius", "Inner radius", PXP, "20")],
        "path" => vec![f("d", "Path (SVG)", Fk::Path, "\"\""), f("closed", "Closed", Fk::Bool, "false")],
        "text" => vec![
            f("text", "Words", Fk::Long, "\"\""),
            f("fontFamily", "Font", Fk::Choice(FONTS), "\"Manrope\""),
            f("fontSize", "Size", PXP, "72"),
            f("fontWeight", "Weight", Fk::Num(100.0, 0, 100.0, 900.0), "700"),
            f("italic", "Italic", Fk::Bool, "false"),
            f("align", "Align", Fk::Choice(ALIGN), "\"center\""),
            f("lineHeight", "Line height", Fk::Num(0.01, 2, 0.1, 10.0), "1.15"),
            f("letterSpacing", "Tracking", Fk::Num(0.5, 1, -500.0, 1000.0), "0"),
            f("value", "Number ({value})", Fk::OptNum(1.0, 2, -1e12, 1e12), "null"),
            f("decimals", "Decimals", INT(0.0, 6.0), "0"),
            f("reveal", "Reveal progress", UNIT, "1"),
            f("reveal.by", "Reveal by", Fk::Choice(&["char", "word", "line"]), "\"char\""),
            f("reveal.style", "Reveal style", Fk::Choice(kimchi_core::motion::REVEAL_STYLES), "\"rise\""),
            f("reveal.overlap", "Reveal overlap", Fk::Num(0.1, 1, 0.0, 100.0), "3"),
            f("path", "On a path (SVG)", Fk::Path, "null"),
            f("pathOffset", "Along the path", UNIT, "0"),
        ],
        "image" => vec![f("asset", "Picture", Fk::Asset, "\"\""), f("width", "Width", Fk::OptNum(1.0, 0, 0.0, 100_000.0), "null"), f("height", "Height", Fk::OptNum(1.0, 0, 0.0, 100_000.0), "null"), f("radius", "Corners", PXP, "0")],
        "comp" => vec![
            f("comp", "Composition", Fk::Composition, "\"\""),
            f("speed", "Speed", Fk::Num(0.01, 2, -100.0, 100.0), "1"),
            f("offset", "Starts at (its time)", Fk::Num(0.01, 2, -100_000.0, 100_000.0), "0"),
            f("loop", "Loop", Fk::Bool, "false"),
            f("time", "Time remap (its time shown)", Fk::OptNum(0.01, 2, -100_000.0, 100_000.0), "null"),
        ],
        "particles" => particle_fields(false),
        _ => vec![],
    }
}

/// 2D layer: fill, outline and the looks every shape has.
pub fn layer_style(kind: &str) -> Vec<Section> {
    let mut out = vec![];
    if !matches!(kind, "group" | "null" | "adjustment" | "comp" | "image" | "particles") {
        out.push(sec("Fill and outline", vec![
            f("fill", "Fill", Fk::Color, "\"#ffffff\""),
            f("strokeColor", "Outline", Fk::Color, "\"#ffffff\""),
            f("strokeWidth", "Outline width", Fk::Num(0.5, 1, 0.0, 1000.0), "0"),
            f("stroke.cap", "Line ends", Fk::Choice(&["butt", "round", "square"]), "\"round\""),
            f("stroke.join", "Corners", Fk::Choice(&["miter", "round", "bevel"]), "\"round\""),
            f("dashOffset", "Dash offset", Fk::Num(0.5, 1, -100_000.0, 100_000.0), "0"),
            f("trimStart", "Draw from", UNIT, "0"),
            f("trimEnd", "Draw to", UNIT, "1"),
            f("trimOffset", "Draw offset", UNIT, "0"),
        ]));
    }
    if kind != "null" {
        out.push(sec("Look", vec![
            f("blur", "Blur", Fk::Num(0.5, 1, 0.0, 1000.0), "0"),
            f("blend", "Blend", Fk::Choice(BLENDS), "\"normal\""),
            f("motionBlur", "Motion blur", Fk::Bool, "false"),
            f("shadowColor", "Shadow", Fk::Color, "\"#00000080\""),
            f("shadowBlur", "Shadow softness", Fk::Num(0.5, 1, 0.0, 1000.0), "12"),
            f("shadowX", "Shadow X", PXN, "0"),
            f("shadowY", "Shadow Y", PXN, "6"),
            f("glowColor", "Glow", Fk::Color, "\"#ffffff\""),
            f("glowRadius", "Glow size", Fk::Num(0.5, 1, 0.0, 1000.0), "20"),
            f("glowStrength", "Glow strength", Fk::Num(0.01, 2, 0.0, 2.0), "1"),
        ]));
    }
    out.push(sec("Links", vec![
        f("parent", "Parent", Fk::Sibling, "null"),
        f("matte.layer", "Track matte", Fk::Sibling, "null"),
        f("matte.mode", "Matte mode", Fk::Choice(kimchi_core::motion::MATTE_MODES), "\"alpha\""),
        f("mask", "Shape mask (layer)", Fk::Sibling, "null"),
        f("maskInvert", "Invert shape mask", Fk::Bool, "false"),
    ]));
    out.push(sec("Timing", vec![f("start", "Appears at", SEC, "0"), f("end", "Disappears at", Fk::OptNum(0.05, 2, 0.0, 100_000.0), "null"), f("hidden", "Hidden", Fk::Bool, "false")]));
    out
}

pub fn scene2d_fields() -> Vec<Section> {
    vec![sec("Scene", vec![f("background", "Background", Fk::OptColor, "null"), f("shutter", "Motion blur shutter", UNIT, "0.5"), f("motionBlurSamples", "Motion blur samples", INT(1.0, 64.0), "8")])]
}

pub fn composition_fields() -> Vec<F> {
    vec![
        f("width", "Width", Fk::OptNum(1.0, 0, 1.0, 16_384.0), "null"),
        f("height", "Height", Fk::OptNum(1.0, 0, 1.0, 16_384.0), "null"),
        f("duration", "Length", Fk::OptNum(0.05, 2, 0.01, 100_000.0), "null"),
        f("background", "Background", Fk::OptColor, "null"),
    ]
}

/// Ready formulas for the expressions tab: (label, text).
pub const SNIPPETS: &[(&str, &str)] = &[
    ("Wiggle", "wiggle(2, 30)"),
    ("Turn with time", "time * 90"),
    ("Bob up and down", "value + sin(time * 4) * 20"),
    ("Loop the keyframes", "loopOut('cycle')"),
    ("Ping-pong the keyframes", "loopOut('pingpong')"),
    ("Follow another thing", "prop('id', 'x') + 100"),
    ("Step every second", "floor(time)"),
];
