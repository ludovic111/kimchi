//! Chromatic aberration: red and blue drawn a little apart from green, in one
//! direction or growing out from the centre like a cheap lens.

use kimchi_plugin::prelude::*;

pub struct ChromaticAberration {
    amount: f64,
    angle: f64,
    radial: bool,
}

impl Plugin for ChromaticAberration {
    const INFO: Info = Info::effect("xyz.lsuite.kimchi.chromatic-aberration", "Chromatic aberration", crate::VENDOR, "Stylize").describe("Red and blue shifted apart from green, like a cheap lens (chromatic aberration).");

    fn params() -> Vec<Param> {
        vec![
            number("Amount", 0.0, 200.0, 8.0).unit("px").hint("How far red and blue move, in project pixels."),
            angle("Angle", 0.0).hint("Which way red moves (blue goes the other way)."),
            choice("Mode", &["Direction", "From the centre"], 0),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { amount: 8.0, angle: 0.0, radial: false }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.amount = value.number(),
            1 => self.angle = value.number(),
            2 => self.radial = value.choice() == 1,
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        let d = ctx.px(self.amount);
        let (sin, cos) = (self.angle as f32).to_radians().sin_cos();
        let (cx, cy) = (input.width() as f32 / 2.0, input.height() as f32 / 2.0);
        let half_diagonal = (cx * cx + cy * cy).sqrt().max(1.0);
        let radial = self.radial;
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                // The shift at this pixel: fixed, or outwards and growing towards the corners.
                let (ox, oy) = if radial {
                    let (vx, vy) = (fx - cx, fy - cy);
                    let k = d / half_diagonal;
                    (vx * k, vy * k)
                } else {
                    (cos * d, sin * d)
                };
                let r = input.sample_or_clear(fx - ox, fy - oy);
                let g = input.sample_or_clear(fx, fy);
                let b = input.sample_or_clear(fx + ox, fy + oy);
                let a = r[3].max(g[3]).max(b[3]);
                *px = from_float([r[0], g[1], b[2], a]);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn red_and_blue_move_apart() {
        let mut b = Bench::<ChromaticAberration>::new();
        b.set("Amount", 3.0);
        // A white column at x = 10.
        let out = b.effect(&Image::from_fn(21, 4, |x, _| if x == 10 { [255; 4] } else { [0, 0, 0, 255] }));
        assert_eq!(out.pixel(13, 1), [255, 0, 0, 255], "red moved right");
        assert_eq!(out.pixel(7, 1), [0, 0, 255, 255], "blue moved left");
        assert_eq!(out.pixel(10, 1), [0, 255, 0, 255]);
        b.set("Amount", 0.0);
        let card = Image::card(20, 20, 4);
        assert_eq!(b.effect(&card), card);
    }
}
