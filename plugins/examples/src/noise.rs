//! Noise: a generator for soft clouds of noise between two colours (fractal value noise), still
//! or drifting slowly.

use kimchi_plugin::prelude::*;

pub struct Noise {
    size: f64,
    detail: u32,
    speed: f64,
    seed: u64,
    a: [f32; 4],
    b: [f32; 4],
}

impl Plugin for Noise {
    const INFO: Info = Info::generator("xyz.lsuite.kimchi.noise", "Noise", crate::VENDOR, "Generate").describe("Soft clouds of noise in two colours, still or drifting.").animated();

    fn params() -> Vec<Param> {
        vec![
            number("Size", 2.0, 2000.0, 160.0).unit("px").hint("Size of the largest clouds, in project pixels."),
            integer("Detail", 1, 8, 4).hint("Layers of finer noise on top."),
            number("Drift", 0.0, 500.0, 0.0).unit("px/s").hint("How fast the clouds move."),
            integer("Seed", 0, 9999, 0),
            color("Colour A", [0.05, 0.05, 0.08, 1.0]),
            color("Colour B", [0.85, 0.85, 0.9, 1.0]),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { size: 160.0, detail: 4, speed: 0.0, seed: 0, a: [0.05, 0.05, 0.08, 1.0], b: [0.85, 0.85, 0.9, 1.0] }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.size = value.number(),
            1 => self.detail = value.integer() as u32,
            2 => self.speed = value.number(),
            3 => self.seed = value.integer() as u64,
            4 => self.a = value.color(),
            5 => self.b = value.color(),
            _ => {}
        }
    }

    fn render(&mut self, _inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        // Everything in project pixels, so the preview shows the export's clouds.
        let to_project = 1.0 / ctx.scale;
        let drift = (self.speed * ctx.time) as f32;
        let (size, detail, seed, a, b) = (self.size as f32, self.detail.max(1), self.seed, self.a, self.b);
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let (gx, gy) = ((x as f32 + 0.5) * to_project + drift, (y as f32 + 0.5) * to_project + drift * 0.37);
                let (mut sum, mut weight, mut cell, mut k) = (0.0, 0.0, size, 1.0);
                for octave in 0..detail {
                    sum += random::smooth(random::hash(seed, octave as u64), gx, gy, cell) * k;
                    weight += k;
                    cell *= 0.5;
                    k *= 0.5;
                }
                *px = from_straight(mix(a, b, sum / weight));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::Bench;

    #[test]
    fn clouds_vary_and_drift() {
        let mut b = Bench::<Noise>::new();
        b.set("Size", 16.0);
        let first = b.generate(64, 64);
        let (lo, hi) = first.data.chunks(4).fold((255, 0), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
        assert!(hi - lo > 60, "{lo}…{hi}");
        b.at(1.0);
        assert_eq!(b.generate(64, 64), first, "no drift: the same every frame");
        b.set("Drift", 20.0);
        assert!(b.generate(64, 64).difference(&first) > 5.0);
    }
}
