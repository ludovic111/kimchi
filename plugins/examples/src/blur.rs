//! A Gaussian-looking blur for the examples: three box blurs in a row, done along rows, then along
//! the columns of the transposed buffer, on every thread. The radius may be fractional (the box's
//! end samples are weighted), so an animated radius grows smoothly.

use kimchi_plugin::prelude::*;

/// Pixels of 4 floats, row after row.
pub struct Buffer {
    pub width: usize,
    pub height: usize,
    pub px: Vec<[f32; 4]>,
}

impl Buffer {
    /// `frame` made `factor` times smaller (each pixel the mean of a `factor`×`factor` block),
    /// each source pixel first read in linear light and passed through `f`.
    pub fn downsample(frame: &Frame, factor: usize, ctx: &RenderCtx, f: impl Fn([f32; 4]) -> [f32; 4] + Sync) -> Self {
        let factor = factor.max(1);
        let (width, height) = (frame.width().div_ceil(factor), frame.height().div_ceil(factor));
        let mut px = vec![[0.0; 4]; width * height];
        ctx.chunks(&mut px, width.max(1), |y, row| {
            for (x, out) in row.iter_mut().enumerate() {
                let mut sum = [0.0f32; 4];
                let mut n = 0.0;
                for sy in y * factor..((y + 1) * factor).min(frame.height()) {
                    for sx in x * factor..((x + 1) * factor).min(frame.width()) {
                        let c = f(frame.linear(sx, sy));
                        for i in 0..4 {
                            sum[i] += c[i];
                        }
                        n += 1.0;
                    }
                }
                *out = sum.map(|s| s / n.max(1.0));
            }
        });
        Self { width, height, px }
    }

    /// Blurs in place: `sigma` is the Gaussian's deviation in this buffer's pixels.
    pub fn blur(&mut self, sigma: f32, ctx: &RenderCtx) {
        if sigma < 0.2 || self.width == 0 || self.height == 0 {
            return;
        }
        // Three boxes of radius r have the variance of a Gaussian of deviation sigma when
        // 3·((2r+1)² − 1)/12 = sigma².
        let radius = (((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).max(0.0);
        self.blur_rows(radius, ctx);
        let mut t = self.transposed(ctx);
        t.blur_rows(radius, ctx);
        *self = t.transposed(ctx);
    }

    fn blur_rows(&mut self, radius: f32, ctx: &RenderCtx) {
        let w = self.width;
        ctx.chunks(&mut self.px, w, |_, row| {
            let mut tmp = vec![[0.0; 4]; row.len()];
            for _ in 0..3 {
                box_line(row, &mut tmp, radius);
                row.copy_from_slice(&tmp);
            }
        });
    }

    fn transposed(&self, ctx: &RenderCtx) -> Self {
        let (w, h) = (self.width, self.height);
        let mut px = vec![[0.0; 4]; w * h];
        ctx.chunks(&mut px, h, |x, column| {
            for (y, out) in column.iter_mut().enumerate() {
                *out = self.px[y * w + x];
            }
        });
        Self { width: h, height: w, px }
    }

    /// The buffer at frame pixel (x, y) of a frame `factor` times larger, bilinear.
    pub fn at(&self, x: usize, y: usize, factor: usize) -> [f32; 4] {
        let k = 1.0 / factor.max(1) as f32;
        let fx = ((x as f32 + 0.5) * k - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = ((y as f32 + 0.5) * k - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.px[y * self.width + x];
        mix(mix(p(x0, y0), p(x1, y0), tx), mix(p(x0, y1), p(x1, y1), tx), ty)
    }
}

/// One box blur of `src` into `dst`, edges extended.
fn box_line(src: &[[f32; 4]], dst: &mut [[f32; 4]], radius: f32) {
    let n = src.len() as isize;
    if n == 0 {
        return;
    }
    let r = radius.floor() as isize;
    let frac = radius - r as f32;
    let norm = 1.0 / ((2 * r + 1) as f32 + 2.0 * frac);
    let at = |i: isize| src[i.clamp(0, n - 1) as usize];
    let mut sum = [0.0f32; 4];
    for k in -r..=r {
        let c = at(k);
        for i in 0..4 {
            sum[i] += c[i];
        }
    }
    for (i, out) in dst.iter_mut().enumerate() {
        let i = i as isize;
        let (before, after) = (at(i - r - 1), at(i + r + 1));
        for c in 0..4 {
            out[c] = (sum[c] + frac * (before[c] + after[c])) * norm;
        }
        let (add, drop) = (at(i + r + 1), at(i - r));
        for c in 0..4 {
            sum[c] += add[c] - drop[c];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_keeps_the_total_and_spreads() {
        let mut src = vec![[0.0; 4]; 21];
        src[10] = [1.0; 4];
        let mut dst = vec![[0.0; 4]; 21];
        box_line(&src, &mut dst, 2.5);
        let total: f32 = dst.iter().map(|p| p[0]).sum();
        assert!((total - 1.0).abs() < 1e-4, "{total}");
        assert!(dst[10][0] > dst[12][0] && dst[12][0] > dst[13][0] && dst[13][0] > 0.0 && dst[14][0] == 0.0);
    }
}
