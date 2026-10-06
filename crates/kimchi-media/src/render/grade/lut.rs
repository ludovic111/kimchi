//! Colour lookup tables in the formats other apps write, read into one model: per-channel 1D
//! curves (a shaper or a whole 1D LUT), then a 3D table looked up with tetrahedral
//! interpolation.
//!
//! Formats, and the specifications followed:
//! - `.cube`: Adobe Cube LUT Specification 1.0 (2013): `LUT_1D_SIZE`, `LUT_3D_SIZE`,
//!   `DOMAIN_MIN`/`DOMAIN_MAX`, `TITLE`; and DaVinci Resolve's variant, a 1D shaper followed by
//!   a 3D table with `LUT_1D_INPUT_RANGE`/`LUT_3D_INPUT_RANGE` (Resolve 17 manual, "LUTs").
//! - `.3dl`: Autodesk Lustre / Flame 3D mesh: the input shaper line of integers, then integer
//!   outputs with blue changing fastest; the output bit depth is the one OpenColorIO's
//!   `FileFormat3DL` picks from the largest value (or a `Mesh <in> <out>` header's).
//! - `.csp`: Rising Sun Research cineSpace `CSPLUTV100`, 1D or 3D, per-channel pre-LUT
//!   (OpenColorIO's `FileFormatCSP` layout), 3D red fastest.
//! - `.spi1d` / `.spi3d`: Sony Pictures Imageworks (OpenColorIO's own): `From`, `Length`,
//!   `Components`; 3D lines with explicit indices.
//! - Hald CLUT images (PNG, TIFF): level `L` is an `L³`×`L³` picture of an `L²`-point cube, red
//!   fastest (Eskil Steenberg's Hald CLUT, as ffmpeg's `haldclut`, darktable and G'MIC use it).

use std::path::Path;

/// A LUT read from a file: optional per-channel curves, then an optional 3D table.
#[derive(Debug, Clone)]
pub struct ColorLut {
    /// The file format (`cube`, `3dl`, `csp`, `spi1d`, `spi3d`, `hald`).
    pub format: &'static str,
    /// `TITLE` of a `.cube`.
    pub title: Option<String>,
    pre: Option<[Curve; 3]>,
    cube: Option<Table>,
}

/// One channel's 1D curve: `ys` sampled evenly over `min..max`, or at the inputs `xs`.
#[derive(Debug, Clone)]
struct Curve {
    min: f32,
    max: f32,
    xs: Option<Vec<f32>>,
    ys: Vec<f32>,
}

impl Curve {
    fn uniform(min: f32, max: f32, ys: Vec<f32>) -> Self {
        Self { min, max, xs: None, ys }
    }

    #[inline]
    fn eval(&self, v: f32) -> f32 {
        let n = self.ys.len();
        if n == 0 {
            return v;
        }
        if n == 1 {
            return self.ys[0];
        }
        let pos = match &self.xs {
            None => ((v - self.min) / (self.max - self.min).max(1e-9)).clamp(0.0, 1.0) * (n - 1) as f32,
            Some(xs) => {
                if v <= xs[0] {
                    0.0
                } else if v >= xs[n - 1] {
                    (n - 1) as f32
                } else {
                    // The segment holding v (xs rise).
                    let i = xs.partition_point(|x| *x <= v).clamp(1, n - 1) - 1;
                    let span = (xs[i + 1] - xs[i]).max(1e-9);
                    i as f32 + ((v - xs[i]) / span).clamp(0.0, 1.0)
                }
            }
        };
        let i = (pos.floor() as usize).min(n - 2);
        let f = pos - i as f32;
        self.ys[i] + (self.ys[i + 1] - self.ys[i]) * f
    }
}

/// A 3D table: `size[0]·size[1]·size[2]` entries, red fastest, over `min..max` per channel.
#[derive(Debug, Clone)]
struct Table {
    size: [usize; 3],
    data: Vec<[f32; 3]>,
    min: [f32; 3],
    max: [f32; 3],
}

impl Table {
    fn new(size: [usize; 3], data: Vec<[f32; 3]>) -> Result<Self, String> {
        if size.iter().any(|n| *n < 2) {
            return Err("a 3D LUT needs at least 2 points on each side".into());
        }
        if size.iter().any(|n| *n > 256) {
            return Err(format!("a {}-point 3D LUT is too big (256 at most)", size.iter().max().unwrap_or(&0)));
        }
        let want = size[0] * size[1] * size[2];
        if data.len() != want {
            return Err(format!("the 3D table should have {want} entries, it has {}", data.len()));
        }
        Ok(Self { size, data, min: [0.0; 3], max: [1.0; 3] })
    }

    fn cell(&self, rgb: [f32; 3]) -> ([usize; 3], [f32; 3]) {
        let mut i0 = [0usize; 3];
        let mut f = [0f32; 3];
        for c in 0..3 {
            let n = self.size[c];
            let v = ((rgb[c] - self.min[c]) / (self.max[c] - self.min[c]).max(1e-9)).clamp(0.0, 1.0) * (n - 1) as f32;
            i0[c] = (v.floor() as usize).min(n - 2);
            f[c] = v - i0[c] as f32;
        }
        (i0, f)
    }

    #[inline]
    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[r + g * self.size[0] + b * self.size[0] * self.size[1]]
    }

    fn trilinear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let (i, f) = self.cell(rgb);
        let mut out = [0f32; 3];
        for k in 0..8 {
            let (dr, dg, db) = (k & 1, (k >> 1) & 1, (k >> 2) & 1);
            let w = (if dr == 1 { f[0] } else { 1.0 - f[0] }) * (if dg == 1 { f[1] } else { 1.0 - f[1] }) * (if db == 1 { f[2] } else { 1.0 - f[2] });
            let c = self.at(i[0] + dr, i[1] + dg, i[2] + db);
            for ch in 0..3 {
                out[ch] += c[ch] * w;
            }
        }
        out
    }

    /// Tetrahedral interpolation: the cell is split into six tetrahedra along its grey diagonal
    /// and the colour mixes the four corners of the one it is in. Neutrals stay neutral and it
    /// follows the table's curvature better than trilinear.
    fn tetrahedral(&self, rgb: [f32; 3]) -> [f32; 3] {
        let ([r, g, b], [fr, fg, fb]) = self.cell(rgb);
        let c000 = self.at(r, g, b);
        let c111 = self.at(r + 1, g + 1, b + 1);
        // (weights of c000, corner 1, corner 2, c111) and the two middle corners.
        let (w, c1, c2) = if fr > fg {
            if fg > fb {
                ([1.0 - fr, fr - fg, fg - fb, fb], self.at(r + 1, g, b), self.at(r + 1, g + 1, b))
            } else if fr > fb {
                ([1.0 - fr, fr - fb, fb - fg, fg], self.at(r + 1, g, b), self.at(r + 1, g, b + 1))
            } else {
                ([1.0 - fb, fb - fr, fr - fg, fg], self.at(r, g, b + 1), self.at(r + 1, g, b + 1))
            }
        } else if fb > fg {
            ([1.0 - fb, fb - fg, fg - fr, fr], self.at(r, g, b + 1), self.at(r, g + 1, b + 1))
        } else if fb > fr {
            ([1.0 - fg, fg - fb, fb - fr, fr], self.at(r, g + 1, b), self.at(r, g + 1, b + 1))
        } else {
            ([1.0 - fg, fg - fr, fr - fb, fb], self.at(r, g + 1, b), self.at(r + 1, g + 1, b))
        };
        std::array::from_fn(|ch| w[0] * c000[ch] + w[1] * c1[ch] + w[2] * c2[ch] + w[3] * c111[ch])
    }
}

/// How a 3D table is looked up between its points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Trilinear,
    Tetrahedral,
}

impl ColorLut {
    /// The graded colour (tetrahedral between the 3D table's points).
    #[inline]
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.lookup_with(rgb, Interpolation::Tetrahedral)
    }

    pub fn lookup_with(&self, rgb: [f32; 3], how: Interpolation) -> [f32; 3] {
        let mut v = rgb;
        if let Some(pre) = &self.pre {
            v = [pre[0].eval(v[0]), pre[1].eval(v[1]), pre[2].eval(v[2])];
        }
        match &self.cube {
            Some(t) => match how {
                Interpolation::Tetrahedral => t.tetrahedral(v),
                Interpolation::Trilinear => t.trilinear(v),
            },
            None => v,
        }
    }

    /// "3D, 33 points", "1D, 4096 points", "1D shaper and 3D, 33 points".
    pub fn describe(&self) -> String {
        let n1 = self.pre.as_ref().map(|p| p[0].ys.len());
        match (&self.cube, n1) {
            (Some(t), None) => format!("3D, {} points", t.size[0]),
            (Some(t), Some(_)) => format!("shaper and 3D, {} points", t.size[0]),
            (None, Some(n)) => format!("1D, {n} points"),
            (None, None) => "no change".into(),
        }
    }

    /// Reads a LUT file by its extension (a `.cube`, `.3dl`, `.csp`, `.spi1d`, `.spi3d`, or a
    /// Hald CLUT `.png` / `.tif`).
    pub fn read(path: &Path) -> Result<ColorLut, String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" => hald_png(path),
            "tif" | "tiff" => hald_tiff(path),
            _ => {
                let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
                // Some writers use Latin-1 in titles and comments.
                let text = String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|b| *b as char).collect());
                Self::parse(&text, &ext)
            }
        }
    }

    /// Parses a text LUT in the format `ext` names.
    pub fn parse(text: &str, ext: &str) -> Result<ColorLut, String> {
        match ext.to_ascii_lowercase().as_str() {
            "cube" => parse_cube(text),
            "3dl" => parse_3dl(text),
            "csp" => parse_csp(text),
            "spi1d" => parse_spi1d(text),
            "spi3d" => parse_spi3d(text),
            other => Err(format!(
                "`.{other}` isn't a LUT kimchi reads: use a .cube, .3dl, .csp, .spi1d or .spi3d file, or a Hald CLUT picture (.png, .tif)"
            )),
        }
    }

    /// A Hald CLUT from its pixels (0…1, row by row), `w`×`h`.
    pub fn from_hald(pixels: Vec<[f32; 3]>, w: u32, h: u32) -> Result<ColorLut, String> {
        if w != h {
            return Err(format!("a Hald CLUT picture is square; this one is {w}×{h}"));
        }
        let level = (w as f64).cbrt().round() as u32;
        if level < 2 || level * level * level != w {
            return Err(format!("{w}×{h} isn't a Hald CLUT size (level 8 is 512×512, level 12 is 1728×1728)"));
        }
        let n = (level * level) as usize;
        Ok(ColorLut { format: "hald", title: None, pre: None, cube: Some(Table::new([n; 3], pixels)?) })
    }

    /// A 3D-only LUT from a `size`³ table, red fastest (for tests and bakes).
    pub fn from_table(size: usize, data: Vec<[f32; 3]>) -> Result<ColorLut, String> {
        Ok(ColorLut { format: "cube", title: None, pre: None, cube: Some(Table::new([size; 3], data)?) })
    }
}

/// Numbers on a line, or `None` when one isn't a number.
fn numbers(line: &str) -> Option<Vec<f32>> {
    line.split_whitespace().map(|x| x.parse::<f32>().ok()).collect()
}

fn triple(rest: &str) -> Option<[f32; 3]> {
    let v = numbers(rest)?;
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

fn pair(rest: &str) -> Option<(f32, f32)> {
    let v = numbers(rest)?;
    (v.len() == 2).then(|| (v[0], v[1]))
}

fn is_data(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.')
}

fn parse_cube(text: &str) -> Result<ColorLut, String> {
    let (mut n1, mut n3) = (0usize, 0usize);
    let (mut dmin, mut dmax) = (None, None);
    let (mut range1, mut range3) = (None, None);
    let mut title = None;
    let mut data: Vec<[f32; 3]> = vec![];
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let bad = |what: &str| format!("bad {what} on line {}", n + 1);
        match word {
            "TITLE" => title = Some(rest.trim_matches('"').to_string()),
            "LUT_3D_SIZE" => n3 = rest.parse().map_err(|_| bad("LUT_3D_SIZE"))?,
            "LUT_1D_SIZE" => n1 = rest.parse().map_err(|_| bad("LUT_1D_SIZE"))?,
            "DOMAIN_MIN" => dmin = Some(triple(rest).ok_or_else(|| bad("DOMAIN_MIN"))?),
            "DOMAIN_MAX" => dmax = Some(triple(rest).ok_or_else(|| bad("DOMAIN_MAX"))?),
            "LUT_1D_INPUT_RANGE" => range1 = Some(pair(rest).ok_or_else(|| bad("LUT_1D_INPUT_RANGE"))?),
            "LUT_3D_INPUT_RANGE" => range3 = Some(pair(rest).ok_or_else(|| bad("LUT_3D_INPUT_RANGE"))?),
            _ if is_data(word) => data.push(triple(line).ok_or_else(|| format!("line {} isn't three numbers", n + 1))?),
            // LUT_IN_VIDEO_RANGE and other writers' keywords.
            _ => {}
        }
    }
    if n1 == 0 && n3 == 0 {
        return Err("not a .cube LUT: it has neither LUT_3D_SIZE nor LUT_1D_SIZE".into());
    }
    if n1 > 65_536 || n1 == 1 {
        return Err(format!("LUT_1D_SIZE {n1} is out of range (2 to 65536)"));
    }
    let want = n1 + n3 * n3 * n3;
    if data.len() != want {
        return Err(format!("the LUT should have {want} entries, it has {}", data.len()));
    }
    let (min, max) = (dmin.unwrap_or([0.0; 3]), dmax.unwrap_or([1.0; 3]));
    if (0..3).any(|c| max[c] <= min[c]) {
        return Err("DOMAIN_MAX should be above DOMAIN_MIN".into());
    }
    let pre = (n1 > 0).then(|| {
        let (lo, hi): ([f32; 3], [f32; 3]) = match range1 {
            Some((a, b)) => ([a; 3], [b; 3]),
            None => (min, max),
        };
        std::array::from_fn(|c| Curve::uniform(lo[c], hi[c], data[..n1].iter().map(|v| v[c]).collect()))
    });
    let cube = if n3 > 0 {
        let mut t = Table::new([n3; 3], data[n1..].to_vec())?;
        match (range3, n1 > 0) {
            (Some((a, b)), _) => (t.min, t.max) = ([a; 3], [b; 3]),
            // With a shaper, DOMAIN_* is the shaper's.
            (None, true) => {}
            (None, false) => (t.min, t.max) = (min, max),
        }
        Some(t)
    } else {
        None
    };
    Ok(ColorLut { format: "cube", title, pre, cube })
}

/// OpenColorIO's guess at a 3DL's integer bit depth from its largest value: the first of 8,
/// 10, 12, 14, 16 bits whose doubled range holds it.
fn likely_bit_depth(max: f32) -> u32 {
    [8u32, 10, 12, 14, 16].into_iter().find(|b| max <= ((1u64 << b) * 2 - 1) as f32).unwrap_or(16)
}

fn parse_3dl(text: &str) -> Result<ColorLut, String> {
    let mut shaper: Option<Vec<f32>> = None;
    let mut rows: Vec<[f32; 3]> = vec![];
    let mut out_bits: Option<u32> = None;
    let mut floats = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or("");
        if !is_data(first) {
            // Lustre's `Mesh <input bits> <output bits>`; 3DMESH, LUT8, gamma: nothing to do.
            if first.eq_ignore_ascii_case("mesh")
                && let Some(v) = numbers(line.split_once(char::is_whitespace).map_or("", |x| x.1))
                && v.len() == 2
            {
                out_bits = Some(v[1] as u32);
            }
            continue;
        }
        let v = numbers(line).ok_or_else(|| format!("line {} isn't numbers", n + 1))?;
        floats |= line.contains('.') || line.contains('e');
        if v.len() == 3 && (shaper.is_some() || !rows.is_empty()) {
            rows.push([v[0], v[1], v[2]]);
        } else if shaper.is_none() && rows.is_empty() && v.len() != 3 {
            shaper = Some(v);
        } else if v.len() == 3 {
            rows.push([v[0], v[1], v[2]]);
        } else {
            return Err(format!("line {} should be three numbers", n + 1));
        }
    }
    let size = match &shaper {
        Some(s) => s.len(),
        None => (rows.len() as f64).cbrt().round() as usize,
    };
    if size < 2 || rows.len() != size * size * size {
        return Err(format!("a {size}-point .3dl should have {} entries, it has {}", size * size * size, rows.len()));
    }
    let max = rows.iter().flat_map(|r| r.iter().copied()).fold(0.0f32, f32::max);
    let scale = if floats && max <= 4.0 {
        1.0
    } else {
        let bits = out_bits.filter(|b| (8..=16).contains(b)).unwrap_or_else(|| likely_bit_depth(max));
        ((1u32 << bits) - 1) as f32
    };
    // Blue fastest in the file: reorder to red fastest.
    let mut data = vec![[0.0f32; 3]; rows.len()];
    for (k, row) in rows.iter().enumerate() {
        let (r, g, b) = (k / (size * size), (k / size) % size, k % size);
        data[r + g * size + b * size * size] = row.map(|x| x / scale);
    }
    // The shaper: where each mesh point sits on the input scale.
    let pre = match shaper {
        Some(s) => {
            let last = s.iter().copied().fold(0.0f32, f32::max);
            let in_max = if floats && last <= 4.0 { 1.0 } else { ((1u32 << likely_bit_depth(last)) - 1) as f32 };
            let xs: Vec<f32> = s.iter().map(|x| x / in_max).collect();
            let even = xs.iter().enumerate().all(|(i, x)| (x - i as f32 / (size - 1) as f32).abs() < 0.5 / in_max.max(1.0) + 1e-3);
            if xs.windows(2).any(|w| w[1] <= w[0]) {
                return Err("the .3dl's input line should rise".into());
            }
            (!even).then(|| {
                let ys: Vec<f32> = (0..size).map(|i| i as f32 / (size - 1) as f32).collect();
                std::array::from_fn(|_| Curve { min: 0.0, max: 1.0, xs: Some(xs.clone()), ys: ys.clone() })
            })
        }
        None => None,
    };
    Ok(ColorLut { format: "3dl", title: None, pre, cube: Some(Table::new([size; 3], data)?) })
}

fn parse_csp(text: &str) -> Result<ColorLut, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if !lines.next().is_some_and(|l| l.eq_ignore_ascii_case("CSPLUTV100")) {
        return Err("not a cineSpace LUT (it should start with CSPLUTV100)".into());
    }
    let kind = lines.next().unwrap_or("").to_ascii_uppercase();
    if kind != "1D" && kind != "3D" {
        return Err(format!("the second line of a .csp is 1D or 3D, not \"{kind}\""));
    }
    let mut rest: Vec<&str> = vec![];
    let mut meta = false;
    for l in lines {
        if l.eq_ignore_ascii_case("BEGIN METADATA") {
            meta = true;
        } else if l.eq_ignore_ascii_case("END METADATA") {
            meta = false;
        } else if !meta {
            rest.push(l);
        }
    }
    let mut it = rest.into_iter();
    let mut curves: Vec<Curve> = vec![];
    for ch in ["red", "green", "blue"] {
        let mut next = |what: &str| it.next().ok_or_else(|| format!("the .csp ends before its {ch} pre-LUT {what}"));
        let count: usize = next("size")?.parse().map_err(|_| format!("bad {ch} pre-LUT size"))?;
        let xs = numbers(next("inputs")?).ok_or_else(|| format!("bad {ch} pre-LUT inputs"))?;
        let ys = numbers(next("outputs")?).ok_or_else(|| format!("bad {ch} pre-LUT outputs"))?;
        if xs.len() != count || ys.len() != count {
            return Err(format!("the {ch} pre-LUT should have {count} inputs and outputs"));
        }
        if xs.windows(2).any(|w| w[1] < w[0]) {
            return Err(format!("the {ch} pre-LUT's inputs should rise"));
        }
        curves.push(if count < 2 { Curve::uniform(0.0, 1.0, vec![0.0, 1.0]) } else { Curve { min: xs[0], max: xs[count - 1], xs: Some(xs), ys } });
    }
    let identity = curves.iter().all(|c| match &c.xs {
        Some(xs) => xs.iter().zip(&c.ys).all(|(x, y)| (x - y).abs() < 1e-6),
        None => true,
    });
    let [r, g, b]: [Curve; 3] = curves.try_into().map_err(|_| "bad pre-LUT")?;
    let pre = (!identity).then_some([r, g, b]);
    let mut body = it;
    let head = body.next().ok_or("the .csp ends before its table")?;
    let read = |count: usize, body: &mut dyn Iterator<Item = &str>| -> Result<Vec<[f32; 3]>, String> {
        let v: Vec<[f32; 3]> = body.take(count).map(|l| triple(l).ok_or_else(|| format!("\"{l}\" isn't three numbers"))).collect::<Result<_, _>>()?;
        if v.len() != count {
            return Err(format!("the table should have {count} entries, it has {}", v.len()));
        }
        Ok(v)
    };
    if kind == "1D" {
        let n: usize = head.parse().map_err(|_| "bad 1D size")?;
        let v = read(n, &mut body)?;
        let one: [Curve; 3] = std::array::from_fn(|c| Curve::uniform(0.0, 1.0, v.iter().map(|x| x[c]).collect()));
        // pre-LUT then the 1D table: fold into one sampled curve per channel.
        let curves = match pre {
            None => one,
            Some(p) => std::array::from_fn(|c| {
                let (lo, hi) = (p[c].min, p[c].max);
                let steps = 4096;
                let ys = (0..steps).map(|i| one[c].eval(p[c].eval(lo + (hi - lo) * i as f32 / (steps - 1) as f32))).collect();
                Curve::uniform(lo, hi, ys)
            }),
        };
        return Ok(ColorLut { format: "csp", title: None, pre: Some(curves), cube: None });
    }
    let sizes: Vec<usize> = head.split_whitespace().map(|x| x.parse().map_err(|_| "bad 3D size line")).collect::<Result<_, _>>()?;
    let [nr, ng, nb]: [usize; 3] = sizes.try_into().map_err(|_| "the 3D size line should be three numbers")?;
    let v = read(nr * ng * nb, &mut body)?;
    Ok(ColorLut { format: "csp", title: None, pre, cube: Some(Table::new([nr, ng, nb], v)?) })
}

fn parse_spi1d(text: &str) -> Result<ColorLut, String> {
    let (mut from, mut length, mut comps) = ((0.0f32, 1.0f32), 0usize, 1usize);
    let mut values: Vec<Vec<f32>> = vec![];
    let mut inside = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if inside {
            if line.starts_with('}') {
                inside = false;
                continue;
            }
            values.push(numbers(line).ok_or_else(|| format!("line {} isn't numbers", n + 1))?);
            continue;
        }
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match word.to_ascii_lowercase().as_str() {
            "version" => {}
            "from" => from = pair(rest).ok_or("bad From line")?,
            "length" => length = rest.trim().parse().map_err(|_| "bad Length line")?,
            "components" => comps = rest.trim().parse().map_err(|_| "bad Components line")?,
            "{" => inside = true,
            _ => {}
        }
    }
    if !(1..=3).contains(&comps) {
        return Err(format!("Components is 1, 2 or 3, not {comps}"));
    }
    if values.len() != length || length < 2 {
        return Err(format!("the .spi1d should have {length} values, it has {}", values.len()));
    }
    if values.iter().any(|v| v.len() != comps) {
        return Err(format!("every .spi1d value should have {comps} components"));
    }
    let pick = |c: usize| -> Vec<f32> { values.iter().map(|v| v[c.min(comps - 1)]).collect() };
    let pre = std::array::from_fn(|c| Curve::uniform(from.0, from.1, if comps == 2 && c == 2 { pick(0) } else { pick(c) }));
    Ok(ColorLut { format: "spi1d", title: None, pre: Some(pre), cube: None })
}

fn parse_spi3d(text: &str) -> Result<ColorLut, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    if !lines.next().is_some_and(|l| l.to_ascii_uppercase().starts_with("SPILUT")) {
        return Err("not a .spi3d LUT (it should start with SPILUT 1.0)".into());
    }
    let _channels = lines.next().ok_or("the .spi3d ends early")?;
    let sizes = numbers(lines.next().ok_or("the .spi3d has no size line")?).ok_or("bad size line")?;
    let [nr, ng, nb]: [usize; 3] = sizes.iter().map(|x| *x as usize).collect::<Vec<_>>().try_into().map_err(|_| "the size line should be three numbers")?;
    let mut data = vec![[f32::NAN; 3]; nr * ng * nb];
    for line in lines {
        let v = numbers(line).ok_or_else(|| format!("\"{line}\" isn't numbers"))?;
        if v.len() != 6 {
            return Err(format!("\"{line}\" should be three indices and three values"));
        }
        let (r, g, b) = (v[0] as usize, v[1] as usize, v[2] as usize);
        if r >= nr || g >= ng || b >= nb {
            return Err(format!("\"{line}\" is outside the {nr}×{ng}×{nb} table"));
        }
        data[r + g * nr + b * nr * ng] = [v[3], v[4], v[5]];
    }
    if data.iter().any(|v| v[0].is_nan()) {
        return Err("the .spi3d is missing entries".into());
    }
    Ok(ColorLut { format: "spi3d", title: None, pre: None, cube: Some(Table::new([nr, ng, nb], data)?) })
}

fn hald_png(path: &Path) -> Result<ColorLut, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().map_err(|e| format!("not a PNG picture ({e})"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("the PNG can't be read ({e})"))?;
    let chans = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => return Err("a Hald CLUT is a colour picture; this one is grey".into()),
        png::ColorType::Indexed => 3,
    };
    let wide = info.bit_depth == png::BitDepth::Sixteen;
    let step = chans * if wide { 2 } else { 1 };
    let px: Vec<[f32; 3]> = buf[..info.buffer_size()]
        .chunks_exact(step)
        .map(|p| {
            if wide {
                std::array::from_fn(|c| u16::from_be_bytes([p[c * 2], p[c * 2 + 1]]) as f32 / 65535.0)
            } else {
                std::array::from_fn(|c| p[c] as f32 / 255.0)
            }
        })
        .collect();
    ColorLut::from_hald(px, info.width, info.height)
}

fn hald_tiff(path: &Path) -> Result<ColorLut, String> {
    use tiff::decoder::{Decoder, DecodingResult};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = Decoder::new(std::io::BufReader::new(file)).map_err(|e| format!("not a TIFF picture ({e})"))?;
    let (w, h) = dec.dimensions().map_err(|e| e.to_string())?;
    let chans = match dec.colortype().map_err(|e| e.to_string())? {
        tiff::ColorType::RGB(_) => 3,
        tiff::ColorType::RGBA(_) => 4,
        other => return Err(format!("a Hald CLUT is an RGB picture; this TIFF is {other:?}")),
    };
    let px: Vec<[f32; 3]> = match dec.read_image().map_err(|e| format!("the TIFF can't be read ({e})"))? {
        DecodingResult::U8(v) => v.chunks_exact(chans).map(|p| std::array::from_fn(|c| p[c] as f32 / 255.0)).collect(),
        DecodingResult::U16(v) => v.chunks_exact(chans).map(|p| std::array::from_fn(|c| p[c] as f32 / 65535.0)).collect(),
        DecodingResult::F32(v) => v.chunks_exact(chans).map(|p| std::array::from_fn(|c| p[c])).collect(),
        _ => return Err("kimchi reads 8 and 16-bit or float TIFF Hald CLUTs".into()),
    };
    ColorLut::from_hald(px, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|c| (a[c] - b[c]).abs() < 1e-4)
    }

    /// An `n`³ table of `f`, red fastest.
    fn grid(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Vec<[f32; 3]> {
        let k = (n - 1) as f32;
        (0..n * n * n).map(|i| f([(i % n) as f32 / k, ((i / n) % n) as f32 / k, (i / (n * n)) as f32 / k])).collect()
    }

    #[test]
    fn tetrahedral_and_trilinear_on_known_values() {
        // Only the white corner lit: trilinear mixes r·g·b of it, tetrahedral the smallest channel.
        let t = ColorLut::from_table(2, grid(2, |c| if c == [1.0; 3] { [1.0; 3] } else { [0.0; 3] })).unwrap();
        let p = [0.5, 0.5, 0.5];
        assert!(close(t.lookup_with(p, Interpolation::Trilinear), [0.125; 3]));
        assert!(close(t.lookup_with(p, Interpolation::Tetrahedral), [0.5; 3]));
        let q = [0.8, 0.4, 0.2];
        assert!(close(t.lookup_with(q, Interpolation::Trilinear), [0.8 * 0.4 * 0.2; 3]));
        assert!(close(t.lookup_with(q, Interpolation::Tetrahedral), [0.2; 3]));
        // Both are exact on a linear table, at points and between them.
        let id = ColorLut::from_table(5, grid(5, |c| [c[0] * 0.5 + 0.1, c[1], 1.0 - c[2]])).unwrap();
        for v in [[0.13, 0.77, 0.4], [0.0, 1.0, 0.5], [0.9, 0.1, 0.33]] {
            let want = [v[0] * 0.5 + 0.1, v[1], 1.0 - v[2]];
            assert!(close(id.lookup_with(v, Interpolation::Tetrahedral), want));
            assert!(close(id.lookup_with(v, Interpolation::Trilinear), want));
        }
    }

    #[test]
    fn cube_1d_3d_and_shaper() {
        let mut one = String::from("TITLE \"half\"\nLUT_1D_SIZE 3\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n");
        one.push_str("0 0 0\n0.25 0.5 0.5\n0.5 1 1\n");
        let l = ColorLut::parse(&one, "cube").unwrap();
        assert_eq!(l.title.as_deref(), Some("half"));
        assert!(close(l.lookup([1.0, 1.0, 2.0]), [0.25, 0.5, 1.0]));
        assert!(close(l.lookup([0.5, 0.0, 0.0]), [0.125, 0.0, 0.0]));
        // Resolve: a shaper squaring the input, then an inverting 2³ table over 0…1.
        let mut both = String::from("LUT_1D_SIZE 3\nLUT_1D_INPUT_RANGE 0 1\nLUT_3D_SIZE 2\nLUT_3D_INPUT_RANGE 0 1\n");
        both.push_str("0 0 0\n0.25 0.25 0.25\n1 1 1\n");
        for c in grid(2, |c| c.map(|x| 1.0 - x)) {
            both.push_str(&format!("{} {} {}\n", c[0], c[1], c[2]));
        }
        let l = ColorLut::parse(&both, "cube").unwrap();
        assert_eq!(l.describe(), "shaper and 3D, 2 points");
        assert!(close(l.lookup([0.5, 1.0, 0.0]), [0.75, 0.0, 1.0]));
        assert!(ColorLut::parse("LUT_3D_SIZE 3\n0 0 0\n", "cube").unwrap_err().contains("27"));
        assert!(ColorLut::parse("TITLE x\n", "cube").is_err());
    }

    #[test]
    fn three_dl_detects_depth_and_order() {
        // A 2-point mesh: 10-bit inputs, 12-bit outputs, blue fastest; the table swaps red and blue.
        let mut t = String::from("# Lustre\n0 1023\n");
        for r in 0..2 {
            for g in 0..2 {
                for b in 0..2 {
                    t.push_str(&format!("{} {} {}\n", b * 4095, g * 4095, r * 4095));
                }
            }
        }
        let l = ColorLut::parse(&t, "3dl").unwrap();
        assert!(close(l.lookup([1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]));
        assert!(close(l.lookup([0.0, 0.5, 0.25]), [0.25, 0.5, 0.0]));
        // 10-bit outputs (largest 1023) and a 16-bit header.
        let ten = t.replace("4095", "1023");
        assert!(close(ColorLut::parse(&ten, "3dl").unwrap().lookup([1.0, 1.0, 1.0]), [1.0; 3]));
        let sixteen = format!("Mesh 1 16\n{}", t.replace("4095", "65535"));
        assert!(close(ColorLut::parse(&sixteen, "3dl").unwrap().lookup([0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]));
        assert_eq!(likely_bit_depth(1023.0), 10);
        assert_eq!(likely_bit_depth(4095.0), 12);
        assert_eq!(likely_bit_depth(65535.0), 16);
    }

    #[test]
    fn three_dl_uneven_shaper() {
        // Mesh points at 0, 256 and 1023: half-way through the input range is past the middle point.
        let mut t = String::from("0 256 1023\n");
        for r in 0..3 {
            for _g in 0..3 {
                for _b in 0..3 {
                    let v = r as f32 / 2.0 * 1023.0;
                    t.push_str(&format!("{v:.0} 0 0\n"));
                }
            }
        }
        let l = ColorLut::parse(&t, "3dl").unwrap();
        // Input 256/1023 is mesh point 1 → output 0.5.
        assert!((l.lookup([256.0 / 1023.0, 0.0, 0.0])[0] - 0.5).abs() < 2e-3);
    }

    #[test]
    fn csp_with_prelut() {
        let mut t = String::from("CSPLUTV100\n3D\n\nBEGIN METADATA\nmade by hand\nEND METADATA\n\n");
        // Each pre-LUT maps 0…4 onto 0…1.
        for _ in 0..3 {
            t.push_str("2\n0 4\n0 1\n");
        }
        t.push_str("\n2 2 2\n");
        for c in grid(2, |c| c) {
            t.push_str(&format!("{} {} {}\n", c[0], c[1], c[2]));
        }
        let l = ColorLut::parse(&t, "csp").unwrap();
        assert!(close(l.lookup([2.0, 4.0, 1.0]), [0.5, 1.0, 0.25]));
        let one = "CSPLUTV100\n1D\n2\n0 1\n0 1\n2\n0 1\n0 1\n2\n0 1\n0 1\n3\n0 0 0\n0.2 0.2 0.2\n1 1 1\n";
        assert!(close(ColorLut::parse(one, "csp").unwrap().lookup([0.5; 3]), [0.2; 3]));
        assert!(ColorLut::parse("CSPLUT\n3D\n", "csp").is_err());
    }

    #[test]
    fn spi_files() {
        let one = "Version 1\nFrom 0.0 1.0\nLength 3\nComponents 1\n{\n 0.0\n 0.1\n 1.0\n}\n";
        assert!(close(ColorLut::parse(one, "spi1d").unwrap().lookup([0.5, 0.25, 1.0]), [0.1, 0.05, 1.0]));
        let mut three = String::from("SPILUT 1.0\n3 3\n2 2 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    three.push_str(&format!("{r} {g} {b} {} {} {}\n", 1 - r, 1 - g, 1 - b));
                }
            }
        }
        assert!(close(ColorLut::parse(&three, "spi3d").unwrap().lookup([0.25, 0.5, 1.0]), [0.75, 0.5, 0.0]));
    }

    #[test]
    fn hald_sizes() {
        // Level 2: an 8×8 picture of a 4-point cube.
        let px = grid(4, |c| c);
        let l = ColorLut::from_hald(px, 8, 8).unwrap();
        assert!(close(l.lookup([0.2, 0.6, 0.9]), [0.2, 0.6, 0.9]));
        assert!(ColorLut::from_hald(vec![[0.0; 3]; 100], 10, 10).unwrap_err().contains("Hald"));
    }
}
