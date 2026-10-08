//! Halftone: the picture as a grid of ink dots that grow with darkness, like newsprint. In
//! colour (a dot per channel on rotated screens, as in print) or in one ink on paper.

use kimchi_plugin::prelude::*;

pub struct Halftone {
    size: f64,
    angle: f64,
    colour: bool,
    ink: [f32; 4],
    paper: [f32; 4],
    softness: f32,
}

impl Plugin for Halftone {
    const INFO: Info = Info::effect("xyz.lsuite.kimchi.halftone", "Halftone", crate::VENDOR, "Stylize").describe("The picture as a grid of ink dots that grow with darkness, like print.");

    fn params() -> Vec<Param> {
        vec![
            number("Dot size", 2.0, 80.0, 10.0).unit("px").hint("Distance between dots, in project pixels."),
            angle("Angle", 45.0).hint("How the screen of dots is turned."),
            choice("Ink", &["One ink", "Colour"], 0),
            color("Ink colour", [0.04, 0.04, 0.04, 1.0]),
            color("Paper", [0.96, 0.95, 0.92, 1.0]),
            number("Softness", 0.0, 100.0, 30.0).unit("%").hint("How soft the dots' edges are."),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { size: 10.0, angle: 45.0, colour: false, ink: [0.04, 0.04, 0.04, 1.0], paper: [0.96, 0.95, 0.92, 1.0], softness: 0.3 }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.size = value.number(),
            1 => self.angle = value.number(),
            2 => self.colour = value.choice() == 1,
            3 => self.ink = value.color(),
            4 => self.paper = value.color(),
            5 => self.softness = value.number() as f32 / 100.0,
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        let cell = ctx.px(self.size).max(2.0);
        let soft = (self.softness * 0.5 + 0.02) * cell;
        let (ink, paper, colour, base) = (self.ink, self.paper, self.colour, self.angle as f32);
        // Print's screen angles for cyan, magenta and yellow-ish (the key ink uses `base`).
        let screens = [base + 15.0, base + 75.0, base];
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let src = input.straight(x, y);
                if src[3] <= 0.0 {
                    *px = [0, 0, 0, 0];
                    continue;
                }
                let out = if colour {
                    // One screen per channel: a dot of "missing" light, so dark channels get big dots.
                    let mut c = [1.0f32; 3];
                    for (i, angle) in screens.iter().enumerate() {
                        let (cx, cy, d) = cell_centre(fx, fy, cell, *angle);
                        let sample = input.sample(cx, cy);
                        let lit = straight_channel(sample, i);
                        let radius = (1.0 - lit).sqrt() * cell * std::f32::consts::FRAC_1_SQRT_2;
                        c[i] = 1.0 - coverage(d, radius, soft);
                    }
                    [c[0], c[1], c[2], src[3]]
                } else {
                    let (cx, cy, d) = cell_centre(fx, fy, cell, base);
                    let sample = input.sample(cx, cy);
                    let a = sample[3].max(1e-6);
                    let l = luma([sample[0] / a, sample[1] / a, sample[2] / a, 1.0]);
                    let radius = (1.0 - l).max(0.0).sqrt() * cell * std::f32::consts::FRAC_1_SQRT_2;
                    let k = coverage(d, radius, soft);
                    let m = mix(paper, ink, k);
                    [m[0], m[1], m[2], src[3]]
                };
                *px = from_straight(out);
            }
        });
    }
}

/// The centre of the screen cell `(x, y)` falls in, on a screen turned by `angle` degrees, and
/// the distance from it.
fn cell_centre(x: f32, y: f32, cell: f32, angle: f32) -> (f32, f32, f32) {
    let (s, c) = angle.to_radians().sin_cos();
    // Into the screen's frame, snap to the cell's centre, and back.
    let (u, v) = (x * c + y * s, -x * s + y * c);
    let (cu, cv) = (((u / cell).floor() + 0.5) * cell, ((v / cell).floor() + 0.5) * cell);
    let (cx, cy) = (cu * c - cv * s, cu * s + cv * c);
    let d = ((u - cu).powi(2) + (v - cv).powi(2)).sqrt();
    (cx, cy, d)
}

/// Channel `i` of a premultiplied sample, straight.
fn straight_channel(p: [f32; 4], i: usize) -> f32 {
    if p[3] <= 0.0 { 1.0 } else { (p[i] / p[3]).clamp(0.0, 1.0) }
}

/// How much of a dot of `radius` covers a point `d` from its centre, with a soft edge.
fn coverage(d: f32, radius: f32, soft: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    1.0 - smoothstep(radius - soft, radius + soft, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn dark_makes_big_dots_and_white_stays_paper() {
        let mut b = Bench::<Halftone>::new();
        b.set("Dot size", 8.0).set("Softness", 0.0);
        let white = b.effect(&Image::solid(32, 32, [255, 255, 255, 255]));
        let paper = white.mean();
        assert!(paper[0] > 230.0 && paper[2] > 220.0, "white is paper: {paper:?}");
        let black = b.effect(&Image::solid(32, 32, [0, 0, 0, 255]));
        assert!(black.mean()[0] < 80.0, "black is mostly ink: {:?}", black.mean());
        let grey = b.effect(&Image::solid(32, 32, [128, 128, 128, 255]));
        let g = grey.mean()[0];
        assert!(g > black.mean()[0] && g < paper[0], "grey is between: {g}");
        // Transparent stays transparent.
        assert_eq!(b.effect(&Image::solid(8, 8, [0, 0, 0, 0])).pixel(3, 3), [0, 0, 0, 0]);
        assert!(grey.is_premultiplied());
    }

    #[test]
    fn colour_screens_keep_the_hue() {
        let mut b = Bench::<Halftone>::new();
        b.set("Ink", 1.0).set("Dot size", 6.0);
        let red = b.effect(&Image::solid(36, 36, [230, 20, 20, 255])).mean();
        assert!(red[0] > red[1] + 60.0 && red[0] > red[2] + 60.0, "{red:?}");
    }
}
