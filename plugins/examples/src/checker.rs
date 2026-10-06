//! Checker: a generator for checkerboards and stripes, turned by an angle around a centre,
//! smoothed at the edges by drawing four samples per pixel.

use kimchi_plugin::prelude::*;

pub struct Checker {
    stripes: bool,
    size: f64,
    angle: f64,
    centre: [f32; 2],
    a: [u8; 4],
    b: [u8; 4],
}

impl Plugin for Checker {
    const INFO: Info = Info::generator("xyz.lsuite.kimchi.checker", "Checker", crate::VENDOR, "Generate").describe("A checkerboard or stripes in two colours.");

    fn params() -> Vec<Param> {
        vec![
            choice("Pattern", &["Checker", "Stripes"], 0),
            number("Size", 2.0, 1000.0, 80.0).unit("px").hint("Width of a square or stripe, in project pixels."),
            angle("Angle", 0.0),
            point("Centre", [0.5, 0.5]).hint("The point the pattern turns around."),
            color("Colour A", [0.96, 0.96, 0.94, 1.0]),
            color("Colour B", [0.12, 0.12, 0.14, 1.0]),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { stripes: false, size: 80.0, angle: 0.0, centre: [0.5, 0.5], a: from_straight([0.96, 0.96, 0.94, 1.0]), b: from_straight([0.12, 0.12, 0.14, 1.0]) }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.stripes = value.choice() == 1,
            1 => self.size = value.number(),
            2 => self.angle = value.number(),
            3 => self.centre = value.point(),
            4 => self.a = from_straight(value.color()),
            5 => self.b = from_straight(value.color()),
            _ => {}
        }
    }

    fn render(&mut self, _inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let size = ctx.px(self.size).max(0.5);
        let (sin, cos) = (self.angle as f32).to_radians().sin_cos();
        let (cx, cy) = (self.centre[0] * output.width() as f32, self.centre[1] * output.height() as f32);
        let (stripes, a, b) = (self.stripes, self.a, self.b);
        // Which colour a point is (true: A). The pattern is centred: a square's corner sits on
        // the centre, so turning it keeps the centre where it is.
        let is_a = |x: f32, y: f32| {
            let (dx, dy) = (x - cx, y - cy);
            let (u, v) = ((dx * cos + dy * sin) / size, (dy * cos - dx * sin) / size);
            let (i, j) = (u.floor() as i64, v.floor() as i64);
            if stripes { i.rem_euclid(2) == 0 } else { (i + j).rem_euclid(2) == 0 }
        };
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let n = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)].iter().filter(|(ox, oy)| is_a(x as f32 + ox, y as f32 + oy)).count() as u16;
                *px = std::array::from_fn(|i| ((a[i] as u16 * n + b[i] as u16 * (4 - n) + 2) / 4) as u8);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::Bench;

    #[test]
    fn squares_alternate() {
        let mut b = Bench::<Checker>::new();
        b.set("Size", 10.0).set("Colour A", [1.0, 1.0, 1.0, 1.0]).set("Colour B", [0.0, 0.0, 0.0, 1.0]);
        let out = b.generate(40, 40);
        // The centre (20, 20) is a corner: the squares on either side differ.
        assert_eq!(out.pixel(25, 25), [255; 4]);
        assert_eq!(out.pixel(15, 25), [0, 0, 0, 255]);
        assert_eq!(out.pixel(15, 15), [255; 4]);
        b.set("Pattern", 1);
        let out = b.generate(40, 40);
        assert_eq!(out.pixel(25, 5), out.pixel(25, 35), "stripes run top to bottom");
        // Turned 90°, stripes run across.
        b.set("Angle", 90.0);
        let out = b.generate(40, 40);
        assert_eq!(out.pixel(5, 25), out.pixel(35, 25));
    }
}
