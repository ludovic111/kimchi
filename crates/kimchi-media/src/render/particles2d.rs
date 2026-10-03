//! Draws a 2D particle layer's particles (from `ParticleSystem::at`): circles, squares,
//! triangles, stars, sparks (streaks along their motion) or pictures, each with its own colour,
//! opacity, size and turn.

use std::sync::Arc;

use kimchi_core::motion::particles::{Particle, ParticleSystem};
use tiny_skia::{Color, FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, Rect, Shader, Stroke, Transform};

/// Draws `parts` (positions relative to the emitter) with `ts`, which maps emitter-relative
/// pixels to the target. `picture` is the asset of image particles with its size.
pub(crate) fn draw(target: &mut Pixmap, ps: &ParticleSystem, parts: &[Particle], ts: Transform, alpha: f32, picture: Option<&(Arc<Pixmap>, f64, f64)>) {
    let shape = ps.shape.as_deref().unwrap_or("circle");
    let unit = shape_path(shape);
    let stretch = ps.stretch.max(0.0) as f32;
    let (w, h) = (target.width() as f32, target.height() as f32);
    let device = (ts.sx * ts.sy - ts.kx * ts.ky).abs().sqrt();
    for part in parts {
        let c = part.color.0;
        let a = (c[3] / 255.0) as f32 * alpha;
        let size = part.size as f32;
        if a <= 0.002 || size <= 0.0 || !size.is_finite() {
            continue;
        }
        let (x, y) = (part.pos[0] as f32, part.pos[1] as f32);
        if !(x.is_finite() && y.is_finite()) {
            continue;
        }
        // Skip what lands well outside the picture.
        let mut at = Point::from_xy(x, y);
        ts.map_point(&mut at);
        let reach = (size + stretch * (part.vel[0].hypot(part.vel[1]) as f32)) * device + 4.0;
        if at.x < -reach || at.y < -reach || at.x > w + reach || at.y > h + reach {
            continue;
        }
        let color = Color::from_rgba((c[0] / 255.0) as f32, (c[1] / 255.0) as f32, (c[2] / 255.0) as f32, a.clamp(0.0, 1.0)).unwrap_or(Color::WHITE);
        let paint = Paint { shader: Shader::SolidColor(color), anti_alias: true, ..Paint::default() };
        match shape {
            "spark" => {
                let (vx, vy) = (part.vel[0] as f32 * stretch, part.vel[1] as f32 * stretch);
                let mut pb = PathBuilder::new();
                pb.move_to(x, y);
                // A still spark is a dot.
                pb.line_to(x - vx + 0.01, y - vy);
                if let Some(path) = pb.finish() {
                    let stroke = Stroke { width: size, line_cap: LineCap::Round, ..Stroke::default() };
                    target.stroke_path(&path, &paint, &stroke, ts, None);
                }
            }
            "image" => {
                let Some((pic, nw, nh)) = picture else { continue };
                let aspect = (*nw / nh.max(1e-6)) as f32;
                let (pw, ph) = if aspect >= 1.0 { (size, size / aspect) } else { (size * aspect, size) };
                let place = ts
                    .pre_translate(x, y)
                    .pre_rotate(part.rotation as f32)
                    .pre_translate(-pw / 2.0, -ph / 2.0)
                    .pre_scale(pw / pic.width() as f32, ph / pic.height() as f32);
                let paint = PixmapPaint { opacity: a.clamp(0.0, 1.0), quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
                target.draw_pixmap(0, 0, pic.as_ref().as_ref(), &paint, place, None);
            }
            _ => {
                let Some(path) = &unit else { continue };
                let place = ts.pre_translate(x, y).pre_rotate(part.rotation as f32).pre_scale(size, size);
                target.fill_path(path, &paint, FillRule::Winding, place, None);
            }
        }
    }
}

/// A particle's outline at size 1 (diameter), centred.
fn shape_path(shape: &str) -> Option<Path> {
    match shape {
        "square" => Some(PathBuilder::from_rect(Rect::from_xywh(-0.5, -0.5, 1.0, 1.0)?)),
        "triangle" => super::paint::polygon(3.0, 0.5, 0.0),
        "star" => super::paint::star(5.0, 0.5, 0.2),
        _ => PathBuilder::from_circle(0.0, 0.0, 0.5),
    }
}
