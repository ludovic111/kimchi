//! The compositor: every picture kimchi shows or exports is put together here, one frame at a
//! time, so the preview is exactly what renders.
//!
//! ffmpeg only decodes (each media clip into raw RGBA at the size it is drawn, see [`source`])
//! and, on export, encodes. Each frame starts from the project's background; then every visible
//! clip, bottom track first, is drawn at its keyframed placement (position, scale, rotation,
//! opacity, blur) times its fades: media pictures, solids, titles, 2D motion scenes
//! ([`flat`]) and 3D scenes ([`space`]).
//!
//! A [`Renderer`] is used in one of two ways. Playback and export ask for frames in order
//! ([`Renderer::frame`]): every playing video clip then has a decoder running ahead, started a
//! little before the clip comes up. Scrubbing asks for any time ([`Renderer::still`]): each
//! visible video is decoded at that time, in parallel. Pictures are kept between frames, so a
//! still image or a title that doesn't move is decoded or drawn once.
//!
//! Timing matches the old ffmpeg graph: a clip shows from half a frame before its start to half a
//! frame before its end, and fades are linear in opacity.

pub(crate) mod flat;
pub(crate) mod paint;
pub(crate) mod source;
pub mod space;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use kimchi_core::{Clip, ClipContent, Fit, Id, MediaKind, Placement, Project, Scene, TrackKind};
use tiny_skia::{FillRule, Pixmap, PixmapPaint, Transform};

use self::source::VideoStream;
use crate::{MediaResult, Tools};

/// Seconds ahead of a clip's start its decoder is started during playback.
const PREFETCH: f64 = 0.75;
/// Largest side stills and image layers are decoded at.
const MAX_STILL: f64 = 4096.0;

/// Puts frames of a project together at one output size.
pub struct Renderer {
    tools: Tools,
    project: Arc<Project>,
    width: u32,
    height: u32,
    fps: f64,
    strict: bool,
    /// Output pixels per project pixel.
    sx: f32,
    sy: f32,
    streams: HashMap<StreamKey, VideoStream>,
    /// Frames grabbed for the still being drawn (scrubbing).
    grabbed: HashMap<StreamKey, Arc<Pixmap>>,
}

/// Decoded pictures by file and size.
type Stills = Mutex<HashMap<(PathBuf, u32, u32), Arc<Pixmap>>>;
/// Each title's last picture, with the key of what it showed.
type Titles = Mutex<HashMap<Id, (String, Arc<Pixmap>)>>;

/// Decoded stills, shared by every renderer (the preview, playback and exports draw the same
/// pictures).
fn stills() -> &'static Stills {
    static S: OnceLock<Stills> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The last picture of each title, by what it showed (titles rarely move).
fn titles() -> &'static Titles {
    static T: OnceLock<Titles> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A decoder's owner: a clip, or an image layer (by its asset reference) inside a motion clip.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StreamKey(Id, Option<String>);

impl Renderer {
    /// A renderer for `project` at `width`×`height` (rounded down to even numbers) and `fps`.
    /// Media are read through their proxies when those exist.
    pub fn new(tools: &Tools, project: &Project, width: u32, height: u32, fps: f64) -> Self {
        let (width, height) = (even(width), even(height));
        Self::with_project(tools, playable(project), width, height, fps, false)
    }

    /// Render original media and report failures instead of omitting clips.
    pub fn for_export(tools: &Tools, project: &Project, width: u32, height: u32, fps: f64) -> Self {
        Self::with_project(tools, project.clone(), width, height, fps, true)
    }

    fn with_project(tools: &Tools, project: Project, width: u32, height: u32, fps: f64, strict: bool) -> Self {
        let (width, height) = (even(width), even(height));
        let ps = &project.settings;
        Self {
            tools: tools.clone(),
            sx: width as f32 / ps.width.max(1) as f32,
            sy: height as f32 / ps.height.max(1) as f32,
            project: Arc::new(project),
            width,
            height,
            fps: if fps.is_finite() && fps > 0.0 { fps } else { 30.0 },
            strict,
            streams: HashMap::new(),
            grabbed: HashMap::new(),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Which 3D renderer draws scenes: "gpu (<adapter>)" or "cpu".
    pub fn engine() -> String {
        lock(space::shared()).describe()
    }

    /// The frame at `t` when frames are asked for in order (playback, export).
    pub fn frame(&mut self, t: f64) -> MediaResult<Pixmap> {
        let canvas = self.compose(t, true)?;
        // Decoders for clips that are about to start, so they are ready on time.
        let soon: Vec<(Clip, f64)> = self
            .visible_clips(t + PREFETCH)
            .into_iter()
            .chain(self.visible_clips(t + PREFETCH / 2.0))
            .filter(|c| c.start > t)
            .map(|c| (c, 0.0))
            .collect();
        for (clip, local) in soon {
            let key = StreamKey(clip.id, None);
            if !self.streams.contains_key(&key) {
                let _ = self.start_stream(&clip, key, local);
            }
        }
        Ok(canvas)
    }

    /// The frame at `t`, wherever the previous one was (scrubbing). Videos are decoded at `t`.
    pub fn still(&mut self, t: f64) -> MediaResult<Pixmap> {
        self.streams.clear();
        self.grab_all(t);
        let canvas = self.compose(t, false);
        self.grabbed.clear();
        canvas
    }

    /// Picture clips on screen at `t`, bottom first.
    fn visible_clips(&self, t: f64) -> Vec<Clip> {
        let half = 0.5 / self.fps;
        let mut out = vec![];
        for track in self.project.tracks.iter().rev().filter(|tr| tr.kind == TrackKind::Video && !tr.hidden) {
            for c in &track.clips {
                if t >= c.start - half - 1e-9 && t < c.end() - half - 1e-9 {
                    out.push(c.clone());
                }
            }
        }
        out
    }

    fn compose(&mut self, t: f64, streaming: bool) -> MediaResult<Pixmap> {
        let mut canvas = Pixmap::new(self.width, self.height).expect("non-empty canvas");
        canvas.fill(paint::color(&self.project.settings.background));
        let mut used = HashSet::new();
        for clip in self.visible_clips(t) {
            if let Err(e) = self.draw_clip(&mut canvas, &clip, t, streaming, &mut used) {
                if self.strict {
                    return Err(e);
                }
                tracing::warn!(clip = %clip.name, "skipped in this frame: {e}");
            }
        }
        if streaming {
            self.streams.retain(|k, _| used.contains(k) || self.project.clip(k.0).is_some_and(|c| c.start > t));
        }
        Ok(canvas)
    }

    fn draw_clip(&mut self, canvas: &mut Pixmap, clip: &Clip, t: f64, streaming: bool, used: &mut HashSet<StreamKey>) -> MediaResult<()> {
        let pl = clip.placement_at(t);
        let alpha = (pl.opacity * fade(clip, t)) as f32;
        if alpha <= 0.0 || pl.scale_x <= 0.0 || pl.scale_y <= 0.0 {
            return Ok(());
        }
        let (w, h) = (self.width as f32, self.height as f32);
        // Where the clip's centre goes and how it turns.
        let center = Transform::from_translate(w / 2.0 + pl.x as f32 * self.sx, h / 2.0 + pl.y as f32 * self.sy).pre_rotate(pl.rotation as f32);
        let blur = pl.blur as f32 * self.sx;
        match &clip.content {
            ClipContent::Pending { .. } => Ok(()),
            ClipContent::Solid { color } => {
                let path = paint::rect(w, h, 0.0).expect("non-empty");
                let ts = center.pre_scale(pl.scale_x as f32, pl.scale_y as f32);
                let spec = paint::FillSpec::Solid(paint::color(color));
                self.layer(canvas, alpha, blur, |own, a| own.fill_path(&path, &spec.paint(a), FillRule::Winding, ts, None));
                Ok(())
            }
            ClipContent::Media { asset_id } => {
                let asset = self.project.asset(*asset_id).cloned().ok_or_else(|| crate::MediaError::Unsupported(format!("missing asset {asset_id}")))?;
                if !Path::new(&asset.path).is_file() {
                    return Err(crate::MediaError::Unsupported(format!("missing media file {}", asset.path)));
                }
                let (dw, dh) = self.decode_size(clip, asset.meta.width, asset.meta.height);
                let pic = match asset.kind {
                    MediaKind::Image => self.still_image(Path::new(&asset.path), dw, dh)?,
                    MediaKind::Video if asset.meta.has_video => {
                        let key = StreamKey(clip.id, None);
                        used.insert(key.clone());
                        let local = t - clip.start;
                        if streaming {
                            self.stream_frame(clip, key, local)?
                        } else {
                            self.grabbed.get(&key).cloned()
                        }
                        .ok_or_else(|| crate::MediaError::Unsupported("no frame".into()))?
                    }
                    _ => return Ok(()),
                };
                let (fw, fh) = fitted(clip.transform.fit, asset.meta.width, asset.meta.height, w, h);
                let ts = center
                    .pre_scale((fw * pl.scale_x as f32) / pic.width() as f32, (fh * pl.scale_y as f32) / pic.height() as f32)
                    .pre_translate(-(pic.width() as f32) / 2.0, -(pic.height() as f32) / 2.0);
                self.layer(canvas, alpha, blur, |own, a| draw_picture(own, &pic, ts, a));
                Ok(())
            }
            ClipContent::Text { .. } => {
                let pic = self.title(clip, &pl, t);
                self.layer(canvas, alpha, blur, |own, a| {
                    own.draw_pixmap(0, 0, (*pic).as_ref(), &PixmapPaint { opacity: a, ..PixmapPaint::default() }, Transform::identity(), None)
                });
                Ok(())
            }
            ClipContent::Motion { scene, .. } => {
                let st = clip.scene_time(t);
                let scene = scene.clone();
                // Scene pixels map to the canvas through the clip's placement, around the canvas centre.
                let ts = center.pre_scale(pl.scale_x as f32, pl.scale_y as f32).pre_translate(-w / 2.0, -h / 2.0);
                let plain = ts.is_identity() && alpha >= 1.0 && blur <= 0.0;
                let mut own = if plain { None } else { Some(Pixmap::new(self.width, self.height).expect("non-empty")) };
                {
                    let target = own.as_mut().unwrap_or(canvas);
                    match &scene {
                        Scene::Flat(s) => {
                            let base = Transform::from_translate(w / 2.0, h / 2.0).pre_scale(self.sx, self.sy);
                            let sx = self.sx;
                            let mut pics = ScenePictures { r: self, clip: clip.id, streaming, used };
                            let mut fx = flat::Flat { pictures: &mut pics, scale: sx };
                            flat::draw(target, s, st, base, &mut fx);
                        }
                        Scene::Space(s) => {
                            let (width, height) = (self.width, self.height);
                            let mut pics = ScenePictures { r: self, clip: clip.id, streaming, used };
                            let img = lock(space::shared()).render(s, st, width, height, &mut pics)?;
                            draw_picture(target, &img, Transform::identity(), 1.0);
                        }
                    }
                }
                if let Some(mut own) = own {
                    if blur > 0.0 {
                        paint::blur(&mut own, blur);
                    }
                    let paint = PixmapPaint { opacity: alpha, quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
                    canvas.draw_pixmap(0, 0, own.as_ref(), &paint, ts, None);
                }
                Ok(())
            }
        }
    }

    /// Draws with `f` straight onto the canvas, or onto a layer that is blurred first.
    fn layer(&self, canvas: &mut Pixmap, alpha: f32, blur: f32, f: impl FnOnce(&mut Pixmap, f32)) {
        if blur <= 0.0 {
            f(canvas, alpha);
            return;
        }
        let mut own = Pixmap::new(self.width, self.height).expect("non-empty");
        f(&mut own, 1.0);
        paint::blur(&mut own, blur);
        canvas.draw_pixmap(0, 0, own.as_ref(), &PixmapPaint { opacity: alpha, ..PixmapPaint::default() }, Transform::identity(), None);
    }

    /// A title's picture (canvas-sized), redrawn only when what it shows changed.
    fn title(&mut self, clip: &Clip, pl: &Placement, t: f64) -> Arc<Pixmap> {
        let style = clip.text_at(t).expect("text clip");
        let key = format!(
            "{}|{}|{}|{}|{}|{}|{}x{}",
            serde_json::to_string(&style).unwrap_or_default(),
            pl.x,
            pl.y,
            pl.scale_x,
            pl.scale_y,
            pl.rotation,
            self.width,
            self.height
        );
        if let Some((k, p)) = lock(titles()).get(&clip.id)
            && *k == key
        {
            return p.clone();
        }
        let ps = &self.project.settings;
        let p = Arc::new(crate::text::rasterize_title(&style, pl, ps.width, ps.height, self.sx));
        let mut map = lock(titles());
        if map.len() > 256 {
            map.clear();
        }
        map.insert(clip.id, (key, p.clone()));
        p
    }

    /// Size to decode a clip's picture at: its fitted size at the largest scale it reaches,
    /// never more than the source or twice the output.
    fn decode_size(&self, clip: &Clip, sw: Option<u32>, sh: Option<u32>) -> (u32, u32) {
        let (w, h) = (self.width as f32, self.height as f32);
        let (fw, fh) = fitted(clip.transform.fit, sw, sh, w, h);
        let s = clip.max_scale().max(0.01) as f32;
        let (mut dw, mut dh) = (fw * s, fh * s);
        let (src_w, src_h) = (sw.unwrap_or(self.width) as f32, sh.unwrap_or(self.height) as f32);
        let cap = (src_w / dw).min(src_h / dh).min(2.0 * w.max(h) / dw.max(dh)).min(1.0);
        if cap < 1.0 {
            dw *= cap;
            dh *= cap;
        }
        ((dw.round() as u32).max(2), (dh.round() as u32).max(2))
    }

    fn still_image(&mut self, path: &Path, w: u32, h: u32) -> MediaResult<Arc<Pixmap>> {
        let key = (path.to_path_buf(), w, h);
        if let Some(p) = lock(stills()).get(&key) {
            return Ok(p.clone());
        }
        let p = Arc::new(source::grab(&self.tools, path, None, w, h)?);
        let mut map = lock(stills());
        if map.len() > 64 {
            map.clear();
        }
        map.insert(key, p.clone());
        Ok(p)
    }

    fn start_stream(&mut self, clip: &Clip, key: StreamKey, local: f64) -> MediaResult<()> {
        let Some(asset) = clip.asset_id().and_then(|id| self.project.asset(id)).cloned() else { return Ok(()) };
        if asset.kind != MediaKind::Video || !asset.meta.has_video {
            return Ok(());
        }
        let (dw, dh) = self.decode_size(clip, asset.meta.width, asset.meta.height);
        let local = local.max(0.0);
        let s = VideoStream::start(&self.tools, Path::new(&asset.path), clip.source_time(clip.start + local), clip.speed, self.fps, dw, dh, local)?;
        self.streams.insert(key, s);
        Ok(())
    }

    fn stream_frame(&mut self, clip: &Clip, key: StreamKey, local: f64) -> MediaResult<Option<Arc<Pixmap>>> {
        if !self.streams.get(&key).is_some_and(|s| s.serves(local)) {
            self.start_stream(clip, key.clone(), local)?;
        }
        Ok(self.streams.get_mut(&key).and_then(|s| s.at(local)))
    }

    /// Decodes, in parallel, the frame every visible video clip shows at `t`.
    fn grab_all(&mut self, t: f64) {
        let mut jobs = vec![];
        for clip in self.visible_clips(t) {
            let Some(asset) = clip.asset_id().and_then(|id| self.project.asset(id)) else { continue };
            if asset.kind != MediaKind::Video || !asset.meta.has_video {
                continue;
            }
            let (dw, dh) = self.decode_size(&clip, asset.meta.width, asset.meta.height);
            jobs.push((StreamKey(clip.id, None), PathBuf::from(&asset.path), clip.source_time(t), dw, dh));
        }
        let tools = &self.tools;
        let results: Vec<(StreamKey, MediaResult<Pixmap>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .into_iter()
                .map(|(key, path, at, w, h)| scope.spawn(move || (key, source::grab(tools, &path, Some(at), w, h))))
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        for (key, r) in results {
            match r {
                Ok(p) => {
                    self.grabbed.insert(key, Arc::new(p));
                }
                Err(e) => tracing::warn!("preview frame: {e}"),
            }
        }
    }

    /// A media item, by id, name or path, for scenes.
    fn resolve(&self, reference: &str) -> Option<(PathBuf, MediaKind, Option<u32>, Option<u32>)> {
        let p = &self.project;
        let asset = reference
            .parse::<Id>()
            .ok()
            .and_then(|id| p.asset(id))
            .or_else(|| p.assets.iter().find(|a| a.name == reference))
            .or_else(|| p.assets.iter().find(|a| a.name.eq_ignore_ascii_case(reference)));
        if let Some(a) = asset {
            return Some((PathBuf::from(&a.path), a.kind, a.meta.width, a.meta.height));
        }
        let path = PathBuf::from(reference);
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let video = ["mp4", "mov", "webm", "mkv", "m4v", "avi"].contains(&ext.as_str());
            return Some((path, if video { MediaKind::Video } else { MediaKind::Image }, None, None));
        }
        None
    }
}

/// Image layers and textures of a motion clip, from the renderer's decoders and stills.
struct ScenePictures<'a> {
    r: &'a mut Renderer,
    clip: Id,
    streaming: bool,
    used: &'a mut HashSet<StreamKey>,
}

impl ScenePictures<'_> {
    fn get(&mut self, reference: &str, time: f64) -> Option<(Arc<Pixmap>, f64, f64)> {
        let (path, kind, w, h) = self.r.resolve(reference)?;
        let (nw, nh) = match (w, h) {
            (Some(w), Some(h)) => (w as f64, h as f64),
            _ => match source::dimensions(&self.r.tools, &path) {
                Some((w, h)) => (w as f64, h as f64),
                None => return None,
            },
        };
        let k = (MAX_STILL / nw.max(nh)).min(1.0);
        let (dw, dh) = (((nw * k).round() as u32).max(1), ((nh * k).round() as u32).max(1));
        let pic = match kind {
            MediaKind::Video => {
                let key = StreamKey(self.clip, Some(reference.to_string()));
                self.used.insert(key.clone());
                if self.streaming {
                    if !self.r.streams.get(&key).is_some_and(|s| s.serves(time)) {
                        let s = VideoStream::start(&self.r.tools, &path, time.max(0.0), 1.0, self.r.fps, dw, dh, time.max(0.0)).ok()?;
                        self.r.streams.insert(key.clone(), s);
                    }
                    self.r.streams.get_mut(&key)?.at(time)?
                } else {
                    Arc::new(source::grab(&self.r.tools, &path, Some(time.max(0.0)), dw, dh).ok()?)
                }
            }
            _ => self.r.still_image(&path, dw, dh).ok()?,
        };
        Some((pic, nw, nh))
    }
}

impl flat::Pictures for ScenePictures<'_> {
    fn picture(&mut self, asset: &str, time: f64) -> Option<(Arc<Pixmap>, f64, f64)> {
        self.get(asset, time)
    }
}

impl space::Pictures for ScenePictures<'_> {
    fn picture(&mut self, asset: &str, time: f64) -> Option<Arc<Pixmap>> {
        self.get(asset, time).map(|(p, _, _)| p)
    }

    fn path(&self, reference: &str) -> Option<PathBuf> {
        self.r.resolve(reference).map(|(p, ..)| p)
    }
}

/// Draws a picture with a transform: pixel-exact when it is only moved by whole pixels.
fn draw_picture(target: &mut Pixmap, pic: &Pixmap, ts: Transform, alpha: f32) {
    let whole = ts.sx == 1.0 && ts.sy == 1.0 && ts.kx == 0.0 && ts.ky == 0.0 && ts.tx.fract() == 0.0 && ts.ty.fract() == 0.0;
    let quality = if whole { tiny_skia::FilterQuality::Nearest } else { tiny_skia::FilterQuality::Bilinear };
    target.draw_pixmap(0, 0, pic.as_ref(), &PixmapPaint { opacity: alpha, quality, ..PixmapPaint::default() }, ts, None);
}

/// A picture `sw`×`sh` fitted into `w`×`h` per `fit` (the output canvas at scale 1).
fn fitted(fit: Fit, sw: Option<u32>, sh: Option<u32>, w: f32, h: f32) -> (f32, f32) {
    let (iw, ih) = (sw.unwrap_or(w as u32).max(1) as f32, sh.unwrap_or(h as u32).max(1) as f32);
    match fit {
        Fit::Stretch => (w, h),
        Fit::Contain => {
            let s = (w / iw).min(h / ih);
            (iw * s, ih * s)
        }
        Fit::Cover => {
            let s = (w / iw).max(h / ih);
            (iw * s, ih * s)
        }
    }
}

/// Opacity from the clip's fades at `t` (linear, like ffmpeg's `fade`).
fn fade(c: &Clip, t: f64) -> f64 {
    let local = t - c.start;
    let mut k = 1.0;
    if c.fade_in > 1e-6 {
        k *= (local / c.fade_in).clamp(0.0, 1.0);
    }
    if c.fade_out > 1e-6 {
        k *= ((c.duration - local) / c.fade_out).clamp(0.0, 1.0);
    }
    k
}

/// The project as the renderer reads it: proxies where they exist, and without the media whose
/// files are gone (their clips are skipped instead of failing the whole frame).
pub(crate) fn playable(project: &Project) -> Project {
    let mut project = project.clone();
    project.assets.retain_mut(|a| {
        if let Some(proxy) = a.proxy.as_deref().filter(|p| Path::new(p).is_file()) {
            a.path = proxy.to_string();
        }
        Path::new(&a.path).is_file()
    });
    project
}

fn even(x: u32) -> u32 {
    (x.max(2) / 2) * 2
}

/// Straight (not premultiplied) RGBA bytes of an opaque frame.
pub fn to_rgba(p: Pixmap) -> Vec<u8> {
    // The background is opaque, so premultiplied = straight.
    p.take()
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send>() {}
    is_send::<Renderer>();
}
