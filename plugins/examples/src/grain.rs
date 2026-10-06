//! Film grain: noise like film's, strongest in the mid-tones, different on every frame and yet
//! the same every time a given frame is drawn (seeded by the frame number, see
//! `kimchi_plugin::random`), so the preview shows what the export will have.

use kimchi_plugin::prelude::*;

pub struct FilmGrain {
    amount: f32,
    size: f64,
    colour: bool,
    seed: u64,
    moving: bool,
}

impl Plugin for FilmGrain {
    // `animated`: the grain changes every frame even when no parameter does.
    const INFO: Info = Info::effect("xyz.lsuite.kimchi.film-grain", "Film grain", crate::VENDOR, "Noise & Grain").describe("Moving grain like film's, strongest in the mid-tones.").animated();

    fn params() -> Vec<Param> {
        vec![
            number("Amount", 0.0, 100.0, 30.0).unit("%"),
            number("Size", 0.5, 12.0, 1.5).unit("px").hint("Grain size, in project pixels."),
            toggle("Colour", false).hint("Grain of its own in each colour, like colour film."),
            toggle("Moving", true).hint("New grain every frame; off keeps one pattern."),
            integer("Seed", 0, 9999, 0).hint("Another seed gives other grain."),
        ]
    }

    fn new(_: &Setup) -> Self {
        Self { amount: 0.3, size: 1.5, colour: false, seed: 0, moving: true }
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.amount = value.number() as f32 / 100.0,
            1 => self.size = value.number(),
            2 => self.colour = value.toggle(),
            3 => self.moving = value.toggle(),
            4 => self.seed = value.integer() as u64,
            _ => {}
        }
    }

    fn render(&mut self, inputs: &[Frame], output: &mut FrameMut, ctx: &RenderCtx) {
        let input = inputs[0];
        // One seed per frame (or one for all frames), and one per channel for colour grain.
        let seed = if self.moving { ctx.seed(self.seed) } else { random::hash(self.seed, 0x6772_6169_6e) };
        let seeds = [seed, random::hash(seed, 1), random::hash(seed, 2)];
        // Grain is measured in project pixels: the preview, drawn smaller, shows the same grain.
        let (cell, to_project) = (self.size as f32, 1.0 / ctx.scale);
        let (amount, colour) = (self.amount, self.colour);
        ctx.rows(output, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let c = input.straight(x, y);
                let (gx, gy) = ((x as f32 + 0.5) * to_project, (y as f32 + 0.5) * to_project);
                // Smoothed value noise has about half the spread of plain noise: 1.8 evens it out.
                let n = |s: u64| (random::smooth(s, gx, gy, cell) * 2.0 - 1.0) * 1.8;
                let mono = n(seeds[0]);
                let mut out = c;
                for i in 0..3 {
                    let g = if colour { n(seeds[i]) } else { mono };
                    // Strongest in the mid-tones, as on film: 1 at mid grey, a quarter at the ends.
                    let tone = 0.25 + 3.0 * c[i] * (1.0 - c[i]);
                    out[i] = (c[i] + g * amount * 0.25 * tone).clamp(0.0, 1.0);
                }
                *px = from_straight(out);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_plugin::testing::{Bench, Image};

    #[test]
    fn grain_moves_but_repeats() {
        let grey = Image::solid(64, 64, [128, 128, 128, 255]);
        let mut b = Bench::<FilmGrain>::new();
        let first = b.effect(&grey);
        assert!(first.difference(&grey) > 3.0, "grain shows");
        let mean = first.mean()[0];
        assert!((mean - 128.0).abs() < 4.0, "and doesn't change the brightness ({mean})");
        // The same frame again: the same grain (the preview and the export match).
        assert_eq!(b.effect(&grey), first);
        // The next frame: other grain.
        b.at(1.0 / 30.0);
        assert!(b.effect(&grey).difference(&first) > 3.0);
        // Not moving: every frame the same.
        b.set("Moving", false);
        let still = b.effect(&grey);
        b.at(2.0);
        assert_eq!(b.effect(&grey), still);
        // Monochrome grain is grey; colour grain isn't.
        assert!(still.data.chunks(4).all(|p| p[0] == p[1] && p[1] == p[2]));
        b.set("Colour", true);
        assert!(b.effect(&grey).data.chunks(4).any(|p| p[0] != p[1]));
    }
}
