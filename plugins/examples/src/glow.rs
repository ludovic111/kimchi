//! Glow (bloom): the bright parts of the picture spill light around them.
//!
//! How: take the pixels brighter than the threshold (in linear light, so the light adds up the
//! way light does), blur them at a lower resolution (a wide blur on a quarter-size buffer looks
//! the same and costs a sixteenth), and add them back, tinted.

use kimchi_plugin::prelude::*;

use crate::blur::Buffer;

pub struct Glow {
    threshold: f32,
    radius: f64,
    intensity: f32,
    tint: [f32; 4],
    only: bool,
}

impl Plugin for Glow {
    const INFO: Info = Info::effect("xyz.lsuite.kimchi.glow", "Glow", crate::VENDOR, "Blur & Glow").describe("Bright parts spill soft light around them (bloom).");

    fn params() -> Vec<Param> {
        vec![
            number("Threshold", 0.0, 100.0, 60.0).unit("%").hint("How bright a pixel must be to glow."),
            number("Radius", 0.0, 400.0, 40.0).unit("px").hint("How far the light spreads, in project pixels."),
            number("Intensity", 0.0, 500.0, 100.0).unit("%"),
            color("Tint", [1.0, 1.0, 1.0, 1.0]),
            toggle("Glow only", false).hint("Show the light without the picture under it."),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { threshold: 0.6, radius: 40.0, intensity: 1.0, tint: [1.0; 4], only: false }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.threshold = value.number() as f32 / 100.0,
            1 => self.radius = value.number(),
            2 => self.intensity = value.number() as f32 / 100.0,
            3 => self.tint = value.color(),
            4 => self.only = value.toggle(),
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        // The blur's deviation in frame pixels; the buffer is made smaller as it grows, so the
        // blur stays a few buffer pixels wide (quicker still for draft frames).
        let sigma = ctx.px(self.radius) / 2.0;
        let target = if ctx.draft { 3.0 } else { 6.0 };
        let factor = ((sigma / target).floor() as usize).clamp(1, 32);
        let (threshold, knee) = (self.threshold, 0.1f32);
        let mut light = Buffer::downsample(&input, factor, ctx, |c| {
            if c[3] <= 0.0 {
                return [0.0; 4];
            }
            // Brightness of the straight colour; a soft knee keeps the edge of the threshold smooth.
            let l = luma(c) / c[3];
            let k = smoothstep(threshold - knee, threshold + knee, l);
            c.map(|v| v * k)
        });
        light.blur(sigma / factor as f32, ctx);
        let tint = [0, 1, 2].map(|i| srgb_to_linear(self.tint[i]));
        let (gain, only) = (self.intensity, self.only);
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let g = light.at(x, y, factor);
                let base = if only { [0.0; 4] } else { input.linear(x, y) };
                let mut c = [0.0f32; 4];
                for i in 0..3 {
                    c[i] = base[i] + g[i] * gain * tint[i];
                }
                // Light over transparent parts makes them show: alpha covers the colour added.
                c[3] = (base[3] + g[3] * gain * (1.0 - base[3])).max(c[0].max(c[1]).max(c[2])).min(1.0);
                *px = from_linear(c);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    /// A white square in the middle of a black picture.
    fn spot() -> Image {
        Image::from_fn(64, 64, |x, y| if (28..36).contains(&x) && (28..36).contains(&y) { [255; 4] } else { [0, 0, 0, 255] })
    }

    #[test]
    fn light_spreads_from_bright_parts() {
        let mut b = Bench::<Glow>::new();
        let out = b.effect(&spot());
        // Around the square it is lighter than black, far away it isn't.
        assert!(out.pixel(24, 32)[0] > 20, "{:?}", out.pixel(24, 32));
        assert_eq!(out.pixel(2, 2), [0, 0, 0, 255]);
        // The square stays white.
        assert_eq!(out.pixel(32, 32), [255; 4]);
        // Nothing above the threshold, nothing changes.
        b.set("Threshold", 100.0);
        let dim = Image::solid(16, 16, [100, 100, 100, 255]);
        assert!(b.effect(&dim).difference(&dim) < 0.5);
    }

    #[test]
    fn looks_the_same_at_preview_size() {
        // The radius is in project pixels: half the size with scale 0.5 spreads half as far.
        let mut full = Bench::<Glow>::new();
        let big = full.effect(&Image::from_fn(128, 128, |x, y| if (56..72).contains(&x) && (56..72).contains(&y) { [255; 4] } else { [0, 0, 0, 255] }));
        let mut small = Bench::<Glow>::new();
        small.scale = 0.5;
        let half = small.effect(&spot());
        let (a, b) = (big.pixel(48, 64)[0] as i32, half.pixel(24, 32)[0] as i32);
        assert!((a - b).abs() < 25, "{a} vs {b}");
    }

    #[test]
    fn glow_only_and_tint() {
        let mut b = Bench::<Glow>::new();
        b.set("Glow only", true).set("Tint", [1.0, 0.0, 0.0, 1.0]);
        let out = b.effect(&spot());
        let p = out.pixel(24, 32);
        assert!(p[0] > 0 && p[1] == 0 && p[2] == 0, "{p:?}");
        assert!(out.pixel(2, 2)[3] < 5, "far from the light it is transparent");
    }
}
