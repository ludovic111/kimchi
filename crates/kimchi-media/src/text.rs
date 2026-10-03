//! Text layers, rasterised in Rust so the preview and the export draw them the same.
//!
//! A port of the Canvas 2D drawing the webview editor did (`measure`/`drawText` in the old
//! `ui/src/lib/util/text.ts`), so projects made there render the same: a transparent canvas the
//! size of the project, translated to its centre + `transform.x/y`, rotated by
//! `transform.rotation` degrees and scaled by `transform.scale`; an optional rounded box behind the
//! block; lines `line_height · font_size` apart on a "middle" baseline, aligned left/center/right
//! within the block; letter spacing in pixels after every character; and a soft drop shadow.
//! The "middle" baseline follows WebKit (the macOS webview): `(ascent − descent) / 2` of the
//! primary font. Chromium centres the em box instead, which puts text a few pixels higher.
//! `tests.rs` checks block widths and ink bounds against numbers from WKWebView (within 2 px).
//!
//! Shaping, font matching and fallback come from cosmic-text (system fonts through fontdb, plus
//! the families bundled here); glyph outlines are filled with tiny-skia, so rotated and scaled
//! text stays crisp. Titles are drawn at full opacity: clip opacity, fades and blur are applied by
//! the compositor ([`crate::render`]), which also draws motion text layers glyph by glyph (reveals).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Command, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent,
    Weight, Wrap, fontdb,
};
use kimchi_core::{TextAlign, TextStyle, Transform};
use tiny_skia::{
    Color, FillRule, IntSize, Paint, PathBuilder, Pixmap, PixmapPaint, PremultipliedColorU8, Rect, Stroke,
    Transform as Affine,
};


/// The families the editor offers first. Manrope and IBM Plex Mono are the lsuite faces;
/// Instrument Sans/Serif were the webview editor's defaults, so older projects use them.
pub const BUNDLED_FAMILIES: [&str; 4] = ["Manrope", "IBM Plex Mono", "Instrument Sans", "Instrument Serif"];

/// Every embedded font file (all OFL, licences in `crates/kimchi-media/fonts/*/OFL.txt`).
/// Manrope and IBM Plex Mono are static instances (400–800 / 400–700 with italics) so CoreText
/// and cosmic-text pick the same faces; Instrument Sans is variable (`wght` 400–700).
pub static BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../fonts/manrope/Manrope-Regular.ttf"),
    include_bytes!("../fonts/manrope/Manrope-Medium.ttf"),
    include_bytes!("../fonts/manrope/Manrope-SemiBold.ttf"),
    include_bytes!("../fonts/manrope/Manrope-Bold.ttf"),
    include_bytes!("../fonts/manrope/Manrope-ExtraBold.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-Regular.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-Italic.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-Medium.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-MediumItalic.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-SemiBold.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-SemiBoldItalic.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-Bold.ttf"),
    include_bytes!("../fonts/ibmplexmono/IBMPlexMono-BoldItalic.ttf"),
    include_bytes!("../fonts/instrumentsans/InstrumentSans.ttf"),
    include_bytes!("../fonts/instrumentsans/InstrumentSans-Italic.ttf"),
    include_bytes!("../fonts/instrumentserif/InstrumentSerif-Regular.ttf"),
    include_bytes!("../fonts/instrumentserif/InstrumentSerif-Italic.ttf"),
];

/// [`BUNDLED_FONTS`] as a list, e.g. for GPUI's `text_system().add_fonts`.
pub fn bundled_fonts() -> Vec<&'static [u8]> {
    BUNDLED_FONTS.to_vec()
}


/// Draws a text layer onto a transparent canvas `width`×`height` (the project size), exactly as
/// the export does. Pixels are premultiplied RGBA (tiny-skia's layout).
pub fn rasterize_text(style: &TextStyle, transform: &Transform, width: u32, height: u32) -> Pixmap {
    rasterize_text_scaled(style, transform, width, height, 1.0)
}

/// [`rasterize_text`] drawn at `scale` × the project size (e.g. 1/3 for a 640×360 preview of a
/// 1080p project), as if the full-size canvas had been scaled: shadow offset and blur scale too.
pub fn rasterize_text_scaled(style: &TextStyle, transform: &Transform, width: u32, height: u32, scale: f32) -> Pixmap {
    let t = transform;
    let pl = kimchi_core::Placement { x: t.x, y: t.y, scale_x: t.scale, scale_y: t.scale, rotation: t.rotation, opacity: 1.0, blur: 0.0, fit: t.fit };
    rasterize_title(style, &pl, width, height, scale)
}

/// A title at a placement (keyframes applied, so x and y scale may differ), `scale` × the
/// project size `width`×`height`. Opacity and blur are left to the compositor.
pub fn rasterize_title(style: &TextStyle, pl: &kimchi_core::Placement, width: u32, height: u32, scale: f32) -> Pixmap {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (pw, ph) = (((width.max(1) as f32 * scale).round() as u32).max(1), ((height.max(1) as f32 * scale).round() as u32).max(1));
    let mut canvas = Pixmap::new(pw, ph).expect("non-empty canvas");
    if !(pl.scale_x.is_finite() && pl.scale_y.is_finite() && pl.scale_x > 0.0 && pl.scale_y > 0.0) {
        return canvas;
    }
    let to_canvas = Affine::from_scale(scale, scale)
        .pre_translate(width as f32 / 2.0 + pl.x as f32, height as f32 / 2.0 + pl.y as f32)
        .pre_rotate(pl.rotation as f32)
        .pre_scale(pl.scale_x as f32, pl.scale_y as f32);
    let fs = style.font_size.max(0.0) as f32;
    let layout = layout_cached(style, scale * pl.scale_x.max(pl.scale_y) as f32);

    if let Some(bg) = style.background.as_deref().filter(|b| !b.trim().is_empty()) {
        let (x, y, w, h, r) = background_box(layout.width, layout.height, style.font_size);
        if let Some(path) = round_rect(x as f32, y as f32, w as f32, h as f32, r as f32) {
            canvas.fill_path(&path, &solid(parse_color(bg)), FillRule::Winding, to_canvas, None);
        }
    }

    // Glyphs go on their own layer first: the shadow is cast by exactly what the text covers.
    let mut ink = Pixmap::new(pw, ph).expect("non-empty canvas");
    let color = parse_color(&style.color);
    let paint = solid(color);
    for glyph in &layout.glyphs {
        match &glyph.ink {
            Ink::Outline { path, embolden } => {
                ink.fill_path(path, &paint, FillRule::Winding, to_canvas, None);
                if *embolden > 0.0 {
                    let stroke = Stroke { width: *embolden, ..Stroke::default() };
                    ink.stroke_path(path, &paint, &stroke, to_canvas, None);
                }
            }
            Ink::Image { pixmap, x, y, size } => {
                let at = to_canvas.pre_translate(*x, *y).pre_scale(1.0 / size, 1.0 / size);
                let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
                ink.draw_pixmap(0, 0, pixmap.as_ref(), &paint, at, None);
            }
        }
    }
    if style.shadow && fs > 0.0 {
        // Canvas shadows live in canvas pixels: neither rotated nor scaled by the transform.
        let shadow = drop_shadow(&ink, fs * 0.18 * scale / 2.0, 0.45);
        let at = Affine::from_translate(0.0, fs * 0.04 * scale);
        let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
        canvas.draw_pixmap(0, 0, shadow.as_ref(), &paint, at, None);
    }
    canvas.draw_pixmap(0, 0, ink.as_ref(), &PixmapPaint::default(), Affine::identity(), None);
    canvas
}

/// Size of a text block in project pixels, before `transform.scale` (the old `measure`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextMetrics {
    /// Widest line (at least 1 px), letter spacing included.
    pub width: f64,
    /// `line_height · font_size · lines`.
    pub height: f64,
    pub line_height: f64,
    pub line_widths: Vec<f64>,
}

/// Measures `style` the way the renderer lays it out (for selection boxes and hit testing).
pub fn measure(style: &TextStyle) -> TextMetrics {
    let l = layout_cached(style, 0.0);
    TextMetrics { width: l.width, height: l.height, line_height: l.line_height, line_widths: l.line_widths.clone() }
}

/// Family names for a font picker: the bundled ones first, then the system's, sorted.
pub fn font_families() -> Vec<String> {
    let fonts = fonts();
    let mut system: Vec<String> = fonts
        .system
        .db()
        .faces()
        .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
        .filter(|n| !n.starts_with('.') && !BUNDLED_FAMILIES.iter().any(|b| b.eq_ignore_ascii_case(n)))
        .collect();
    system.sort_by_key(|n| n.to_lowercase());
    system.dedup();
    BUNDLED_FAMILIES.iter().map(|s| s.to_string()).chain(system).collect()
}

/// The family that `requested` resolves to: itself when installed (or bundled), else a bundled
/// family of the same kind (monospace, serif, or Manrope for everything else). Accepts CSS-style
/// lists (`"Futura", sans-serif`) and the generic names.
pub fn resolve_family(requested: &str) -> String {
    fonts().resolve(requested)
}

/// Loads the fonts now (they otherwise load on first use, which takes a moment).
pub fn preload_fonts() {
    drop(fonts());
}

// ---------------------------------------------------------------------------------------------
// Layout

/// A shaped block, in block-local coordinates: origin at the block centre, y down, project
/// pixels before `transform.scale`.
pub(crate) struct Layout {
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) line_height: f64,
    pub(crate) line_widths: Vec<f64>,
    pub(crate) glyphs: Vec<Glyph>,
    /// How many letters (not counting spaces), words and lines there are, for reveals.
    pub(crate) units: Units,
}

/// Counts or indices of a glyph's letter (spaces skipped), word and line.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Units {
    pub(crate) chars: usize,
    pub(crate) words: usize,
    pub(crate) lines: usize,
}

pub(crate) struct Glyph {
    pub(crate) ink: Ink,
    /// Which letter, word and line it belongs to.
    pub(crate) unit: Units,
    /// Middle of the glyph's advance box (for per-letter scaling), block-local.
    pub(crate) center: (f32, f32),
    /// Width of its advance box (letter spacing not included).
    pub(crate) advance: f32,
    /// Its line's baseline, block-local y (text on a path sits there).
    pub(crate) baseline: f32,
}

pub(crate) enum Ink {
    /// A glyph outline; `embolden` > 0 strokes it too (synthetic bold).
    Outline { path: tiny_skia::Path, embolden: f32 },
    /// A bitmap glyph (colour emoji) rasterised at `size`× its layout size, top-left at (x, y).
    Image { pixmap: Pixmap, x: f32, y: f32, size: f32 },
}

/// Where a line starts (its left edge) for `align`, like Canvas `textAlign` with the anchor at
/// the block's left edge, centre or right edge.
fn line_start(align: TextAlign, block_width: f64, line_width: f64) -> f64 {
    match align {
        TextAlign::Left => -block_width / 2.0,
        TextAlign::Center => -line_width / 2.0,
        TextAlign::Right => block_width / 2.0 - line_width,
    }
}

/// Vertical centre of line `i` (the Canvas "middle" baseline before the font's offset).
fn line_middle(i: usize, block_height: f64, line_height: f64) -> f64 {
    -block_height / 2.0 + line_height * (i as f64 + 0.5)
}

/// The rounded box behind the text: `(x, y, w, h, radius)` in block-local pixels.
fn background_box(width: f64, height: f64, font_size: f64) -> (f64, f64, f64, f64, f64) {
    let (pad_x, pad_y) = (font_size * 0.35, font_size * 0.18);
    (-width / 2.0 - pad_x, -height / 2.0 - pad_y, width + pad_x * 2.0, height + pad_y * 2.0, font_size * 0.18)
}

/// Lines as the webview split them: on `\n` only, an empty text being one blank line.
fn split_lines(content: &str) -> Vec<&str> {
    let content = if content.is_empty() { " " } else { content };
    content.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

/// Shapes `style`. `device_scale` is how many canvas pixels a layout pixel covers, used to
/// rasterise bitmap glyphs sharply; 0 skips glyph geometry entirely (measuring only).
fn layout(style: &TextStyle, device_scale: f32) -> Layout {
    let fs = style.font_size.max(0.0);
    let line_height = fs * style.line_height;
    let lines = split_lines(&style.content);
    let mut fonts = fonts();
    let family = fonts.resolve(&style.font_family);
    let requested = Weight(style.font_weight.clamp(1, 1000));
    let italic = if style.italic { Style::Italic } else { Style::Normal };
    // cosmic-text only uses a family's face when its weight matches exactly (or the face can vary
    // to it), falling back to other families otherwise; browsers take the nearest weight and
    // embolden. So pick the face like CSS does and shape at its weight.
    let (primary, weight) = fonts.primary(&family, requested, italic);
    let attrs = Attrs::new()
        .family(Family::Name(&family))
        .weight(weight)
        .style(italic)
        .cache_key_flags(CacheKeyFlags::DISABLE_HINTING);
    let middle = primary.map_or(fs * 0.35, |id| fonts.middle_offset(id, weight, fs as f32) as f64);

    // Letter, word and line of every byte of every line, for reveals.
    let mut units = Units::default();
    let mut unit_of: Vec<Vec<Units>> = vec![];
    for (li, line) in lines.iter().enumerate() {
        let mut row = Vec::with_capacity(line.len() + 1);
        let mut in_word = false;
        for ch in line.chars() {
            let space = ch.is_whitespace();
            if !space && !in_word {
                units.words += 1;
            }
            in_word = !space;
            let here = Units { chars: units.chars, words: units.words.saturating_sub(1), lines: li };
            row.extend(std::iter::repeat_n(here, ch.len_utf8()));
            if !space {
                units.chars += 1;
            }
        }
        row.push(Units { chars: units.chars, words: units.words.saturating_sub(1), lines: li });
        unit_of.push(row);
    }
    units.lines = lines.len();

    // Shape every line first: alignment needs the widest one.
    let mut shaped = vec![];
    for line in &lines {
        let mut buffer = Buffer::new_empty(Metrics::new((fs as f32).max(0.01), (line_height as f32).max(0.01)));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        buffer.set_text(line, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut fonts.system, false);
        let mut glyphs = vec![];
        let mut width = 0.0f64;
        for run in buffer.layout_runs() {
            // Letter spacing goes after every character (cluster), the last one included.
            let mut clusters = 0usize;
            let mut last_start = None;
            for g in run.glyphs {
                if last_start != Some(g.start) {
                    if last_start.is_some() {
                        clusters += 1;
                    }
                    last_start = Some(g.start);
                }
                let x = g.x as f64 + (g.font_size * g.x_offset) as f64 + clusters as f64 * style.letter_spacing;
                let y = (g.y - g.font_size * g.y_offset) as f64;
                glyphs.push((x, y, g.w as f64, g.clone()));
            }
            let count = clusters + usize::from(last_start.is_some());
            width = width.max(run.line_w as f64 + count as f64 * style.letter_spacing);
        }
        shaped.push((width.max(0.0), glyphs));
    }
    let width = shaped.iter().map(|(w, _)| *w).fold(1.0, f64::max);
    let height = line_height * lines.len() as f64;

    let mut ink = vec![];
    if device_scale > 0.0 {
        for (i, (line_width, glyphs)) in shaped.iter().enumerate() {
            let x0 = line_start(style.align, width, *line_width);
            let mid = line_middle(i, height, line_height);
            let baseline = mid + middle;
            for (x, y, advance, g) in glyphs {
                let (gx, gy) = ((x0 + x) as f32, (baseline + y) as f32);
                if let Some(glyph) = fonts.glyph(g, gx, gy, requested, device_scale) {
                    let unit = unit_of[i].get(g.start).copied().unwrap_or_default();
                    ink.push(Glyph {
                        ink: glyph,
                        unit,
                        center: ((x0 + x + advance / 2.0) as f32, mid as f32),
                        advance: *advance as f32,
                        baseline: baseline as f32,
                    });
                }
            }
        }
    }
    Layout { width, height, line_height, line_widths: shaped.iter().map(|(w, _)| *w).collect(), glyphs: ink, units }
}

/// [`layout`], remembered: shaping is the slow part of drawing text, and animated text draws the
/// same layout every frame.
pub(crate) fn layout_cached(style: &TextStyle, device_scale: f32) -> Arc<Layout> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Layout>>>> = OnceLock::new();
    // Bitmap glyphs depend on the device scale; outlines don't, so round it for the key.
    let key = format!("{}|{:.2}", serde_json::to_string(style).unwrap_or_default(), device_scale);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(l) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return l.clone();
    }
    let l = Arc::new(layout(style, device_scale));
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 512 {
        map.clear();
    }
    map.insert(key, l.clone());
    l
}

// ---------------------------------------------------------------------------------------------
// Fonts

struct Fonts {
    system: FontSystem,
    swash: SwashCache,
    /// Requested family → family used.
    resolved: HashMap<String, String>,
    /// Lower-cased installed family names → their spelling.
    families: HashMap<String, String>,
    faces: HashMap<fontdb::ID, Face>,
}

fn fonts() -> MutexGuard<'static, Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            for font in BUNDLED_FONTS {
                db.load_font_source(fontdb::Source::Binary(Arc::new(*font)));
            }
            if std::env::var_os("KIMCHI_NO_SYSTEM_FONTS").is_none() {
                db.load_system_fonts();
            }
            db.set_sans_serif_family("Manrope");
            db.set_serif_family("Instrument Serif");
            db.set_monospace_family("IBM Plex Mono");
            let families = db
                .faces()
                .flat_map(|f| f.families.iter().map(|(n, _)| (n.to_lowercase(), n.clone())))
                .collect();
            Mutex::new(Fonts {
                system: FontSystem::new_with_locale_and_db("en-US".into(), db),
                swash: SwashCache::new(),
                resolved: HashMap::new(),
                families,
                faces: HashMap::new(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

impl Fonts {
    fn resolve(&mut self, requested: &str) -> String {
        if let Some(r) = self.resolved.get(requested) {
            return r.clone();
        }
        let names: Vec<String> =
            requested.split(',').map(|n| n.trim().trim_matches(['"', '\'']).trim().to_string()).filter(|n| !n.is_empty()).collect();
        let found = names.iter().find_map(|n| {
            let lower = n.to_lowercase();
            match lower.as_str() {
                "sans-serif" | "system-ui" | "ui-sans-serif" => Some("Manrope".to_string()),
                "serif" | "ui-serif" => Some("Instrument Serif".to_string()),
                "monospace" | "ui-monospace" => Some("IBM Plex Mono".to_string()),
                _ => self.families.get(&lower).cloned(),
            }
        });
        let family = found.unwrap_or_else(|| fallback_family(names.first().map_or("", String::as_str)).to_string());
        if names.first().is_some_and(|n| !n.eq_ignore_ascii_case(&family)) {
            tracing::debug!(requested, family, "font family not installed; using a bundled one");
        }
        self.resolved.insert(requested.to_string(), family.clone());
        family
    }

    /// The face CSS matching picks for `family` at `weight`/`style`, and the weight to shape at:
    /// the requested one when the face can vary to it, else the face's own.
    fn primary(&mut self, family: &str, weight: Weight, style: Style) -> (Option<fontdb::ID>, Weight) {
        let query = fontdb::Query { families: &[Family::Name(family)], weight, style, ..fontdb::Query::default() };
        let Some(id) = self.system.db().query(&query) else { return (None, weight) };
        let face = self.face(id);
        let shaped = match face.wght {
            Some((lo, hi)) if (lo..=hi).contains(&(weight.0 as f32)) => weight,
            _ => Weight(face.class),
        };
        (Some(id), shaped)
    }

    /// Canvas "middle" baseline: the baseline sits `(ascent − descent) / 2` below the line's
    /// middle, with the metrics of the primary font.
    fn middle_offset(&mut self, id: fontdb::ID, weight: Weight, size: f32) -> f32 {
        let Some(font) = self.system.get_font(id, weight) else { return size * 0.35 };
        let m = font.as_swash().metrics(&[]);
        let upem = (m.units_per_em as f32).max(1.0);
        (m.ascent - m.descent.abs()) / 2.0 * size / upem
    }

    /// What we need to know about a face to draw it.
    fn face(&mut self, id: fontdb::ID) -> Face {
        if let Some(f) = self.faces.get(&id) {
            return *f;
        }
        let class = self.system.db().face(id).map_or(400, |f| f.weight.0);
        let font = self.system.get_font(id, Weight(class));
        let wght = font.as_ref().and_then(|font| {
            let axis = font.as_swash().variations().find(|v| v.tag() == u32::from_be_bytes(*b"wght"))?;
            Some((axis.min_value(), axis.max_value()))
        });
        let color = font.is_some_and(|font| {
            [b"sbix", b"CBDT", b"COLR"].iter().any(|t| font.as_swash().table(u32::from_be_bytes(**t)).is_some())
        });
        let face = Face { class, wght, color };
        self.faces.insert(id, face);
        face
    }

    /// One positioned glyph: its outline, or a bitmap for colour glyphs.
    fn glyph(&mut self, g: &cosmic_text::LayoutGlyph, x: f32, y: f32, requested: Weight, device_scale: f32) -> Option<Ink> {
        let face = self.face(g.font_id);
        if face.color
            && let Some(image) = self.bitmap(g, x, y, device_scale)
        {
            return Some(image);
        }
        let (key, _, _) = CacheKey::new(g.font_id, g.glyph_id, g.font_size, (0.0, 0.0), g.font_weight, g.cache_key_flags);
        let commands = self.swash.get_outline_commands(&mut self.system, key)?;
        let path = outline_path(commands, x, y)?;
        // Like browsers, embolden a face lighter than a bold request that can't vary its weight.
        let can_vary = face.wght.is_some_and(|(_, hi)| hi >= requested.0 as f32);
        let embolden =
            if requested.0 >= 600 && face.class < 600 && !can_vary { fake_bold_width(g.font_size) } else { 0.0 };
        Some(Ink::Outline { path, embolden })
    }

    /// A colour glyph (emoji) rasterised at device size.
    fn bitmap(&mut self, g: &cosmic_text::LayoutGlyph, x: f32, y: f32, device_scale: f32) -> Option<Ink> {
        let size = device_scale.max(0.01);
        let (key, _, _) =
            CacheKey::new(g.font_id, g.glyph_id, g.font_size * size, (0.0, 0.0), g.font_weight, g.cache_key_flags);
        let image = self.swash.get_image_uncached(&mut self.system, key)?;
        let (w, h) = (image.placement.width, image.placement.height);
        let rgba: Vec<u8> = match image.content {
            SwashContent::Color => image.data.as_chunks::<4>().0.iter().flat_map(premultiply).collect(),
            SwashContent::Mask | SwashContent::SubpixelMask => return None,
        };
        let pixmap = Pixmap::from_vec(rgba, IntSize::from_wh(w, h)?)?;
        let (left, top) = (image.placement.left as f32 / size, image.placement.top as f32 / size);
        Some(Ink::Image { pixmap, x: x + left, y: y - top, size })
    }
}

#[derive(Debug, Clone, Copy)]
struct Face {
    /// OS/2 weight class.
    class: u16,
    /// Range of the `wght` axis, for variable fonts.
    wght: Option<(f32, f32)>,
    /// Has colour glyphs (sbix/CBDT/COLR).
    color: bool,
}

/// Skia's synthetic bold: an outline stroke 1/24 of the size at 9 px, down to 1/32 from 36 px.
fn fake_bold_width(size: f32) -> f32 {
    let t = ((size - 9.0) / 27.0).clamp(0.0, 1.0);
    size * (1.0 / 24.0 + (1.0 / 32.0 - 1.0 / 24.0) * t)
}

/// A family of the same kind as an unavailable one.
fn fallback_family(name: &str) -> &'static str {
    let n = name.to_lowercase();
    let mono = ["mono", "menlo", "courier", "consol", "code", "monaco", "terminal"];
    let serif = [
        "serif", "georgia", "times", "didot", "garamond", "baskerville", "bodoni", "palatino", "caslon", "cambria",
        "playfair", "merriweather", "lora", "charter", "hoefler",
    ];
    if mono.iter().any(|k| n.contains(k)) {
        "IBM Plex Mono"
    } else if serif.iter().any(|k| n.contains(k)) && !n.contains("sans") {
        "Instrument Serif"
    } else {
        "Manrope"
    }
}

/// swash outline (y up, origin on the baseline) → tiny-skia path at (x, y) in y-down space.
fn outline_path(commands: &[Command], x: f32, y: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for c in commands {
        match *c {
            Command::MoveTo(v) => pb.move_to(x + v.x, y - v.y),
            Command::LineTo(v) => pb.line_to(x + v.x, y - v.y),
            Command::QuadTo(c1, v) => pb.quad_to(x + c1.x, y - c1.y, x + v.x, y - v.y),
            Command::CurveTo(c1, c2, v) => pb.cubic_to(x + c1.x, y - c1.y, x + c2.x, y - c2.y, x + v.x, y - v.y),
            Command::Close => pb.close(),
        }
    }
    pb.finish()
}

// ---------------------------------------------------------------------------------------------
// Painting helpers

pub(crate) fn solid(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    paint
}

pub(crate) fn premultiply(px: &[u8; 4]) -> [u8; 4] {
    let a = px[3] as u16;
    let m = |c: u8| ((c as u16 * a + 127) / 255) as u8;
    [m(px[0]), m(px[1]), m(px[2]), px[3]]
}

/// The Canvas `roundRect` helper: four `arcTo` corners, radius clamped to fit.
pub(crate) fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let r = r.clamp(0.0, w.min(h) / 2.0);
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    // Cubic approximation of a quarter circle.
    let k = r * 0.552_284_8;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Black shadow of `layer`'s alpha: Gaussian blur of standard deviation `sigma` (Canvas uses
/// `shadowBlur / 2`), times `opacity`. Only the area around the ink is blurred.
pub(crate) fn drop_shadow(layer: &Pixmap, sigma: f32, opacity: f32) -> Pixmap {
    let (w, h) = (layer.width() as usize, layer.height() as usize);
    let mut out = Pixmap::new(layer.width(), layer.height()).expect("non-empty canvas");
    let alpha: Vec<u8> = layer.pixels().iter().map(|p| p.alpha()).collect();
    let Some((x0, y0, x1, y1)) = ink_bounds(&alpha, w, h) else { return out };
    let margin = (sigma * 3.0).ceil() as usize + 2;
    let (x0, y0, x1, y1) = (x0.saturating_sub(margin), y0.saturating_sub(margin), (x1 + margin).min(w - 1), (y1 + margin).min(h - 1));
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut buf: Vec<f32> = (0..bw * bh).map(|i| alpha[(y0 + i / bw) * w + x0 + i % bw] as f32).collect();
    if sigma > 0.1 {
        for r in box_radii(sigma) {
            box_blur(&mut buf, bw, bh, r);
        }
    }
    let px = out.pixels_mut();
    for (i, a) in buf.iter().enumerate() {
        let a = (a * opacity).round().clamp(0.0, 255.0) as u8;
        if a > 0 {
            px[(y0 + i / bw) * w + x0 + i % bw] = PremultipliedColorU8::from_rgba(0, 0, 0, a).expect("black is premultiplied");
        }
    }
    out
}

pub(crate) fn ink_bounds(alpha: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let row = &alpha[y * w..(y + 1) * w];
        if let (Some(a), Some(b)) = (row.iter().position(|&a| a > 0), row.iter().rposition(|&a| a > 0)) {
            (x0, x1, y0, y1) = (x0.min(a), x1.max(b), y0.min(y), y);
        }
    }
    (x0 <= x1 && y0 <= y1).then_some((x0, y0, x1, y1))
}

/// Radii of three box blurs approximating a Gaussian of standard deviation `sigma`.
pub(crate) fn box_radii(sigma: f32) -> [usize; 3] {
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - 3.0 * (wl * wl) as f32 - 12.0 * wl as f32 - 9.0) / (-4.0 * wl as f32 - 4.0)).round() as i32;
    std::array::from_fn(|i| (((if (i as i32) < m { wl } else { wu }) - 1) / 2).max(0) as usize)
}

/// In-place horizontal then vertical box blur of radius `r` (edges clamp to zero).
pub(crate) fn box_blur(buf: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut line = vec![0.0f32; w.max(h)];
    let mut pass = |buf: &mut [f32], len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        for j in 0..count {
            for (i, v) in line.iter_mut().enumerate().take(len) {
                *v = buf[at(j, i)];
            }
            let mut acc: f32 = line[..r.min(len)].iter().sum();
            for i in 0..len {
                if i + r < len {
                    acc += line[i + r];
                }
                if i > r {
                    acc -= line[i - r - 1];
                }
                buf[at(j, i)] = acc * norm;
            }
        }
    };
    pass(buf, w, h, &|row, i| row * w + i);
    pass(buf, h, w, &|col, i| i * w + col);
}

/// CSS colour → tiny-skia colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()`, a
/// few names. Anything else is black, like an ignored Canvas `fillStyle`.
pub(crate) fn parse_color(c: &str) -> Color {
    try_color(c).unwrap_or_else(|| {
        tracing::warn!(color = c, "unrecognised colour; using black");
        Color::BLACK
    })
}

pub(crate) fn try_color(c: &str) -> Option<Color> {
    let c = c.trim().to_ascii_lowercase();
    if let Some(hex) = c.strip_prefix('#') {
        let digits: Vec<u8> = hex.chars().map(|ch| ch.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
        let [r, g, b, a] = match digits.len() {
            3 | 4 => std::array::from_fn(|i| digits.get(i).map_or(255, |d| d * 17)),
            6 | 8 => std::array::from_fn(|i| digits.get(i * 2..i * 2 + 2).map_or(255, |d| d[0] * 16 + d[1])),
            _ => return None,
        };
        return Some(Color::from_rgba8(r, g, b, a));
    }
    if let Some(args) = c.strip_prefix("rgba(").or_else(|| c.strip_prefix("rgb(")).and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<f32> = args.split([',', ' ', '/']).filter(|p| !p.is_empty()).map(|p| p.parse().ok()).collect::<Option<_>>()?;
        let ch = |v: f32| v.clamp(0.0, 255.0).round() as u8;
        return match parts[..] {
            [r, g, b] => Some(Color::from_rgba8(ch(r), ch(g), ch(b), 255)),
            [r, g, b, a] => Some(Color::from_rgba8(ch(r), ch(g), ch(b), (a.clamp(0.0, 1.0) * 255.0).round() as u8)),
            _ => None,
        };
    }
    match c.as_str() {
        "white" => Some(Color::WHITE),
        "black" => Some(Color::BLACK),
        "transparent" => Some(Color::TRANSPARENT),
        "red" => Some(Color::from_rgba8(255, 0, 0, 255)),
        "yellow" => Some(Color::from_rgba8(255, 255, 0, 255)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
