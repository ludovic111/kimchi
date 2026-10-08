//! Pictures for models that can see.
//!
//! A few commands answer with the path of a picture (`harness.look`, `project.renderFrame`,
//! `media.frame`, `media.look`, `ui.screenshot`). The built-in agent and `kimchi-mcp` hand the model the picture
//! itself, not only its path, so an agent can look at what it made before saying it is done.
//! [`pictures_in`] says which results carry one; [`picture`] reads it (through ffmpeg when it isn't
//! a PNG, upright) and scales it to what models take in.

use std::path::{Path, PathBuf};

use base64::Engine;
use kimchi_media::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use serde_json::Value;

/// Commands whose answer's `path` is a picture to show the model.
pub const LOOKS: &[&str] = &["harness.look", "project.renderFrame", "media.frame", "media.look", "ui.screenshot"];

/// Longest side of a picture sent to a model, in pixels: what the providers keep without
/// scaling it down themselves.
pub const MAX_SIDE: u32 = 1568;

/// Most bytes of one encoded picture (Anthropic takes 5 MB of base64, which is 4/3 of this).
const MAX_BYTES: usize = 3_600_000;

/// A picture ready for a model: a PNG, base64-encoded.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub media_type: &'static str,
    /// Base64 (standard alphabet, no line breaks).
    pub data: String,
    pub width: u32,
    pub height: u32,
}

/// The pictures a command's answer points at (none for most commands).
pub fn pictures_in(command: &str, result: &Value) -> Vec<PathBuf> {
    if !LOOKS.contains(&command) {
        return vec![];
    }
    result.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from).into_iter().collect()
}

/// Reads the picture at `path` for a model: a PNG at most [`MAX_SIDE`] pixels on its longest side.
/// A PNG that already fits is sent as it is, byte for byte.
pub async fn picture(path: &Path) -> Result<Picture, String> {
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("Couldn't read the picture {}: {e}", path.display()))?;
    let decoded = match Pixmap::decode_png(&bytes) {
        Ok(p) => p,
        Err(_) => decode_with_ffmpeg(path).await?,
    };
    let (w, h) = (decoded.width(), decoded.height());
    if w.max(h) <= MAX_SIDE && bytes.len() <= MAX_BYTES && bytes.starts_with(b"\x89PNG") {
        return Ok(Picture { media_type: "image/png", data: base64::engine::general_purpose::STANDARD.encode(&bytes), width: w, height: h });
    }
    let mut side = MAX_SIDE;
    loop {
        let fitted = fit(&decoded, side);
        let png = fitted.encode_png().map_err(|e| format!("Couldn't encode the picture: {e}"))?;
        if png.len() <= MAX_BYTES || side <= 256 {
            return Ok(Picture {
                media_type: "image/png",
                data: base64::engine::general_purpose::STANDARD.encode(&png),
                width: fitted.width(),
                height: fitted.height(),
            });
        }
        side = side * 3 / 4;
    }
}

/// JPEG, WebP, HEIC… (and EXIF rotation) through ffmpeg, into a PNG decoded here.
async fn decode_with_ffmpeg(path: &Path) -> Result<Pixmap, String> {
    let tools = kimchi_media::Tools::locate().map_err(|e| format!("{} isn't a PNG, and ffmpeg, which would read it, wasn't found: {e}", path.display()))?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let tmp = std::env::temp_dir().join(format!("kimchi-look-{}-{nanos}.png", std::process::id()));
    let made = kimchi_media::grab_frame(&tools, path, 0.0, &tmp).await.map_err(|e| format!("Couldn't read the picture {}: {e}", path.display()));
    let decoded = made.and_then(|()| Pixmap::load_png(&tmp).map_err(|e| format!("Couldn't read the picture {}: {e}", path.display())));
    let _ = std::fs::remove_file(&tmp);
    decoded
}

/// `p` scaled down so its longest side is at most `side` (halving first, so large reductions stay smooth).
pub fn fit(p: &Pixmap, side: u32) -> Pixmap {
    let longest = p.width().max(p.height());
    if longest <= side {
        return p.clone();
    }
    let mut cur = p.clone();
    while cur.width().max(cur.height()) >= side * 2 {
        cur = scaled(&cur, 0.5);
    }
    let longest = cur.width().max(cur.height());
    if longest > side { scaled(&cur, side as f32 / longest as f32) } else { cur }
}

fn scaled(p: &Pixmap, factor: f32) -> Pixmap {
    let w = ((p.width() as f32 * factor).round() as u32).max(1);
    let h = ((p.height() as f32 * factor).round() as u32).max(1);
    let Some(mut out) = Pixmap::new(w, h) else { return p.clone() };
    let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() };
    out.draw_pixmap(0, 0, p.as_ref(), &paint, Transform::from_scale(w as f32 / p.width() as f32, h as f32 / p.height() as f32), None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimchi_media::tiny_skia::Color;
    use serde_json::json;

    #[test]
    fn only_looking_commands_carry_pictures() {
        let v = json!({ "path": "/tmp/frame.png" });
        assert_eq!(pictures_in("project.renderFrame", &v), vec![PathBuf::from("/tmp/frame.png")]);
        assert!(pictures_in("clip.get", &v).is_empty());
        assert!(pictures_in("media.frame", &json!({ "path": "" })).is_empty());
    }

    #[tokio::test]
    async fn pictures_fit_what_models_take() {
        let dir = tempfile::tempdir().unwrap();
        let mut big = Pixmap::new(3840, 2160).unwrap();
        big.fill(Color::from_rgba8(200, 40, 60, 255));
        let path = dir.path().join("big.png");
        big.save_png(&path).unwrap();
        let p = picture(&path).await.unwrap();
        assert_eq!((p.width, p.height), (1568, 882));
        let back = Pixmap::decode_png(&base64::engine::general_purpose::STANDARD.decode(&p.data).unwrap()).unwrap();
        assert_eq!(back.pixel(10, 10).unwrap().demultiply().red(), 200);

        // A small PNG goes as it is.
        let mut small = Pixmap::new(320, 180).unwrap();
        small.fill(Color::from_rgba8(1, 2, 3, 255));
        let path = dir.path().join("small.png");
        small.save_png(&path).unwrap();
        let p = picture(&path).await.unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(&p.data).unwrap(), std::fs::read(&path).unwrap());
        assert!(picture(&dir.path().join("missing.png")).await.is_err());
    }
}
