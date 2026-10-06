//! Duotone: the picture in two colours, its shadows in one and its highlights in the other.

use kimchi_plugin::prelude::*;

pub struct Duotone {
    shadows: [f32; 4],
    highlights: [f32; 4],
    contrast: f32,
    amount: f32,
}

impl Plugin for Duotone {
    const INFO: Info = Info::effect("xyz.lsuite.kimchi.duotone", "Duotone", crate::VENDOR, "Color").describe("Shadows in one colour, highlights in another.");

    fn params() -> Vec<Param> {
        vec![
            color("Shadows", [0.10, 0.09, 0.35, 1.0]),
            color("Highlights", [1.0, 0.55, 0.42, 1.0]),
            number("Contrast", -100.0, 100.0, 0.0).unit("%"),
            number("Amount", 0.0, 100.0, 100.0).unit("%").hint("How much of the duotone shows over the picture."),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { shadows: [0.10, 0.09, 0.35, 1.0], highlights: [1.0, 0.55, 0.42, 1.0], contrast: 0.0, amount: 1.0 }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.shadows = value.color(),
            1 => self.highlights = value.color(),
            2 => self.contrast = value.number() as f32 / 100.0,
            3 => self.amount = value.number() as f32 / 100.0,
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        let (lo, hi, amount) = (self.shadows, self.highlights, self.amount);
        // Contrast as a slope around mid grey: 1 is unchanged, up to 3 or down to 0.
        let slope = if self.contrast >= 0.0 { 1.0 + 2.0 * self.contrast } else { 1.0 + self.contrast };
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let c = input.straight(x, y);
                let l = ((luma(c) - 0.5) * slope + 0.5).clamp(0.0, 1.0);
                let tone = mix(lo, hi, l);
                let out = mix(c, tone, amount);
                *px = from_straight([out[0], out[1], out[2], c[3]]);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn black_and_white_become_the_two_colours() {
        let mut b = Bench::<Duotone>::new();
        b.set("Shadows", [0.0, 0.0, 1.0, 1.0]).set("Highlights", [1.0, 1.0, 0.0, 1.0]);
        assert_eq!(b.effect(&Image::solid(2, 2, [0, 0, 0, 255])).pixel(0, 0), [0, 0, 255, 255]);
        assert_eq!(b.effect(&Image::solid(2, 2, [255; 4])).pixel(0, 0), [255, 255, 0, 255]);
        // Half transparent stays half transparent.
        assert_eq!(b.effect(&Image::solid(2, 2, [0, 0, 0, 128])).pixel(1, 1), [0, 0, 128, 128]);
        b.set("Amount", 0.0);
        let card = Image::card(8, 8, 2);
        assert!(b.effect(&card).difference(&card) < 0.6);
    }
}
