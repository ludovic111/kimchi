//! My generator: a kimchi video generator (it draws a picture from nothing; put it on a solid
//! clip). Made with `plugin.new`; change everything below. `kimchi-cli plugin.guide` explains the
//! SDK.

use kimchi_plugin::prelude::*;

pub struct MyGenerator {
    size: f64,
    ink: [f32; 4],
}

impl Plugin for MyGenerator {
    // The id is stored in projects: never change it once the plugin is used.
    const INFO: Info = Info::generator("local.plugins.my-generator", "My generator", "Me", "Generate").describe("Stripes that move across the picture.").animated();

    fn params() -> Vec<Param> {
        vec![number("Width", 2.0, 400.0, 40.0).unit("px"), color("Colour", [1.0, 1.0, 1.0, 1.0])]
    }

    fn new(_setup: &Setup) -> Self {
        Self { size: 40.0, ink: [1.0, 1.0, 1.0, 1.0] }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.size = value.number(),
            1 => self.ink = value.color(),
            _ => {}
        }
    }

    fn render(&mut self, _inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        // Sizes in project pixels, scaled to the frame (the preview is smaller than the export).
        let w = ctx.px(self.size).max(1.0);
        let shift = (ctx.time as f32 * w * 2.0) % (w * 2.0);
        let ink = from_straight(self.ink);
        ctx.rows(output, |_, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let on = ((x as f32 + shift) / w) as i64 % 2 == 0;
                *px = if on { ink } else { [0, 0, 0, 0] };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::Bench;

    #[test]
    fn it_draws() {
        let mut bench = Bench::<MyGenerator>::new();
        bench.set("Width", 2.0);
        let out = bench.generate(8, 2);
        assert_eq!(out.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(out.pixel(2, 0), [0, 0, 0, 0]);
    }
}
