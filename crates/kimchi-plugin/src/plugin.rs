//! The plugin contract: what a plugin is ([`Info`]), the parameters it shows ([`Param`]), the
//! values it is given ([`Value`]), what a frame is drawn for ([`RenderCtx`]) and the trait itself.

use serde::{Deserialize, Serialize};

use crate::frame::{Frame, FrameMut};

/// What a plugin does with pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Changes a picture: `inputs` has one frame, the clip's picture.
    Effect,
    /// Makes a picture from nothing: `inputs` is empty and the output starts transparent.
    Generator,
    /// Mixes two pictures: `inputs` holds the outgoing then the incoming one, and
    /// [`RenderCtx::progress`] goes from 0 to 1 through the transition.
    Transition,
}

/// Who a plugin is. `id` must never change once people use the plugin: projects store it (as
/// `kimchi:<id>`), and a project opened on a computer without it keeps its settings under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// Reverse-DNS: `com.vendor.name`.
    pub id: &'static str,
    /// What the plugin picker shows.
    pub name: &'static str,
    pub vendor: &'static str,
    /// The picker's group: Blur & Glow, Stylize, Color, Distort, Noise & Grain, Generate,
    /// Transition, Utility, or any label of your own.
    pub category: &'static str,
    /// One short sentence: what it does to a picture.
    pub description: &'static str,
    pub version: &'static str,
    pub kind: Kind,
    /// The output changes with time even when no parameter does (grain, flicker, noise that
    /// moves). The host keeps the result for still pictures only when this is false.
    pub animated: bool,
}

impl Info {
    const fn new(kind: Kind, id: &'static str, name: &'static str, vendor: &'static str, category: &'static str) -> Self {
        Self { id, name, vendor, category, description: "", version: "1.0.0", kind, animated: false }
    }

    /// An effect: one picture in, one out.
    pub const fn effect(id: &'static str, name: &'static str, vendor: &'static str, category: &'static str) -> Self {
        Self::new(Kind::Effect, id, name, vendor, category)
    }

    /// A generator: draws a picture of its own.
    pub const fn generator(id: &'static str, name: &'static str, vendor: &'static str, category: &'static str) -> Self {
        Self::new(Kind::Generator, id, name, vendor, category)
    }

    /// A transition from one picture to another.
    pub const fn transition(id: &'static str, name: &'static str, vendor: &'static str) -> Self {
        Self::new(Kind::Transition, id, name, vendor, "Transition")
    }

    pub const fn describe(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    pub const fn version(mut self, version: &'static str) -> Self {
        self.version = version;
        self
    }

    /// See [`Info::animated`].
    pub const fn animated(mut self) -> Self {
        self.animated = true;
        self
    }
}

/// The type of a parameter, which decides the control the inspector shows for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    /// A slider between `min` and `max`, with a unit. Takes keyframes.
    Number,
    /// A whole number between `min` and `max`. Takes keyframes.
    Integer,
    /// On or off.
    Toggle,
    /// One of `choices`; the value is its index.
    Choice,
    /// A colour well: straight (not premultiplied) sRGB red, green, blue and alpha, 0…1.
    Color,
    /// A place on the picture, `[x, y]` from its top left corner as fractions of its width and
    /// height (0…1, so it doesn't depend on the size the picture is drawn at). Takes keyframes.
    Point,
    /// Degrees, clockwise. Takes keyframes.
    Angle,
    /// A line of text.
    Text,
    /// A file on this computer (a picture, a LUT…), by its absolute path; `extensions` filters the
    /// file picker.
    File,
}

/// A parameter's default, of whatever its kind needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Initial {
    Number(f64),
    Toggle(bool),
    Color([f32; 4]),
    Point([f32; 2]),
    Text(&'static str),
}

/// One parameter: a control in the inspector, a value stored in the project. Make them with
/// [`number`], [`integer`], [`toggle`], [`choice`], [`color`], [`point`], [`angle`], [`text`]
/// and [`file`], then refine with the builder methods.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    /// What the value is stored and keyframed by (`plugins.<slot>.<name>`): never rename it.
    pub name: &'static str,
    /// What the inspector shows; the name when empty.
    pub label: &'static str,
    pub kind: ParamKind,
    pub default: Initial,
    /// The range of numbers, integers and angles. The host clamps values to it.
    pub min: f64,
    pub max: f64,
    /// The unit after a number ("px", "%", "°", "s"); empty for none.
    pub unit: &'static str,
    /// Choice labels, in order (the value is an index into them).
    pub choices: &'static [&'static str],
    /// File extensions the file picker offers (without the dot).
    pub extensions: &'static [&'static str],
    /// The fold it is shown under in the inspector; empty for the top.
    pub group: &'static str,
    /// A tooltip.
    pub hint: &'static str,
}

const fn base(name: &'static str, kind: ParamKind, default: Initial) -> Param {
    Param { name, label: "", kind, default, min: 0.0, max: 1.0, unit: "", choices: &[], extensions: &[], group: "", hint: "" }
}

/// A number from `min` to `max`.
pub const fn number(name: &'static str, min: f64, max: f64, default: f64) -> Param {
    Param { min, max, ..base(name, ParamKind::Number, Initial::Number(default)) }
}

/// A whole number from `min` to `max`.
pub const fn integer(name: &'static str, min: i64, max: i64, default: i64) -> Param {
    Param { min: min as f64, max: max as f64, ..base(name, ParamKind::Integer, Initial::Number(default as f64)) }
}

/// A switch.
pub const fn toggle(name: &'static str, default: bool) -> Param {
    base(name, ParamKind::Toggle, Initial::Toggle(default))
}

/// One of `choices`; `default` is an index.
pub const fn choice(name: &'static str, choices: &'static [&'static str], default: usize) -> Param {
    let max = if choices.is_empty() { 0.0 } else { (choices.len() - 1) as f64 };
    Param { min: 0.0, max, choices, ..base(name, ParamKind::Choice, Initial::Number(default as f64)) }
}

/// A colour, straight sRGB `[r, g, b, a]` from 0 to 1.
pub const fn color(name: &'static str, default: [f32; 4]) -> Param {
    base(name, ParamKind::Color, Initial::Color(default))
}

/// A place on the picture, `[x, y]` as fractions of its width and height from the top left.
pub const fn point(name: &'static str, default: [f32; 2]) -> Param {
    base(name, ParamKind::Point, Initial::Point(default))
}

/// An angle in degrees (-360 to 360 unless [`Param::range`] says otherwise).
pub const fn angle(name: &'static str, default: f64) -> Param {
    Param { min: -360.0, max: 360.0, unit: "°", ..base(name, ParamKind::Angle, Initial::Number(default)) }
}

/// A line of text.
pub const fn text(name: &'static str, default: &'static str) -> Param {
    base(name, ParamKind::Text, Initial::Text(default))
}

/// A file, by absolute path (empty: none chosen).
pub const fn file(name: &'static str, extensions: &'static [&'static str]) -> Param {
    Param { extensions, ..base(name, ParamKind::File, Initial::Text("")) }
}

impl Param {
    pub const fn label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }

    pub const fn group(mut self, group: &'static str) -> Self {
        self.group = group;
        self
    }

    pub const fn hint(mut self, hint: &'static str) -> Self {
        self.hint = hint;
        self
    }

    pub const fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// The default as the value `set_param` would get.
    pub fn default_value(&self) -> Value<'static> {
        match (self.kind, self.default) {
            (ParamKind::Integer, Initial::Number(n)) => Value::Integer(n.round() as i64),
            (ParamKind::Choice, Initial::Number(n)) => Value::Choice(n.max(0.0) as usize),
            (_, Initial::Number(n)) => Value::Number(n),
            (_, Initial::Toggle(b)) => Value::Toggle(b),
            (_, Initial::Color(c)) => Value::Color(c),
            (_, Initial::Point(p)) => Value::Point(p),
            (_, Initial::Text(t)) => Value::Text(t),
        }
    }
}

/// A parameter's value, as [`Plugin::set_param`] gets it: always of the parameter's kind and in
/// its range (the host checks), so a plugin can take it with the accessor for its kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    /// Numbers and angles.
    Number(f64),
    Integer(i64),
    Toggle(bool),
    /// An index into the choices.
    Choice(usize),
    /// Straight sRGB `[r, g, b, a]`, 0…1.
    Color([f32; 4]),
    /// `[x, y]`, fractions of the picture from its top left.
    Point([f32; 2]),
    /// Text and file paths.
    Text(&'a str),
}

impl Value<'_> {
    /// Numbers, angles, integers, choices and toggles as a number.
    pub fn number(&self) -> f64 {
        match *self {
            Value::Number(n) => n,
            Value::Integer(i) => i as f64,
            Value::Choice(i) => i as f64,
            Value::Toggle(b) => b as u8 as f64,
            _ => 0.0,
        }
    }

    pub fn integer(&self) -> i64 {
        match *self {
            Value::Integer(i) => i,
            Value::Choice(i) => i as i64,
            _ => self.number().round() as i64,
        }
    }

    pub fn toggle(&self) -> bool {
        match *self {
            Value::Toggle(b) => b,
            _ => self.number() >= 0.5,
        }
    }

    pub fn choice(&self) -> usize {
        self.integer().max(0) as usize
    }

    pub fn color(&self) -> [f32; 4] {
        match *self {
            Value::Color(c) => c,
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    pub fn point(&self) -> [f32; 2] {
        match *self {
            Value::Point(p) => p,
            _ => [0.5, 0.5],
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Value::Text(t) => t,
            _ => "",
        }
    }
}

/// Facts about the frames an instance will draw, given when it is made. Frames may still change
/// size later (the preview and an export draw at different sizes): read sizes from the frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setup {
    pub width: u32,
    pub height: u32,
    /// Frames per second of the timeline.
    pub fps: f64,
}

/// What a frame is drawn for.
#[derive(Clone, Copy)]
pub struct RenderCtx<'a> {
    /// Seconds from the start of the clip.
    pub time: f64,
    /// Frames per second of the output.
    pub fps: f64,
    /// Frame pixels per project pixel: multiply sizes in project pixels ("px" parameters) by it,
    /// so an effect looks the same in the small preview and the full-size export.
    pub scale: f32,
    /// Transitions: 0 at the start, 1 at the end.
    pub progress: f32,
    /// A fast frame (playback, scrubbing): a plugin may draw a cheaper approximation.
    pub draft: bool,
    pub(crate) host: Option<&'a crate::ffi::RawHost>,
}

impl RenderCtx<'_> {
    /// A context with no host (for drawing outside kimchi, e.g. in your own tests).
    pub fn new(time: f64, fps: f64) -> RenderCtx<'static> {
        RenderCtx { time, fps, scale: 1.0, progress: 0.0, draft: false, host: None }
    }

    /// The frame number this time falls on (frame 0 starts the clip). The same in the preview and
    /// the export, so randomness seeded with it matches too.
    pub fn frame(&self) -> i64 {
        let fps = if self.fps.is_finite() && self.fps > 0.0 { self.fps } else { 30.0 };
        (self.time * fps).round() as i64
    }

    /// `px` project pixels in frame pixels.
    pub fn px(&self, px: f64) -> f32 {
        (px * self.scale as f64) as f32
    }

    /// A seed for this frame and `salt` (a seed parameter, a layer number): the same frame
    /// always gives the same seed. See [`crate::random`].
    pub fn seed(&self, salt: u64) -> u64 {
        crate::random::hash(salt ^ 0x6b69_6d63_6869, self.frame() as u64)
    }

    /// Runs `job(i)` for every `i` in `0..count`, spread over the host's threads (kimchi uses all
    /// cores), and returns when all are done. Without a host it runs them in turn.
    pub fn parallel(&self, count: usize, job: impl Fn(usize) + Sync) {
        crate::ffi::parallel(self.host, count, &job);
    }

    /// Calls `f(y, row)` for every row of `frame`, rows spread over the host's threads. `row` is
    /// the row's pixels, premultiplied RGBA bytes.
    pub fn rows(&self, frame: &mut FrameMut, f: impl Fn(usize, &mut [[u8; 4]]) + Sync) {
        let (w, h, stride) = (frame.width(), frame.height(), frame.stride());
        if h == 0 || w == 0 {
            return;
        }
        let base = frame.as_mut_ptr() as usize;
        // A few bands per thread: rows next to each other share cache lines.
        let band = (h / 64).max(1);
        let bands = h.div_ceil(band);
        self.parallel(bands, |b| {
            for y in b * band..((b + 1) * band).min(h) {
                // SAFETY: every row is visited by exactly one job, inside the frame's buffer.
                let row = unsafe { std::slice::from_raw_parts_mut((base + y * stride) as *mut [u8; 4], w) };
                f(y, row);
            }
        });
    }

    /// Calls `f(i, chunk)` for consecutive `len`-long pieces of `data` (the last may be shorter),
    /// spread over the host's threads: for working buffers of your own (a blur's rows).
    pub fn chunks<T: Send>(&self, data: &mut [T], len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
        let len = len.max(1);
        let total = data.len();
        let base = data.as_mut_ptr() as usize;
        self.parallel(total.div_ceil(len), |i| {
            let start = i * len;
            let end = (start + len).min(total);
            // SAFETY: the pieces don't overlap, and `data` is borrowed until every job is done.
            let chunk = unsafe { std::slice::from_raw_parts_mut((base as *mut T).add(start), end - start) };
            f(i, chunk);
        });
    }

    /// Writes a line to kimchi's log (info level). For debugging; don't log every frame.
    pub fn log(&self, message: &str) {
        crate::ffi::log(self.host, 2, message);
    }
}

impl std::fmt::Debug for RenderCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderCtx").field("time", &self.time).field("fps", &self.fps).field("scale", &self.scale).field("progress", &self.progress).field("draft", &self.draft).finish()
    }
}

/// A kimchi video plugin. kimchi makes one instance per place the plugin is used (a clip's
/// plugin slot), gives it every parameter's value through `set_param` before the first frame and
/// whenever one changes (keyframes change them between frames), and calls `render` for each frame,
/// possibly from a different thread each time (never two at once).
///
/// Frames are premultiplied RGBA, 8 bits per channel, sRGB, like the rest of kimchi's
/// compositor; [`Frame`] has helpers to read them straight or in linear light, and to sample
/// between pixels.
pub trait Plugin: Send + 'static {
    const INFO: Info;

    /// The parameters, in a fixed order: `set_param` gets their index in this list.
    fn params() -> Vec<Param>;

    fn new(setup: &Setup) -> Self;

    /// `value` is of the parameter's kind and in its range.
    fn set_param(&mut self, index: usize, value: Value);

    /// Draws `output` (the size of the inputs). Effects get the picture in `inputs[0]`;
    /// generators get no inputs and a transparent output; transitions get the outgoing and the
    /// incoming picture. A panic here is caught: the host shows the picture unchanged.
    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx);
}
