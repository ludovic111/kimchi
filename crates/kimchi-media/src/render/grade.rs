//! A clip's effects on its picture ([`kimchi_core::Effects`]): chroma key, colour corrections,
//! LUT, sharpen and vignette, in that order, on premultiplied RGBA. Rows are processed in
//! parallel.
//!
//! The corrections work on gamma-encoded values (what people see on a slider):
//! brightness shifts by up to ±0.4, contrast scales around mid-grey by 0…2×, saturation
//! scales the distance from the luma by 0…2×, temperature and tint scale red/blue and green by
//! up to ±20 %.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::Effects;
use rayon::prelude::*;
use tiny_skia::Pixmap;

use crate::{MediaError, MediaResult};

/// Applies `e` to `p` in place. `scale` is output pixels per project pixel (sharpen radius).
/// A LUT that can't be read is skipped (and reported once).
pub(crate) fn apply(p: &mut Pixmap, e: &Effects, scale: f32) {
    if !e.is_active() {
        return;
    }
    let lut = e.lut.as_ref().and_then(|l| match cube(Path::new(&l.path)) {
        Ok(c) => Some((c, l.strength as f32)),
        Err(err) => {
            warn_once(&l.path, &err.to_string());
            None
        }
    });
    if e.sharpen > 1e-4 {
        sharpen(p, e.sharpen as f32, (1.5 * scale).clamp(0.75, 4.0));
    }
    let key = e.chroma_key.as_ref().map(|k| Key::new(&k.color, k.similarity as f32, k.softness as f32, k.spill as f32));
    let tone = Tone::of(e);
    let (w, h) = (p.width() as usize, p.height() as usize);
    let vignette = e.vignette as f32;
    p.data_mut().par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let ny = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let a = px[3];
            if a == 0 {
                continue;
            }
            let inv = if a == 255 { 1.0 / 255.0 } else { 1.0 / a as f32 };
            let mut rgb = [px[0] as f32 * inv, px[1] as f32 * inv, px[2] as f32 * inv];
            let mut a8 = a as f32;
            if let Some(k) = &key {
                a8 = ((a8 * k.keep(&mut rgb)).clamp(0.0, 255.0) + 0.5) as u8 as f32;
            }
            tone.apply(&mut rgb);
            if let Some((lut, strength)) = &lut {
                let g = lut.lookup(rgb);
                for c in 0..3 {
                    rgb[c] += (g[c] - rgb[c]) * strength;
                }
            }
            if vignette > 0.0 {
                let nx = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                let d = ((nx * nx + ny * ny) / 2.0).sqrt();
                let k = 1.0 - vignette * smoothstep(0.35, 1.0, d) * 0.9;
                for v in &mut rgb {
                    *v *= k;
                }
            }
            for c in 0..3 {
                px[c] = (rgb[c].clamp(0.0, 1.0) * a8 + 0.5) as u8;
            }
            px[3] = a8 as u8;
        }
    });
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The corrections, as one affine map of the colour: gains, lift and contrast per channel, then
/// saturation around the luma (a matrix), folded together.
struct Tone {
    m: [[f32; 3]; 3],
    t: [f32; 3],
    identity: bool,
}

impl Tone {
    fn of(e: &Effects) -> Self {
        let (t, n) = (e.temperature as f32, e.tint as f32);
        let gains = [1.0 + 0.2 * t, 1.0 - 0.2 * n, 1.0 - 0.2 * t];
        let identity = e.brightness == 0.0 && e.contrast == 0.0 && e.saturation == 0.0 && t == 0.0 && n == 0.0;
        let (lift, contrast, sat) = (e.brightness as f32 * 0.4, 1.0 + e.contrast as f32, 1.0 + e.saturation as f32);
        // Per channel: v · gain · contrast + ((lift − ½) · contrast + ½).
        let scale = gains.map(|g| g * contrast);
        let offset = (lift - 0.5) * contrast + 0.5;
        // Saturation: s · v + (1 − s) · luma(v).
        let luma = [0.2126, 0.7152, 0.0722];
        let mix: [[f32; 3]; 3] = std::array::from_fn(|r| std::array::from_fn(|c| (if r == c { sat } else { 0.0 }) + (1.0 - sat) * luma[c]));
        let m = std::array::from_fn(|r| std::array::from_fn(|c| mix[r][c] * scale[c]));
        let t = std::array::from_fn(|r| offset * mix[r].iter().sum::<f32>());
        Self { m, t, identity }
    }

    #[inline]
    fn apply(&self, rgb: &mut [f32; 3]) {
        if self.identity {
            return;
        }
        let v = *rgb;
        for (o, (row, t)) in rgb.iter_mut().zip(self.m.iter().zip(self.t)) {
            *o = row[0] * v[0] + row[1] * v[1] + row[2] * v[2] + t;
        }
    }
}

/// Chroma key in the Cb/Cr plane. A pixel's "keyness" is how far its chroma goes in the key
/// colour's direction (1 = as far as the key colour), less twice how far it strays from that
/// direction; pixels at or above `1 − similarity` are screen, with a soft band below.
struct Key {
    cb: f32,
    cr: f32,
    /// |key chroma|².
    norm: f32,
    lo: f32,
    hi: f32,
    spill: f32,
    /// The key colour's strongest channel (1 = green).
    main: usize,
}

fn chroma(c: [f32; 3]) -> (f32, f32) {
    let y = luma(c);
    ((c[2] - y) / 1.8556, (c[0] - y) / 1.5748)
}

impl Key {
    fn new(color: &str, similarity: f32, softness: f32, spill: f32) -> Self {
        let c = super::paint::color(color);
        let rgb = [c.red(), c.green(), c.blue()];
        let (cb, cr) = chroma(rgb);
        let main = (0..3).max_by(|a, b| rgb[*a].total_cmp(&rgb[*b])).unwrap_or(1);
        let hi = 1.0 - similarity;
        Self { cb, cr, norm: (cb * cb + cr * cr).max(1e-4), lo: hi - softness - 1e-4, hi, spill, main }
    }

    /// How much of the pixel stays (0…1); takes the key colour's spill out of `rgb`.
    fn keep(&self, rgb: &mut [f32; 3]) -> f32 {
        let (cb, cr) = chroma(*rgb);
        let along = (cb * self.cb + cr * self.cr) / self.norm;
        let off = ((cb - along * self.cb).powi(2) + (cr - along * self.cr).powi(2)).sqrt() / self.norm.sqrt();
        let keep = 1.0 - smoothstep(self.lo, self.hi, along - 2.0 * off);
        if self.spill > 0.0 {
            let cap = (0..3).filter(|c| *c != self.main).map(|c| rgb[c]).fold(0.0, f32::max);
            let m = &mut rgb[self.main];
            if *m > cap {
                *m -= (*m - cap) * self.spill;
            }
        }
        keep
    }
}

/// Unsharp mask: `p + amount × (p − blurred p)`.
fn sharpen(p: &mut Pixmap, amount: f32, radius: f32) {
    let mut soft = p.clone();
    super::paint::blur(&mut soft, radius);
    let k = amount * 1.5;
    p.data_mut().par_chunks_mut(4).zip(soft.data().par_chunks(4)).for_each(|(px, s)| {
        let a = px[3] as f32;
        for c in 0..3 {
            let v = px[c] as f32 + (px[c] as f32 - s[c] as f32) * k;
            px[c] = (v.clamp(0.0, a) + 0.5) as u8;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// LUTs

/// A parsed `.cube` 3D LUT: `size`³ entries, red fastest.
#[derive(Debug)]
pub struct Cube {
    size: usize,
    data: Vec<[f32; 3]>,
    min: [f32; 3],
    max: [f32; 3],
}

impl Cube {
    pub fn parse(text: &str) -> Result<Cube, String> {
        let (mut size, mut min, mut max) = (0usize, [0.0f32; 3], [1.0f32; 3]);
        let mut data = vec![];
        let triple = |rest: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            match word {
                "LUT_3D_SIZE" => size = rest.trim().parse().map_err(|_| format!("bad LUT_3D_SIZE on line {}", n + 1))?,
                "LUT_1D_SIZE" => return Err("1D LUTs aren't supported; use a 3D .cube".into()),
                "DOMAIN_MIN" => min = triple(rest).ok_or("bad DOMAIN_MIN")?,
                "DOMAIN_MAX" => max = triple(rest).ok_or("bad DOMAIN_MAX")?,
                "TITLE" | "LUT_3D_INPUT_RANGE" => {}
                _ if word.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') => {
                    data.push(triple(line).ok_or_else(|| format!("line {} isn't three numbers", n + 1))?);
                }
                _ => {}
            }
        }
        if !(2..=256).contains(&size) {
            return Err("not a 3D .cube LUT (no LUT_3D_SIZE)".into());
        }
        if data.len() != size * size * size {
            return Err(format!("the LUT should have {} entries, it has {}", size * size * size, data.len()));
        }
        Ok(Cube { size, data, min, max })
    }

    /// Trilinear lookup of a colour (0…1 per channel).
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let mut i0 = [0usize; 3];
        let mut f = [0f32; 3];
        for c in 0..3 {
            let v = ((rgb[c] - self.min[c]) / (self.max[c] - self.min[c]).max(1e-6)).clamp(0.0, 1.0) * (n - 1) as f32;
            i0[c] = (v.floor() as usize).min(n - 2);
            f[c] = v - i0[c] as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + g * n + b * n * n];
        let mut out = [0f32; 3];
        for (corner, w) in (0..8).map(|k| {
            let (dr, dg, db) = (k & 1, (k >> 1) & 1, (k >> 2) & 1);
            let w = (if dr == 1 { f[0] } else { 1.0 - f[0] }) * (if dg == 1 { f[1] } else { 1.0 - f[1] }) * (if db == 1 { f[2] } else { 1.0 - f[2] });
            (at(i0[0] + dr, i0[1] + dg, i0[2] + db), w)
        }) {
            for c in 0..3 {
                out[c] += corner[c] * w;
            }
        }
        out
    }
}

type Cubes = Mutex<HashMap<PathBuf, (std::time::SystemTime, Arc<Cube>)>>;

/// The LUT at `path`, parsed once (again when the file changes).
pub fn cube(path: &Path) -> MediaResult<Arc<Cube>> {
    static CUBES: OnceLock<Cubes> = OnceLock::new();
    let cubes = CUBES.get_or_init(Default::default);
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).map_err(MediaError::Io)?;
    if let Some((m, c)) = super::lock(cubes).get(path)
        && *m == modified
    {
        return Ok(c.clone());
    }
    let text = std::fs::read_to_string(path)?;
    let c = Arc::new(Cube::parse(&text).map_err(|e| MediaError::Unsupported(format!("{}: {e}", path.display())))?);
    super::lock(cubes).insert(path.to_path_buf(), (modified, c.clone()));
    Ok(c)
}

fn warn_once(path: &str, err: &str) {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    if super::lock(SEEN.get_or_init(Default::default)).insert(path.to_string()) {
        tracing::warn!("LUT skipped: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_core::ChromaKey;
    use tiny_skia::Color;

    fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    fn filled(c: Color) -> Pixmap {
        let mut p = Pixmap::new(8, 8).unwrap();
        p.fill(c);
        p
    }

    #[test]
    fn saturation_minus_one_is_grey() {
        let mut p = filled(Color::from_rgba8(200, 40, 40, 255));
        apply(&mut p, &Effects { saturation: -1.0, ..Default::default() }, 1.0);
        let [r, g, b, a] = px(&p, 4, 4);
        assert_eq!((r, a), (g, 255));
        assert_eq!(g, b);
    }

    #[test]
    fn warmer_is_redder_and_brighter_is_lighter() {
        let grey = Color::from_rgba8(128, 128, 128, 255);
        let mut warm = filled(grey);
        apply(&mut warm, &Effects { temperature: 1.0, ..Default::default() }, 1.0);
        let [r, _, b, _] = px(&warm, 4, 4);
        assert!(r > 140 && b < 116, "{r} {b}");
        let mut bright = filled(grey);
        apply(&mut bright, &Effects { brightness: 0.5, ..Default::default() }, 1.0);
        assert!(px(&bright, 4, 4)[0] > 170);
    }

    #[test]
    fn green_screen_keys_out_and_keeps_the_rest() {
        let mut p = filled(Color::from_rgba8(30, 220, 40, 255));
        p.fill_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, 4.0, 8.0).unwrap(), &tiny_skia::Paint { shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(220, 180, 150, 255)), ..Default::default() }, tiny_skia::Transform::identity(), None);
        apply(&mut p, &Effects { chroma_key: Some(ChromaKey::default()), ..Default::default() }, 1.0);
        assert_eq!(px(&p, 6, 4)[3], 0, "the screen is gone");
        assert_eq!(px(&p, 1, 4)[3], 255, "the skin stays");
    }

    #[test]
    fn vignette_darkens_corners_only() {
        let mut p = Pixmap::new(64, 64).unwrap();
        p.fill(Color::WHITE);
        apply(&mut p, &Effects { vignette: 1.0, ..Default::default() }, 1.0);
        assert!(px(&p, 0, 0)[0] < 80);
        assert!(px(&p, 32, 32)[0] > 250);
    }

    #[test]
    fn cube_lut_maps_colours() {
        // An inverting 2³ LUT.
        let mut text = String::from("TITLE \"invert\"\nLUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    text.push_str(&format!("{} {} {}\n", 1 - r, 1 - g, 1 - b));
                }
            }
        }
        let cube = Cube::parse(&text).unwrap();
        let out = cube.lookup([0.25, 0.5, 1.0]);
        assert!((out[0] - 0.75).abs() < 1e-5 && (out[1] - 0.5).abs() < 1e-5 && out[2].abs() < 1e-5);
        assert!(Cube::parse("LUT_3D_SIZE 3\n0 0 0\n").unwrap_err().contains("27"));
    }
}
