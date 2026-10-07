//! My effect: a kimchi video effect (one picture in, one out). Made with `plugin.new`; change
//! everything below. `kimchi-cli plugin.guide` explains the SDK.

use kimchi_plugin::prelude::*;

pub struct MyEffect {
    amount: f32,
    tint: [f32; 4],
}

impl Plugin for MyEffect {
    // The id is stored in projects: never change it once the plugin is used.
    const INFO: Info = Info::effect("local.plugins.my-effect", "My effect", "Me", "Stylize").describe("Pushes the picture towards a colour.");

    fn params() -> Vec<Param> {
        vec![
            number("Amount", 0.0, 100.0, 50.0).unit("%").hint("How far towards the colour."),
            color("Colour", [1.0, 0.55, 0.2, 1.0]),
        ]
    }

    fn new(_setup: &Setup) -> Self {
        Self { amount: 0.5, tint: [1.0, 0.55, 0.2, 1.0] }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.amount = value.number() as f32 / 100.0,
            1 => self.tint = value.color(),
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        let (k, tint) = (self.amount, self.tint);
        // Rows are drawn on all of kimchi's threads.
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let c = input.straight(x, y);
                let m = [0, 1, 2].map(|i| c[i] + (tint[i] - c[i]) * k);
                *px = from_straight([m[0], m[1], m[2], c[3]]);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn it_draws() {
        let mut bench = Bench::<MyEffect>::new();
        bench.set("Amount", 100.0);
        let out = bench.effect(&Image::solid(4, 4, [0, 0, 0, 255]));
        assert_eq!(out.pixel(0, 0), [255, 140, 51, 255]);
    }
}
