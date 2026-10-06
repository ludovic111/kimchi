//! Pixelate: the picture as big blocks, each the average colour of what it covers, drawn as
//! squares or as round dots.

use kimchi_plugin::prelude::*;

pub struct Pixelate {
    size: f64,
    dots: bool,
}

impl Plugin for Pixelate {
    const INFO: Info = Info::effect("xyz.lsuite.kimchi.pixelate", "Pixelate", crate::VENDOR, "Stylize").describe("Big blocks of the picture's average colours, square or round.");

    fn params() -> Vec<Param> {
        vec![
            number("Size", 1.0, 400.0, 24.0).unit("px").hint("Width of a block, in project pixels."),
            choice("Shape", &["Squares", "Dots"], 0),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { size: 24.0, dots: false }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.size = value.number(),
            1 => self.dots = value.choice() == 1,
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        let (w, h) = (input.width(), input.height());
        let block = (ctx.px(self.size).round() as usize).max(1);
        let (cols, rows) = (w.div_ceil(block), h.div_ceil(block));
        // The average of each block (premultiplied, so transparent pixels don't darken it).
        let mut means = vec![[0u8; 4]; cols * rows];
        ctx.chunks(&mut means, cols.max(1), |by, line| {
            for (bx, out) in line.iter_mut().enumerate() {
                let mut sum = [0u32; 4];
                let mut n = 0;
                for y in by * block..((by + 1) * block).min(h) {
                    for p in &input.row(y)[bx * block..((bx + 1) * block).min(w)] {
                        for i in 0..4 {
                            sum[i] += p[i] as u32;
                        }
                        n += 1;
                    }
                }
                *out = sum.map(|s| ((s + n / 2) / n.max(1)) as u8);
            }
        });
        let dots = self.dots;
        let r = block as f32 / 2.0;
        ctx.rows(output, |y, row| {
            let by = y / block;
            for (x, px) in row.iter_mut().enumerate() {
                let c = means[by * cols + x / block];
                if !dots {
                    *px = c;
                    continue;
                }
                // Coverage of a circle filling the block, one pixel of soft edge.
                let (dx, dy) = ((x % block) as f32 + 0.5 - r, (y % block) as f32 + 0.5 - r);
                let k = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
                *px = c.map(|v| (v as f32 * k + 0.5) as u8);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn blocks_are_averages() {
        let mut b = Bench::<Pixelate>::new();
        b.set("Size", 4.0);
        // Stripes of black and white, one pixel wide: each block is mid grey.
        let out = b.effect(&Image::from_fn(16, 8, |x, _| if x % 2 == 0 { [0, 0, 0, 255] } else { [255; 4] }));
        assert_eq!(out.pixel(0, 0), [128, 128, 128, 255]);
        assert_eq!(out.pixel(5, 6), out.pixel(4, 4));
        // Dots leave the corners empty.
        b.set("Shape", 1);
        let out = b.effect(&Image::solid(16, 16, [255; 4]));
        assert_eq!(out.pixel(0, 0), [0; 4]);
        assert_eq!(out.pixel(2, 2), [255; 4]);
    }
}
