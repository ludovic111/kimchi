//! A bench for plugin authors: run a plugin the way kimchi does — through the C ABI, with frames,
//! parameter values checked against the manifest, and the host's thread pool — and look at what
//! comes out. Nothing here needs kimchi.
//!
//! ```
//! use kimchi_plugin::prelude::*;
//! use kimchi_plugin::testing::{Bench, Image};
//! # struct Invert { amount: f32 }
//! # impl Plugin for Invert {
//! #     const INFO: Info = Info::effect("com.example.invert", "Invert", "Example", "Color");
//! #     fn params() -> Vec<Param> { vec![number("Amount", 0.0, 100.0, 100.0).unit("%")] }
//! #     fn new(_: &Setup) -> Self { Self { amount: 1.0 } }
//! #     fn set_param(&mut self, _: usize, v: Value) { self.amount = v.number() as f32 / 100.0 }
//! #     fn render(&mut self, inputs: &[Frame], out: &mut FrameMut, ctx: &RenderCtx) {
//! #         let (input, k) = (inputs[0], self.amount);
//! #         ctx.rows(out, |y, row| for (x, px) in row.iter_mut().enumerate() {
//! #             let c = input.straight(x, y);
//! #             *px = from_straight([c[0] + (1.0 - 2.0 * c[0]) * k, c[1] + (1.0 - 2.0 * c[1]) * k, c[2] + (1.0 - 2.0 * c[2]) * k, c[3]]);
//! #         });
//! #     }
//! # }
//! let mut bench = Bench::<Invert>::new();
//! let out = bench.effect(&Image::solid(8, 8, [255, 0, 0, 255]));
//! assert_eq!(out.pixel(3, 3), [0, 255, 255, 255]);
//! bench.set("Amount", 0.0);
//! assert_eq!(bench.effect(&Image::solid(8, 8, [255, 0, 0, 255])).pixel(0, 0), [255, 0, 0, 255]);
//! ```

use std::ffi::c_void;
use std::marker::PhantomData;
use std::sync::Mutex;

use crate::ffi::{self, Handle, Manifest, RawFrame, RawHost, RawRenderCtx, RawSetup, RawValue};
use crate::plugin::{Kind, Plugin};

/// A picture owned by a test: premultiplied RGBA bytes, row after row, no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image {}×{} (mean {:?})", self.width, self.height, self.mean())
    }
}

impl Image {
    /// Transparent.
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0; width * height * 4] }
    }

    /// Every pixel `px` (premultiplied bytes).
    pub fn solid(width: usize, height: usize, px: [u8; 4]) -> Self {
        Self::from_fn(width, height, |_, _| px)
    }

    pub fn from_fn(width: usize, height: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Self {
        let mut data = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&f(x, y));
            }
        }
        Self { width, height, data }
    }

    /// An opaque test card: red rising left to right, green top to bottom, blue a checker of
    /// `cell`-pixel squares.
    pub fn card(width: usize, height: usize, cell: usize) -> Self {
        let cell = cell.max(1);
        Self::from_fn(width, height, |x, y| {
            let r = (x * 255 / width.max(2).saturating_sub(1).max(1)) as u8;
            let g = (y * 255 / height.max(2).saturating_sub(1).max(1)) as u8;
            let b = if (x / cell + y / cell) % 2 == 0 { 255 } else { 0 };
            [r, g, b, 255]
        })
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// The mean of each channel, 0…255.
    pub fn mean(&self) -> [f64; 4] {
        let n = (self.width * self.height).max(1) as f64;
        let mut sum = [0.0; 4];
        for px in self.data.chunks_exact(4) {
            for i in 0..4 {
                sum[i] += px[i] as f64;
            }
        }
        sum.map(|s| s / n)
    }

    /// Mean absolute difference from `other` (the same size), 0…255.
    pub fn difference(&self, other: &Image) -> f64 {
        assert_eq!((self.width, self.height), (other.width, other.height), "comparing images of different sizes");
        let total: u64 = self.data.iter().zip(&other.data).map(|(a, b)| a.abs_diff(*b) as u64).sum();
        total as f64 / self.data.len().max(1) as f64
    }

    /// No colour channel above alpha (a valid premultiplied picture).
    pub fn is_premultiplied(&self) -> bool {
        self.data.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
    }

    fn raw(&self) -> RawFrame {
        RawFrame { data: self.data.as_ptr() as *mut u8, width: self.width as u32, height: self.height as u32, stride: (self.width * 4) as u32, format: ffi::FORMAT_RGBA8_PREMULTIPLIED }
    }
}

/// A value for [`Bench::set`].
#[derive(Clone, Debug, PartialEq)]
pub enum TestValue {
    Number(f64),
    Toggle(bool),
    Color([f32; 4]),
    Point([f32; 2]),
    Text(String),
}

impl From<f64> for TestValue {
    fn from(v: f64) -> Self {
        TestValue::Number(v)
    }
}
impl From<i32> for TestValue {
    fn from(v: i32) -> Self {
        TestValue::Number(v as f64)
    }
}
impl From<bool> for TestValue {
    fn from(v: bool) -> Self {
        TestValue::Toggle(v)
    }
}
impl From<[f32; 4]> for TestValue {
    fn from(v: [f32; 4]) -> Self {
        TestValue::Color(v)
    }
}
impl From<[f32; 2]> for TestValue {
    fn from(v: [f32; 2]) -> Self {
        TestValue::Point(v)
    }
}
impl From<&str> for TestValue {
    fn from(v: &str) -> Self {
        TestValue::Text(v.into())
    }
}

/// The bench's host: its thread pool is `std::thread::scope` over the machine's cores, its log is
/// kept for [`Bench::logs`].
#[repr(C)]
struct BenchHost {
    raw: RawHost,
    logs: Mutex<Vec<String>>,
}

unsafe extern "C" fn bench_parallel(_host: *const RawHost, count: u32, job: ffi::Job, data: *mut c_void) {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(count as usize).max(1);
    let next = std::sync::atomic::AtomicU32::new(0);
    let data = data as usize;
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= count {
                        break;
                    }
                    // SAFETY: the SDK's job and data, valid until `parallel` returns.
                    unsafe { job(data as *mut c_void, i) };
                }
            });
        }
    });
}

unsafe extern "C" fn bench_log(host: *const RawHost, _level: u32, message: *const u8, len: usize) {
    // SAFETY: `host` is the `raw` field at the start of a `BenchHost`; the message is `len` bytes.
    let host = unsafe { &*(host as *const BenchHost) };
    let text = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message, len) }).into_owned();
    host.logs.lock().unwrap_or_else(|e| e.into_inner()).push(text);
}

/// One instance of `P`, driven through its vtable as kimchi drives it.
pub struct Bench<P: Plugin> {
    table: &'static ffi::PluginVTable,
    manifest: Manifest,
    handle: Option<Handle>,
    /// Values given before the instance was made, and every value since (for a new instance).
    values: Vec<Option<TestValue>>,
    host: Box<BenchHost>,
    /// Seconds from the clip's start of the next frame.
    pub time: f64,
    pub fps: f64,
    /// Frame pixels per project pixel.
    pub scale: f32,
    pub draft: bool,
    _plugin: PhantomData<P>,
}

impl<P: Plugin> Default for Bench<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Plugin> Bench<P> {
    /// Reads and checks the manifest as a host would (panics with the reason if it is wrong).
    pub fn new() -> Self {
        let table: &'static ffi::PluginVTable = Box::leak(Box::new(ffi::vtable::<P>()));
        let manifest = ffi::read_manifest(table).unwrap_or_else(|e| panic!("{}: {e}", P::INFO.name));
        let values = vec![None; manifest.params.len()];
        let host = Box::new(BenchHost { raw: RawHost { size: size_of::<RawHost>() as u32, reserved: 0, parallel: bench_parallel, log: bench_log }, logs: Mutex::new(vec![]) });
        Self { table, manifest, handle: None, values, host, time: 0.0, fps: 30.0, scale: 1.0, draft: false, _plugin: PhantomData }
    }

    /// What the host reads from the plugin.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Sets a parameter by name, as kimchi would. Panics on an unknown name or a value the plugin
    /// refuses, because a test that sets nothing proves nothing.
    pub fn set(&mut self, name: &str, value: impl Into<TestValue>) -> &mut Self {
        let index = self.manifest.params.iter().position(|p| p.name == name).unwrap_or_else(|| {
            let names: Vec<&str> = self.manifest.params.iter().map(|p| p.name.as_str()).collect();
            panic!("{} has no parameter `{name}` (it has {})", P::INFO.name, names.join(", "))
        });
        let value = value.into();
        if let Some(h) = &mut self.handle {
            send(h, index, &value).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        self.values[index] = Some(value);
        self
    }

    /// The next frames are drawn at `time` seconds from the clip's start.
    pub fn at(&mut self, time: f64) -> &mut Self {
        self.time = time;
        self
    }

    /// Lines the plugin wrote with [`crate::RenderCtx::log`].
    pub fn logs(&self) -> Vec<String> {
        self.host.logs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Runs an effect on `input`. Panics if the plugin fails.
    pub fn effect(&mut self, input: &Image) -> Image {
        self.render(&[input], input.width, input.height, 0.0).unwrap_or_else(|e| panic!("{}: {e}", P::INFO.name))
    }

    /// Runs a generator at this size.
    pub fn generate(&mut self, width: usize, height: usize) -> Image {
        self.render(&[], width, height, 0.0).unwrap_or_else(|e| panic!("{}: {e}", P::INFO.name))
    }

    /// Runs a transition from `from` to `to` at `progress` (0…1).
    pub fn transition(&mut self, from: &Image, to: &Image, progress: f32) -> Image {
        self.render(&[from, to], from.width, from.height, progress).unwrap_or_else(|e| panic!("{}: {e}", P::INFO.name))
    }

    /// One frame through the ABI, with what the plugin reports when it fails (a panic is caught,
    /// as in kimchi, and comes back as `Err` here and on every later call).
    pub fn render(&mut self, inputs: &[&Image], width: usize, height: usize, progress: f32) -> Result<Image, String> {
        if self.handle.is_none() {
            let mut h = Handle::new(self.table, &RawSetup::new(width as u32, height as u32, self.fps))?;
            for (i, v) in self.values.iter().enumerate() {
                match v {
                    Some(v) => send(&mut h, i, v)?,
                    None => send_default(&mut h, i, &self.manifest)?,
                }
            }
            self.handle = Some(h);
        }
        let mut out = Image::new(width, height);
        let raw: Vec<RawFrame> = inputs.iter().map(|i| i.raw()).collect();
        let ctx = RawRenderCtx::new(self.time, self.fps, self.scale, progress, self.draft, &self.host.raw);
        self.handle.as_mut().expect("made above").render(&raw, &out.raw(), &ctx)?;
        if P::INFO.kind == Kind::Effect && !out.is_premultiplied() {
            return Err("the output has colour above alpha: frames are premultiplied".into());
        }
        out.data.shrink_to_fit();
        Ok(out)
    }
}

fn send(h: &mut Handle, index: usize, v: &TestValue) -> Result<(), String> {
    let raw = match v {
        TestValue::Number(n) => RawValue::number(*n),
        TestValue::Toggle(b) => RawValue::toggle(*b),
        TestValue::Color(c) => RawValue::color(*c),
        TestValue::Point(p) => RawValue::point(*p),
        TestValue::Text(t) => RawValue::text(t),
    };
    h.set_param(index as u32, &raw)
}

/// The manifest's default for parameter `index`, sent as kimchi sends it.
fn send_default(h: &mut Handle, index: usize, m: &Manifest) -> Result<(), String> {
    let p = &m.params[index];
    let d = &p.default;
    let nums = |n: usize| -> Vec<f32> { d.as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_else(|| vec![0.0; n]) };
    let v = match p.kind {
        crate::ParamKind::Toggle => TestValue::Toggle(d.as_bool().unwrap_or(false)),
        crate::ParamKind::Color => {
            let c = nums(4);
            TestValue::Color([c[0], c[1], c[2], c[3]])
        }
        crate::ParamKind::Point => {
            let c = nums(2);
            TestValue::Point([c[0], c[1]])
        }
        crate::ParamKind::Text | crate::ParamKind::File => TestValue::Text(d.as_str().unwrap_or("").into()),
        _ => TestValue::Number(d.as_f64().unwrap_or(0.0)),
    };
    send(h, index, &v)
}
