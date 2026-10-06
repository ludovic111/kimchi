//! Frames and colour.
//!
//! A frame is what kimchi's compositor works in: rows of pixels, each four bytes, red, green,
//! blue and alpha, **premultiplied** (the colour is already multiplied by the alpha, so a pixel
//! never has more colour than alpha) and **sRGB** encoded (the bytes are what a screen shows, not
//! light intensities). That is the cheapest form to blend and what tiny-skia uses.
//!
//! Most effects can work on the bytes directly. When the maths needs more:
//!
//! - [`Frame::straight`] gives a pixel unpremultiplied, as 0…1 floats: the colour as people pick
//!   it (a duotone, a colour key, a hue shift).
//! - [`Frame::linear`] gives it in linear light, still premultiplied: what blurs, glows and mixes
//!   should average so they don't darken (a bloom done in sRGB looks muddy).
//! - [`Frame::sample`] reads between pixels with bilinear filtering (distortions, scaling).
//!
//! and the matching `set_*` on [`FrameMut`] write them back.

use std::sync::OnceLock;

/// A picture to read: an input of [`crate::Plugin::render`].
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    /// Bytes from one row to the next.
    stride: usize,
}

/// The picture a plugin draws.
pub struct FrameMut<'a> {
    data: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
}

macro_rules! readers {
    () => {
        pub fn width(&self) -> usize {
            self.width
        }

        pub fn height(&self) -> usize {
            self.height
        }

        /// Bytes from the start of one row to the next (at least `width * 4`).
        pub fn stride(&self) -> usize {
            self.stride
        }

        /// The premultiplied bytes at (x, y). Panics outside the frame.
        #[inline]
        pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
            assert!(x < self.width && y < self.height, "pixel ({x}, {y}) is outside the {}×{} frame", self.width, self.height);
            let i = y * self.stride + x * 4;
            [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
        }

        /// The pixel at (x, y) clamped to the frame's edges (any x, y).
        #[inline]
        pub fn clamped(&self, x: isize, y: isize) -> [u8; 4] {
            if self.width == 0 || self.height == 0 {
                return [0; 4];
            }
            self.pixel(x.clamp(0, self.width as isize - 1) as usize, y.clamp(0, self.height as isize - 1) as usize)
        }

        /// One row's pixels.
        #[inline]
        pub fn row(&self, y: usize) -> &[[u8; 4]] {
            let start = y * self.stride;
            as_pixels(&self.data[start..start + self.width * 4])
        }

        /// The pixel at (x, y) unpremultiplied, sRGB, 0…1.
        #[inline]
        pub fn straight(&self, x: usize, y: usize) -> [f32; 4] {
            straight(self.pixel(x, y))
        }

        /// The pixel at (x, y) in linear light, premultiplied, 0…1.
        #[inline]
        pub fn linear(&self, x: usize, y: usize) -> [f32; 4] {
            linear(self.pixel(x, y))
        }

        /// The picture at a point between pixels (pixel centres are at `x + 0.5`), filtered
        /// bilinearly, premultiplied sRGB 0…1. Outside the frame the edge pixels continue.
        pub fn sample(&self, x: f32, y: f32) -> [f32; 4] {
            self.sample_with(x, y, true)
        }

        /// [`Self::sample`], transparent outside the frame.
        pub fn sample_or_clear(&self, x: f32, y: f32) -> [f32; 4] {
            self.sample_with(x, y, false)
        }

        fn sample_with(&self, x: f32, y: f32, clamp: bool) -> [f32; 4] {
            if self.width == 0 || self.height == 0 || !x.is_finite() || !y.is_finite() {
                return [0.0; 4];
            }
            let (fx, fy) = (x - 0.5, y - 0.5);
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let (x0, y0) = (x0 as isize, y0 as isize);
            let at = |x: isize, y: isize| -> [f32; 4] {
                if !clamp && (x < 0 || y < 0 || x >= self.width as isize || y >= self.height as isize) {
                    return [0.0; 4];
                }
                let p = self.clamped(x, y);
                [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
            };
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            let mut out = [0.0; 4];
            for i in 0..4 {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bottom = c[i] + (d[i] - c[i]) * tx;
                out[i] = (top + (bottom - top) * ty) / 255.0;
            }
            out
        }
    };
}

impl<'a> Frame<'a> {
    /// A frame over `data`: `height` rows of `stride` bytes, `width` pixels used in each.
    /// Panics if `data` is too short.
    pub fn new(data: &'a [u8], width: usize, height: usize, stride: usize) -> Self {
        check(data.len(), width, height, stride);
        Self { data, width, height, stride }
    }

    readers!();

    /// The bytes, row after row.
    pub fn bytes(&self) -> &'a [u8] {
        self.data
    }
}

impl<'a> FrameMut<'a> {
    /// See [`Frame::new`].
    pub fn new(data: &'a mut [u8], width: usize, height: usize, stride: usize) -> Self {
        check(data.len(), width, height, stride);
        Self { data, width, height, stride }
    }

    readers!();

    /// This frame, to read.
    pub fn as_frame(&self) -> Frame<'_> {
        Frame { data: self.data, width: self.width, height: self.height, stride: self.stride }
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr()
    }

    /// Sets the premultiplied bytes at (x, y). Panics outside the frame.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, px: [u8; 4]) {
        assert!(x < self.width && y < self.height, "pixel ({x}, {y}) is outside the {}×{} frame", self.width, self.height);
        let i = y * self.stride + x * 4;
        self.data[i..i + 4].copy_from_slice(&px);
    }

    /// Sets (x, y) from unpremultiplied sRGB 0…1.
    #[inline]
    pub fn set_straight(&mut self, x: usize, y: usize, c: [f32; 4]) {
        self.set(x, y, from_straight(c));
    }

    /// Sets (x, y) from premultiplied linear light 0…1.
    #[inline]
    pub fn set_linear(&mut self, x: usize, y: usize, c: [f32; 4]) {
        self.set(x, y, from_linear(c));
    }

    /// Sets (x, y) from premultiplied sRGB 0…1 (what [`Frame::sample`] gives).
    #[inline]
    pub fn set_float(&mut self, x: usize, y: usize, c: [f32; 4]) {
        self.set(x, y, from_float(c));
    }

    /// One row's pixels, to change.
    #[inline]
    pub fn row_mut(&mut self, y: usize) -> &mut [[u8; 4]] {
        let start = y * self.stride;
        as_pixels_mut(&mut self.data[start..start + self.width * 4])
    }

    /// Every pixel set to `px`.
    pub fn fill(&mut self, px: [u8; 4]) {
        for y in 0..self.height {
            self.row_mut(y).fill(px);
        }
    }

    /// Copies `src` (the same size) into this frame.
    pub fn copy_from(&mut self, src: &Frame) {
        assert!(src.width == self.width && src.height == self.height, "copying a {}×{} frame into a {}×{} one", src.width, src.height, self.width, self.height);
        for y in 0..self.height {
            let from = src.row(y);
            self.row_mut(y).copy_from_slice(from);
        }
    }
}

fn check(len: usize, width: usize, height: usize, stride: usize) {
    assert!(stride >= width * 4, "a row of {width} pixels needs {} bytes, the stride is {stride}", width * 4);
    let need = if height == 0 { 0 } else { (height - 1) * stride + width * 4 };
    assert!(len >= need, "a {width}×{height} frame needs {need} bytes, got {len}");
}

fn as_pixels(bytes: &[u8]) -> &[[u8; 4]] {
    // SAFETY: [u8; 4] has the alignment of u8; the length is a multiple of 4.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const [u8; 4], bytes.len() / 4) }
}

fn as_pixels_mut(bytes: &mut [u8]) -> &mut [[u8; 4]] {
    // SAFETY: as above.
    unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut [u8; 4], bytes.len() / 4) }
}

/// sRGB byte → linear light, by table.
fn to_linear_table() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))
}

/// Linear light (in 1/4096 steps) → sRGB byte, by table.
fn from_linear_table() -> &'static [u8; 4097] {
    static T: OnceLock<[u8; 4097]> = OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|i| (linear_to_srgb(i as f32 / 4096.0) * 255.0 + 0.5) as u8))
}

/// The sRGB curve: an encoded value 0…1 to linear light 0…1.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Linear light 0…1 to an encoded sRGB value 0…1.
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// An sRGB byte as linear light (fast, by table).
#[inline]
pub fn byte_to_linear(b: u8) -> f32 {
    to_linear_table()[b as usize]
}

/// Linear light 0…1 as an sRGB byte (fast, by table; clamped).
#[inline]
pub fn linear_to_byte(v: f32) -> u8 {
    let i = (v.clamp(0.0, 1.0) * 4096.0 + 0.5) as usize;
    from_linear_table()[i.min(4096)]
}

/// A 0…1 value as a byte, rounded and clamped (NaN is 0).
#[inline]
pub fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Premultiplied bytes → straight sRGB 0…1.
#[inline]
pub fn straight(px: [u8; 4]) -> [f32; 4] {
    let a = px[3] as f32 / 255.0;
    if px[3] == 0 {
        return [0.0; 4];
    }
    [px[0] as f32 / 255.0 / a, px[1] as f32 / 255.0 / a, px[2] as f32 / 255.0 / a, a].map(|v| v.min(1.0))
}

/// Straight sRGB 0…1 → premultiplied bytes.
#[inline]
pub fn from_straight(c: [f32; 4]) -> [u8; 4] {
    let a = c[3].clamp(0.0, 1.0);
    [to_byte(c[0] * a), to_byte(c[1] * a), to_byte(c[2] * a), to_byte(a)]
}

/// Premultiplied bytes → premultiplied linear light 0…1. The colour is unpremultiplied,
/// linearised, then multiplied again, so the result is exact for any alpha.
#[inline]
pub fn linear(px: [u8; 4]) -> [f32; 4] {
    if px[3] == 255 {
        return [byte_to_linear(px[0]), byte_to_linear(px[1]), byte_to_linear(px[2]), 1.0];
    }
    let s = straight(px);
    let a = s[3];
    [srgb_to_linear(s[0]) * a, srgb_to_linear(s[1]) * a, srgb_to_linear(s[2]) * a, a]
}

/// Premultiplied linear light 0…1 → premultiplied bytes.
#[inline]
pub fn from_linear(c: [f32; 4]) -> [u8; 4] {
    let a = c[3].clamp(0.0, 1.0);
    if a <= 0.0 {
        return [0; 4];
    }
    if a >= 1.0 {
        return [linear_to_byte(c[0]), linear_to_byte(c[1]), linear_to_byte(c[2]), 255];
    }
    let s = |v: f32| linear_to_srgb((v / a).clamp(0.0, 1.0)) * a;
    [to_byte(s(c[0])), to_byte(s(c[1])), to_byte(s(c[2])), to_byte(a)]
}

/// Premultiplied sRGB 0…1 → bytes, keeping colour within alpha.
#[inline]
pub fn from_float(c: [f32; 4]) -> [u8; 4] {
    let a = to_byte(c[3]);
    [to_byte(c[0]).min(a), to_byte(c[1]).min(a), to_byte(c[2]).min(a), a]
}

/// Rec. 709 luma of straight or linear RGB.
#[inline]
pub fn luma(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// `a` towards `b` by `t` (0…1), every channel.
#[inline]
pub fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// Smooth 0 → 1 between `edge0` and `edge1` (Hermite, like GLSL's).
#[inline]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_round_trips() {
        for a in [0u8, 1, 64, 128, 255] {
            for v in [0u8, 3, 77, 128, 200, 255] {
                let px = [v.min(a), (v / 2).min(a), 0, a];
                let back = from_linear(linear(px));
                for i in 0..4 {
                    assert!((back[i] as i32 - px[i] as i32).abs() <= 1, "{px:?} → {back:?}");
                }
                let back = from_straight(straight(px));
                for i in 0..4 {
                    assert!((back[i] as i32 - px[i] as i32).abs() <= 1, "{px:?} → {back:?}");
                }
            }
        }
        for b in 0..=255u8 {
            assert_eq!(linear_to_byte(byte_to_linear(b)), b);
        }
    }

    #[test]
    fn sampling_is_bilinear_and_clamped() {
        // Two pixels: black, white.
        let data = [0, 0, 0, 255, 255, 255, 255, 255];
        let f = Frame::new(&data, 2, 1, 8);
        assert_eq!(f.sample(0.5, 0.5), [0.0, 0.0, 0.0, 1.0]);
        assert!((f.sample(1.0, 0.5)[0] - 0.5).abs() < 1e-6);
        assert_eq!(f.sample(10.0, 0.5)[0], 1.0);
        assert_eq!(f.sample_or_clear(-3.0, 0.5), [0.0; 4]);
        assert_eq!(f.clamped(-4, 9), [0, 0, 0, 255]);
    }

    #[test]
    #[should_panic(expected = "needs 32 bytes")]
    fn short_buffers_are_refused() {
        let data = [0u8; 16];
        let _ = Frame::new(&data, 4, 2, 16);
    }
}
