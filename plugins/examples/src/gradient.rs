//! Gradient: a generator that fills its clip with a linear or radial blend between two colours,
//! from one point to another (points are fractions of the picture, so they need no scaling).

use kimchi_plugin::prelude::*;

pub struct Gradient {
    radial: bool,
    start: [f32; 2],
    end: [f32; 2],
    from: [f32; 4],
    to: [f32; 4],
}

impl Plugin for Gradient {
    const INFO: Info = Info::generator("xyz.lsuite.kimchi.gradient", "Gradient", crate::VENDOR, "Generate").describe("A linear or radial blend between two colours.");

    fn params() -> Vec<Param> {
        vec![
            choice("Shape", &["Linear", "Radial"], 0),
            point("Start", [0.5, 0.0]).hint("Where the first colour is (the centre for radial)."),
            point("End", [0.5, 1.0]).hint("Where the second colour is reached."),
            color("Start colour", [0.98, 0.42, 0.36, 1.0]),
            color("End colour", [0.20, 0.10, 0.45, 1.0]),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { radial: false, start: [0.5, 0.0], end: [0.5, 1.0], from: [0.98, 0.42, 0.36, 1.0], to: [0.20, 0.10, 0.45, 1.0] }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.radial = value.choice() == 1,
            1 => self.start = value.point(),
            2 => self.end = value.point(),
            3 => self.from = value.color(),
            4 => self.to = value.color(),
            _ => {}
        }
    }

    fn render(&mut self, _inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let (w, h) = (output.width() as f32, output.height() as f32);
        let a = [self.start[0] * w, self.start[1] * h];
        let b = [self.end[0] * w, self.end[1] * h];
        let d = [b[0] - a[0], b[1] - a[1]];
        let len2 = (d[0] * d[0] + d[1] * d[1]).max(1e-6);
        let (radial, from, to) = (self.radial, self.from, self.to);
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let p = [x as f32 + 0.5 - a[0], y as f32 + 0.5 - a[1]];
                let t = if radial { ((p[0] * p[0] + p[1] * p[1]) / len2).sqrt() } else { (p[0] * d[0] + p[1] * d[1]) / len2 };
                let c = mix(from, to, t.clamp(0.0, 1.0));
                // Half a step of noise hides banding in slow gradients; it is the same each frame.
                let dither = (random::unit(7, x as u32, y as u32) - 0.5) / 255.0;
                *px = from_straight([c[0] + dither, c[1] + dither, c[2] + dither, c[3]]);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::Bench;

    #[test]
    fn blends_from_start_to_end() {
        let mut b = Bench::<Gradient>::new();
        b.set("Start colour", [0.0, 0.0, 0.0, 1.0]).set("End colour", [1.0, 1.0, 1.0, 1.0]);
        let out = b.generate(10, 100);
        assert!(out.pixel(5, 0)[0] <= 2 && out.pixel(5, 99)[0] >= 253);
        assert!((out.pixel(5, 50)[0] as i32 - 128).abs() <= 3);
        // Radial: the centre is the start colour, the corners the end colour.
        b.set("Shape", 1).set("Start", [0.5, 0.5]).set("End", [1.0, 0.5]);
        let out = b.generate(100, 100);
        assert!(out.pixel(50, 50)[0] <= 3 && out.pixel(0, 0)[0] >= 253);
    }
}
