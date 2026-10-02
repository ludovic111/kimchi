use super::*;

fn style(content: &str) -> TextStyle {
    TextStyle { content: content.into(), font_family: "Manrope".into(), shadow: false, ..TextStyle::default() }
}

/// Bounding box `(x0, y0, x1, y1)` of pixels whose alpha passes `min`.
fn ink(p: &Pixmap, min: u8) -> Option<(u32, u32, u32, u32)> {
    let alpha: Vec<u8> = p.pixels().iter().map(|c| c.alpha()).map(|a| if a >= min { a } else { 0 }).collect();
    ink_bounds(&alpha, p.width() as usize, p.height() as usize).map(|(a, b, c, d)| (a as u32, b as u32, c as u32, d as u32))
}

fn rgba(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = p.pixel(x, y).unwrap().demultiply();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

#[test]
fn alignment_and_lines() {
    // Block 300 wide, a 100-wide line.
    assert_eq!(line_start(TextAlign::Left, 300.0, 100.0), -150.0);
    assert_eq!(line_start(TextAlign::Center, 300.0, 100.0), -50.0);
    assert_eq!(line_start(TextAlign::Right, 300.0, 100.0), 50.0);
    // Three lines of 20: middles at -20, 0, 20 around the block centre.
    assert_eq!([0, 1, 2].map(|i| line_middle(i, 60.0, 20.0)), [-20.0, 0.0, 20.0]);
    assert_eq!(split_lines("a\nb\r\n"), ["a", "b", ""]);
    assert_eq!(split_lines(""), [" "]);
}

#[test]
fn background_box_pads_the_block() {
    // padX 0.35·size, padY 0.18·size, radius 0.18·size, centred on the block.
    let (x, y, w, h, r) = background_box(200.0, 115.0, 100.0);
    assert_eq!((x, y, w, h), (-135.0, -75.5, 270.0, 151.0));
    assert!((r - 18.0).abs() < 1e-9);
    // The radius never exceeds half the box.
    let p = round_rect(0.0, 0.0, 10.0, 4.0, 50.0).unwrap();
    assert_eq!(p.bounds(), Rect::from_xywh(0.0, 0.0, 10.0, 4.0).unwrap());
}

#[test]
fn measures_lines_spacing_and_weight() {
    let one = measure(&style("Hello"));
    assert!(one.width > 100.0 && one.width < 400.0, "{one:?}");
    assert!((one.height - 96.0 * 1.15).abs() < 1e-9 && one.line_widths.len() == 1);

    let two = measure(&style("Hello\nHello world"));
    assert_eq!(two.line_widths.len(), 2);
    assert!((two.line_widths[0] - one.width).abs() < 1e-6);
    assert_eq!(two.width, two.line_widths[1]);
    assert!((two.height - 2.0 * one.height).abs() < 1e-9);

    // Letter spacing comes after every character, the last one included.
    let spaced = measure(&TextStyle { letter_spacing: 10.0, ..style("Hello") });
    assert!((spaced.width - one.width - 50.0).abs() < 1e-3, "{} vs {}", spaced.width, one.width);

    // Bolder Manrope is wider; an empty text is one blank line.
    assert!(measure(&TextStyle { font_weight: 800, ..style("Hello") }).width > measure(&TextStyle { font_weight: 400, ..style("Hello") }).width);
    assert_eq!(measure(&style("")).line_widths.len(), 1);
}

#[test]
fn families_resolve_to_installed_or_bundled() {
    let families = font_families();
    assert_eq!(&families[..4], BUNDLED_FAMILIES.map(String::from));
    assert_eq!(resolve_family("Manrope"), "Manrope");
    assert_eq!(resolve_family("instrument serif"), "Instrument Serif");
    assert_eq!(resolve_family("Nope Mono"), "IBM Plex Mono");
    assert_eq!(resolve_family("Nope Serif Display"), "Instrument Serif");
    assert_eq!(resolve_family("Nope Grotesk"), "Manrope");
    assert_eq!(resolve_family("\"Nope\", monospace"), "IBM Plex Mono");
    assert_eq!(bundled_fonts().len(), BUNDLED_FONTS.len());
}

#[test]
fn draws_box_text_alignment_and_rotation() {
    let (w, h) = (640, 360);
    let boxed = TextStyle { background: Some("#ff0000".into()), font_size: 60.0, ..style("Hi") };
    let p = rasterize_text(&boxed, &Transform::default(), w, h);
    // Corners stay transparent, the box covers the centre around the text.
    assert_eq!(rgba(&p, 0, 0)[3], 0);
    let (x0, y0, x1, y1) = ink(&p, 128).unwrap();
    let m = measure(&boxed);
    let (bw, bh) = (m.width + 0.7 * 60.0, m.height + 0.36 * 60.0);
    assert!(((x1 - x0 + 1) as f64 - bw).abs() <= 2.0, "{x0}..{x1} vs {bw}");
    assert!(((y1 - y0 + 1) as f64 - bh).abs() <= 2.0, "{y0}..{y1} vs {bh}");
    assert!(((x0 + x1) as f64 / 2.0 - 320.0).abs() <= 1.5 && ((y0 + y1) as f64 / 2.0 - 180.0).abs() <= 1.5);
    assert_eq!(rgba(&p, x0 + 3, (y0 + y1) / 2), [255, 0, 0, 255]);

    // Two lines left-aligned: both start at the block's left edge; right-aligned: both end at its right.
    let two = |align| rasterize_text(&TextStyle { align, ..style("I\nIIIIII") }, &Transform::default(), w, h);
    let rows = |p: &Pixmap, y0: u32, y1: u32| {
        let alpha: Vec<u8> = (y0..y1).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| p.pixel(x, y).unwrap().alpha()).collect();
        let (a, _, b, _) = ink_bounds(&alpha, w as usize, (y1 - y0) as usize).unwrap();
        (a as i64, b as i64)
    };
    let m = measure(&style("I\nIIIIII"));
    let (left, right) = (two(TextAlign::Left), two(TextAlign::Right));
    let (l1, l2) = (rows(&left, 0, 180), rows(&left, 180, 360));
    let (r1, r2) = (rows(&right, 0, 180), rows(&right, 180, 360));
    assert!((l1.0 - l2.0).abs() <= 2 && (r1.1 - r2.1).abs() <= 2, "{l1:?} {l2:?} {r1:?} {r2:?}");
    assert!((l2.0 as f64 - (320.0 - m.width / 2.0)).abs() <= 12.0, "{l2:?} {}", m.width);
    assert!(l1.1 < r1.1 && r1.0 > l1.0);

    // Rotated a quarter turn and moved: the ink is taller than wide, centred at the offset.
    let long = style("Long line");
    let t = Transform { x: 100.0, y: -50.0, rotation: 90.0, scale: 0.5, ..Transform::default() };
    let p = rasterize_text(&long, &t, w, h);
    let (x0, y0, x1, y1) = ink(&p, 1).unwrap();
    assert!(y1 - y0 > x1 - x0, "{:?}", (x0, y0, x1, y1));
    assert!(((y1 - y0) as f64 - measure(&long).width * 0.5).abs() < 12.0);
    assert!(((x0 + x1) as f64 / 2.0 - 420.0).abs() < 12.0 && ((y0 + y1) as f64 / 2.0 - 130.0).abs() < 6.0);
}

#[test]
fn shadow_falls_below_and_scaled_render_matches() {
    let s = TextStyle { shadow: true, color: "#ffffff".into(), font_size: 80.0, ..style("H") };
    let plain = rasterize_text(&TextStyle { shadow: false, ..s.clone() }, &Transform::default(), 400, 300);
    let shadowed = rasterize_text(&s, &Transform::default(), 400, 300);
    let (_, _, _, bottom) = ink(&plain, 1).unwrap();
    let (_, _, _, shadow_bottom) = ink(&shadowed, 1).unwrap();
    assert!(shadow_bottom > bottom + 3, "{shadow_bottom} vs {bottom}");
    // The shadow is dark and translucent; the glyph itself stays white.
    let below = rgba(&shadowed, 200, bottom + 3);
    assert!(below[3] > 0 && below[3] < 140 && below[0] == 0, "{below:?}");

    let half = rasterize_text_scaled(&s, &Transform::default(), 400, 300, 0.5);
    assert_eq!((half.width(), half.height()), (200, 150));
    let (a, b) = (ink(&plain, 128).unwrap(), ink(&half, 128).unwrap());
    assert!((a.0 as i64 / 2 - b.0 as i64).abs() <= 2 && (a.2 as i64 / 2 - b.2 as i64).abs() <= 2, "{a:?} {b:?}");
}

#[test]
fn colours_and_layout_cache() {
    assert_eq!(try_color("#fff"), Some(Color::WHITE));
    assert_eq!(try_color("#ff000080").map(|c| c.to_color_u8().alpha()), Some(128));
    assert_eq!(try_color("rgba(0, 0, 0, 0.5)").map(|c| c.to_color_u8().alpha()), Some(128));
    assert_eq!(try_color("rgb(10 20 30)").map(|c| c.to_color_u8().green()), Some(20));
    assert_eq!(try_color("nope"), None);

    // Layouts are shaped once and shared.
    let s = style("x");
    assert!(Arc::ptr_eq(&layout_cached(&s, 1.0), &layout_cached(&s.clone(), 1.0)));
    assert!(!Arc::ptr_eq(&layout_cached(&s, 1.0), &layout_cached(&style("y"), 1.0)));

    assert_eq!(box_radii(0.2), [0, 0, 0]);
    assert!(box_radii(8.0).iter().all(|r| (6..=8).contains(r)), "{:?}", box_radii(8.0));
}

#[test]
#[ignore = "writes sample PNGs to $KIMCHI_TEXT_SAMPLES for eyeballing"]
fn samples() {
    let Some(dir) = std::env::var_os("KIMCHI_TEXT_SAMPLES") else { return };
    let dir = std::path::Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    let cases = [
        ("default", TextStyle::default(), Transform::default()),
        (
            "boxed",
            TextStyle { background: Some("#f7806a".into()), color: "#1b1b1b".into(), content: "Manrope 800\nsecond line".into(), font_family: "Manrope".into(), font_weight: 800, ..TextStyle::default() },
            Transform { rotation: -8.0, y: -120.0, ..Transform::default() },
        ),
        (
            "mono",
            TextStyle { content: "IBM Plex Mono\nitalic, spaced".into(), font_family: "IBM Plex Mono".into(), italic: true, letter_spacing: 6.0, align: TextAlign::Left, font_weight: 400, ..TextStyle::default() },
            Transform { scale: 0.7, ..Transform::default() },
        ),
        (
            "system",
            TextStyle { content: "Georgia & Helvetica Neue 🍜 日本語".into(), font_family: "Helvetica Neue".into(), font_weight: 700, align: TextAlign::Right, ..TextStyle::default() },
            Transform { scale: 0.6, y: 200.0, ..Transform::default() },
        ),
        (
            "serif-bold",
            TextStyle { content: "Instrument Serif 600".into(), font_family: "Instrument Serif".into(), ..TextStyle::default() },
            Transform::default(),
        ),
    ];
    for (name, s, t) in cases {
        let started = std::time::Instant::now();
        let p = rasterize_text(&s, &t, 1920, 1080);
        eprintln!("{name}: {:?}", started.elapsed());
        p.save_png(dir.join(format!("{name}.png"))).unwrap();
    }
}


/// The webview editor drew text with WebKit's Canvas 2D (macOS). Reference numbers from the old
/// `drawText` run in a WKWebView with the bundled fonts on a 1920×1080 canvas: block width and the
/// bounding boxes of pixels with alpha ≥ 128 and ≥ 1 (shadow included).
#[test]
fn matches_the_webkit_canvas() {
    let s = |content: &str, family: &str, size, weight, color: &str, bg: Option<&str>, align, line_height, letter_spacing, shadow| TextStyle {
        content: content.into(),
        font_family: family.into(),
        font_size: size,
        font_weight: weight,
        italic: false,
        color: color.into(),
        background: bg.map(Into::into),
        align,
        line_height,
        letter_spacing,
        shadow,
    };
    let t = |x, y, scale, rotation| Transform { x, y, scale, rotation, ..Transform::default() };
    let cases = [
        ("default", TextStyle::default(), t(0.0, 0.0, 1.0, 0.0), 205.86, (859, 503, 1060, 574), (841, 488, 1079, 597)),
        (
            "boxed",
            s("Manrope 800\nsecond line", "Manrope", 96.0, 800, "#1b1b1b", Some("#f7806a"), TextAlign::Center, 1.15, 0.0, true),
            t(0.0, -120.0, 1.0, -8.0),
            625.06,
            (602, 248, 1317, 591),
            (601, 247, 1318, 592),
        ),
        (
            "left",
            s("Left aligned\nthree\nlines here", "Manrope", 72.0, 400, "#ffffff", None, TextAlign::Left, 1.3, 6.0, false),
            t(100.0, 50.0, 0.7, 0.0),
            463.10,
            (897, 506, 1214, 675),
            (897, 506, 1214, 675),
        ),
        (
            "mono-right",
            s("IBM Plex\nMono right", "IBM Plex Mono", 80.0, 400, "#ffffff", Some("#000000"), TextAlign::Right, 1.15, 0.0, false),
            t(-200.0, 100.0, 1.2, 30.0),
            480.0,
            (424, 375, 1095, 904),
            (424, 374, 1095, 905),
        ),
    ];
    let near = |a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)| {
        [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)].iter().all(|(x, y)| x.abs_diff(*y) <= 2)
    };
    for (name, style, transform, width, solid, any) in cases {
        assert!((measure(&style).width - width).abs() < 0.1, "{name}: {} vs {width}", measure(&style).width);
        let p = rasterize_text(&style, &transform, 1920, 1080);
        let (a, b) = (ink(&p, 128).unwrap(), ink(&p, 1).unwrap());
        assert!(near(a, solid) && near(b, any), "{name}: {a:?} {b:?} vs {solid:?} {any:?}");
    }
}
