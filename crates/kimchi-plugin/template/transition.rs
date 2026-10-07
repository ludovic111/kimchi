//! My transition: a kimchi video transition (the outgoing and incoming pictures in, one out, as
//! `ctx.progress` goes from 0 to 1). Made with `plugin.new`; change everything below.
//! `kimchi-cli plugin.guide` explains the SDK.

use kimchi_plugin::prelude::*;

pub struct MyTransition {
    softness: f32,
}

impl Plugin for MyTransition {
    // The id is stored in projects: never change it once the plugin is used.
    const INFO: Info = Info::transition("local.plugins.my-transition", "My transition", "Me").describe("The next picture wipes in from the left.");

    fn params() -> Vec<Param> {
        vec![number("Softness", 0.0, 100.0, 10.0).unit("%")]
    }

    fn new(_setup: &Setup) -> Self {
        Self { softness: 0.1 }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        if index == 0 {
            self.softness = value.number() as f32 / 100.0;
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let (from, to) = (inputs[0], inputs[1]);
        let width = output.width() as f32;
        let soft = self.softness.max(1e-3);
        // The edge runs past both sides so it is all `from` at 0 and all `to` at 1.
        let edge = ctx.progress * (1.0 + soft) - soft / 2.0;
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let t = 1.0 - smoothstep(edge - soft / 2.0, edge + soft / 2.0, (x as f32 + 0.5) / width);
                let (a, b) = (from.pixel(x, y), to.pixel(x, y));
                *px = std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn it_goes_from_one_to_the_other() {
        let mut bench = Bench::<MyTransition>::new();
        let (a, b) = (Image::solid(8, 2, [255, 0, 0, 255]), Image::solid(8, 2, [0, 0, 255, 255]));
        assert_eq!(bench.transition(&a, &b, 0.0).pixel(4, 0), [255, 0, 0, 255]);
        assert_eq!(bench.transition(&a, &b, 1.0).pixel(4, 0), [0, 0, 255, 255]);
    }
}
