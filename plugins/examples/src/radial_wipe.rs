//! Radial wipe: a transition that sweeps the incoming picture in like a clock hand, from an angle,
//! around a centre, with a soft edge.

use kimchi_plugin::prelude::*;

pub struct RadialWipe {
    centre: [f32; 2],
    start: f64,
    softness: f32,
    clockwise: bool,
}

impl Plugin for RadialWipe {
    const INFO: Info = Info::transition("xyz.lsuite.kimchi.radial-wipe", "Radial wipe", crate::VENDOR).describe("The next picture sweeps in like a clock hand.");

    fn params() -> Vec<Param> {
        vec![
            point("Centre", [0.5, 0.5]),
            angle("Start", 0.0).hint("Where the hand starts: 0° is straight up."),
            number("Softness", 0.0, 100.0, 8.0).unit("%"),
            choice("Direction", &["Clockwise", "Counter-clockwise"], 0),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { centre: [0.5, 0.5], start: 0.0, softness: 0.08, clockwise: true }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.centre = value.point(),
            1 => self.start = value.number(),
            2 => self.softness = value.number() as f32 / 100.0,
            3 => self.clockwise = value.choice() == 0,
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let (from, to) = (inputs[0], inputs[1]);
        let (cx, cy) = (self.centre[0] * output.width() as f32, self.centre[1] * output.height() as f32);
        let (start, soft, clockwise) = (self.start as f32, self.softness.max(1e-4) * 0.25, self.clockwise);
        // The edge runs a little past both ends so the softness is gone at 0 and at 1.
        let edge = ctx.progress * (1.0 + soft);
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                // Degrees clockwise from straight up, then as a fraction of the turn from `start`.
                let a = dx.atan2(-dy).to_degrees();
                let mut turn = ((a - start).rem_euclid(360.0)) / 360.0;
                if !clockwise {
                    turn = (1.0 - turn).rem_euclid(1.0);
                }
                let k = ((edge - turn) / soft).clamp(0.0, 1.0);
                let (a, b) = (from.pixel(x, y), to.pixel(x, y));
                *px = std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * k + 0.5) as u8);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn sweeps_from_one_picture_to_the_other() {
        let (red, blue) = (Image::solid(40, 40, [255, 0, 0, 255]), Image::solid(40, 40, [0, 0, 255, 255]));
        let mut b = Bench::<RadialWipe>::new();
        assert_eq!(b.transition(&red, &blue, 0.0), red);
        assert_eq!(b.transition(&red, &blue, 1.0), blue);
        // A quarter of the way, clockwise from the top: the top right is blue, the top left red.
        let q = b.transition(&red, &blue, 0.3);
        assert_eq!(q.pixel(30, 5), [0, 0, 255, 255]);
        assert_eq!(q.pixel(10, 5), [255, 0, 0, 255]);
        b.set("Direction", 1);
        let q = b.transition(&red, &blue, 0.3);
        assert_eq!(q.pixel(10, 5), [0, 0, 255, 255]);
    }
}
